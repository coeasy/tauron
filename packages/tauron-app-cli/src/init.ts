// tauron-app init — 一键接入现有 Tauri 项目。
//
// 职责：检测项目结构，自动插入依赖和接线代码，生成配置文件。
// 每步幂等 + diff 预览 + --dry-run。

import * as fs from 'node:fs';
import * as path from 'node:path';

import { generateClientConfig } from './client-config.js';
import {
  FRAMEWORK_VERSION,
  PUBLISHED_FRAMEWORK_VERSION,
  REGISTRY_PIN_IS_PUBLISHED,
} from './framework-version.js';
import { ensureDir, writeFile, pathExists, toPosixRelative } from './fs-operations.js';

// ── 类型 ──

export interface InitConfig {
  configPath?: string;
  dryRun?: boolean;
  dir?: string;
  preset?: 'full' | 'minimal' | 'template';
  /**
   * tauron 源码检出根（可选）。
   *
   * 只有显式传入时才使用本地源码；普通使用从 registry 获取固定版本。
   */
  tauronPath?: string;
}

export interface InitResult {
  ok: boolean;
  /** false means the existing command handler needs a manual, reported merge. */
  complete?: boolean;
  steps?: string[];
  error?: string;
}

// ── 检测函数 ──

function detectTauriProject(
  dir: string,
): { srcTauri: string; cargoToml: string; mainRs: string } | null {
  const srcTauri = path.join(dir, 'src-tauri');
  if (!pathExists(srcTauri)) return null;
  const cargoToml = path.join(srcTauri, 'Cargo.toml');
  const libRs = path.join(srcTauri, 'src', 'lib.rs');
  const mainRs = path.join(srcTauri, 'src', 'main.rs');
  if (!pathExists(cargoToml)) return null;
  const cargo = readFileContent(cargoToml) ?? '';
  if (!hasTauri2Dependency(cargo, dir)) return null;
  // Tauri 2 工程通常使用 lib.rs，也兼容仍使用 main.rs 的自定义工程布局。
  if (pathExists(libRs)) {
    return { srcTauri, cargoToml, mainRs: libRs };
  }
  return { srcTauri, cargoToml, mainRs: mainRs };
}

function hasTauri2Dependency(cargo: string, projectDir: string): boolean {
  const versionPattern = /(?:^|\n)\s*version\s*=\s*["']([^"']+)["']/m;
  const inline = /(?:^|\n)\s*tauri\s*=\s*\{([^}\n]+)\}/m.exec(cargo)?.[1];
  const simple = /(?:^|\n)\s*tauri\s*=\s*["']([^"']+)["']/m.exec(cargo)?.[1];
  let inTauriTable = false;
  const tauriTableLines: string[] = [];
  for (const line of cargo.split(/\r?\n/)) {
    if (line.trim().startsWith('[')) {
      inTauriTable = line.trim() === '[dependencies.tauri]';
      continue;
    }
    if (inTauriTable) tauriTableLines.push(line);
  }
  let version =
    (inline && versionPattern.exec(inline)?.[1]) ??
    simple ??
    versionPattern.exec(tauriTableLines.join('\n'))?.[1];

  if (version === undefined && inline?.includes('workspace = true')) {
    let parent = path.resolve(projectDir);
    for (let depth = 0; depth < 6; depth += 1) {
      const workspaceManifest = path.join(parent, 'Cargo.toml');
      if (pathExists(workspaceManifest)) {
        const workspaceCargo = readFileContent(workspaceManifest) ?? '';
        const workspaceLines: string[] = [];
        let inWorkspaceDependencies = false;
        for (const line of workspaceCargo.split(/\r?\n/)) {
          if (line.trim().startsWith('[')) {
            inWorkspaceDependencies = line.trim() === '[workspace.dependencies]';
            continue;
          }
          if (inWorkspaceDependencies) workspaceLines.push(line);
        }
        const section = workspaceLines.join('\n');
        const workspaceInline =
          section && /(?:^|\n)\s*tauri\s*=\s*\{([^}\n]+)\}/m.exec(section)?.[1];
        const workspaceSimple =
          section && /(?:^|\n)\s*tauri\s*=\s*["']([^"']+)["']/m.exec(section)?.[1];
        version = (workspaceInline && versionPattern.exec(workspaceInline)?.[1]) ?? workspaceSimple;
        if (version !== undefined) break;
      }
      const next = path.dirname(parent);
      if (next === parent) break;
      parent = next;
    }
  }
  return version !== undefined && /^2(?:\.|$|\s|\*)/.test(version);
}

function detectFrontendPackageJson(dir: string): string | null {
  const pkgPath = path.join(dir, 'package.json');
  return pathExists(pkgPath) ? pkgPath : null;
}

function readFileContent(filePath: string): string | null {
  try {
    return fs.readFileSync(filePath, 'utf-8');
  } catch {
    return null;
  }
}

function writeFileContent(filePath: string, content: string): void {
  fs.writeFileSync(filePath, content, 'utf-8');
}

// ── Cargo.toml 操作 ──

/**
 * 在 `[dependencies]` 段追加一条依赖（幂等：同名已存在则跳过）。
 *
 * `depValue` 是完整的一行右侧（例如
 * `{ path = "../../crates/tauron-adapter", default-features = false, features = ["tauri"] }`）。
 * 若宿主没有 `[dependencies]` 段，则创建该段并追加依赖。
 */
function addCargoDependency(
  cargoPath: string,
  depName: string,
  depValue: string,
  sectionName = 'dependencies',
): boolean {
  const content = readFileContent(cargoPath);
  if (!content) return false;

  // 按「独立依赖名」判重，避免 `tauron-adapter` 命中 `tauron-adapter-extra`。
  const dupRe = new RegExp(`^\\s*${depName.replace(/[-]/g, '\\-')}\\s*=`, 'm');
  if (dupRe.test(content)) return false;

  const lines = content.split('\n');
  const sectionHeader = `[${sectionName}]`;
  const sectionIndex = lines.findIndex((line) => line.trim() === sectionHeader);
  if (sectionIndex !== -1) {
    // Place the plain dependency before any nested table such as [dependencies.tauri].
    lines.splice(sectionIndex + 1, 0, `${depName} = ${depValue}`);
  } else {
    const nestedPrefix = `[${sectionName}.`;
    const nestedIndex = lines.findIndex((line) => line.trim().startsWith(nestedPrefix));
    const block = [sectionHeader, `${depName} = ${depValue}`, ''];
    if (nestedIndex === -1) lines.push('', ...block);
    else lines.splice(nestedIndex, 0, ...block);
  }
  writeFileContent(cargoPath, lines.join('\n'));
  return true;
}

// ── Rust 源码操作 ──

/** 宿主 `tauri::Builder` 链的接线结果。 */
interface BuilderWiring {
  /** 已实际写入源码的改动。 */
  applied: string[];
  /** **未**改源码、需要用户手工处理的事项（附可粘贴的代码）。 */
  manual: string[];
}

/**
 * 把 tauron 接进宿主的 `tauri::Builder` 链。
 *
 * 采用**应用层 root 注册**（与 `examples/minimal-app` 同形态），而不是
 * `.plugin(tauron_adapter::tauri::init())`：后者走 `plugin:tauron|*` 路由，而
 * `crates/tauron-adapter` 没有 `permissions/` 定义——启用能力检查时那条路由缺权限
 * 条目（部署配置缺口，见 docs/architecture/app-layer-wire.md §1）。root 注册用裸
 * 命令名，不受插件 ACL 管辖。
 *
 * **不自动改动已有的 `.invoke_handler(..)`**：Tauri 的 `invoke_handler` 是覆盖语义，
 * 追加第二次等于把宿主原有命令整片丢掉。这种情况只**如实报告**并给出合并指引。
 */
function wireTauriBuilder(mainRsPath: string, dryRun = false): BuilderWiring | null {
  const content = readFileContent(mainRsPath);
  if (!content) return null;

  const applied: string[] = [];
  const manual: string[] = [];
  const lines = content.split('\n');
  const runIdx = lines.findIndex((line) => line.includes('.run('));

  if (runIdx === -1) {
    return {
      applied,
      manual: [
        '未在宿主源码里找到 `.run(` 行，无法自动定位装配点。请手工在 Builder 链上加入：',
        '  .plugin(tauron_adapter::tauri::state_init_with_adapter_config(tauron_adapter::AdapterConfig::default()))',
        '  .invoke_handler(tauron_adapter::tauron_generate_handler![])',
      ],
    };
  }

  const hasState = content.includes('state_init_with_adapter_config');
  const hasTauronHandler =
    content.includes('tauron_generate_handler!') || content.includes('tauron_substrate_handler!');
  const hasOwnHandler = /\.invoke_handler\s*\(/.test(content);

  const inserts: string[] = [];
  if (!hasState) {
    inserts.push(
      '    .plugin(tauron_adapter::tauri::state_init_with_adapter_config(tauron_adapter::AdapterConfig::default()))',
    );
    applied.push('注入 .plugin(state_init_with_adapter_config(..))');
  }
  if (!hasTauronHandler) {
    if (hasOwnHandler) {
      manual.push(
        '宿主已有 .invoke_handler(..)：Tauri 的 invoke_handler 是覆盖语义，自动追加会丢掉你原有的命令。',
        '请把 tauron 的命令并入你原有的 generate_handler![..]（tauron_adapter::tauron_generate_handler![] 展开后的命令清单）。',
      );
    } else {
      inserts.push('    .invoke_handler(tauron_adapter::tauron_generate_handler![])');
      applied.push('注入 .invoke_handler(tauron_generate_handler![])');
    }
  }

  if (inserts.length > 0 && !dryRun) {
    lines.splice(runIdx, 0, ...inserts);
    writeFileContent(mainRsPath, lines.join('\n'));
  }
  return { applied, manual };
}

// ── 前端 package.json 操作 ──

function addFrontendDependency(pkgJsonPath: string, depName: string, version: string): boolean {
  const content = readFileContent(pkgJsonPath);
  if (!content) return false;

  try {
    const pkg = JSON.parse(content);
    pkg.dependencies = pkg.dependencies || {};
    if (pkg.dependencies[depName]) return false; // 已存在
    pkg.dependencies[depName] = version;
    writeFileContent(pkgJsonPath, JSON.stringify(pkg, null, 2) + '\n');
    return true;
  } catch {
    return false;
  }
}

// ── capabilities 生成 ──

/**
 * 生成 `src-tauri/capabilities/default.json`（Tauri v2 IPC 授权）。
 *
 * 语义澄清：tauron 的 `host_*` 命令走**应用层 root 注册**（裸命令名），不受 Tauri
 * 插件 ACL 管辖，所以这里**不该**塞 `core:host_xxx` 之类的伪权限——Tauri v2 的
 * capability 权限标识符里没有 tauron 命名空间，写进去只会让文件校验失败。
 * 本文件授予的是 Tauri **核心**命令面（事件监听/窗口操作等）。
 *
 * `windows` 必须覆盖 `plugin-*`：插件面板窗 label 恒为 `plugin-<插件 id>`，
 * 不在任何能力文件里就等于该窗口没有 IPC 访问权，插件界面会一片空白。
 */
function generateCapabilitiesJson(): string {
  return (
    JSON.stringify(
      {
        $schema: '../gen/schemas/desktop-schema.json',
        identifier: 'default',
        description:
          '主窗与插件面板窗的 IPC 授权：host_* 命令族走 root 注册（裸命令名），不受 Tauri 插件 ACL 管辖；本文件授予 Tauri 核心命令面。',
        windows: ['main', 'plugin-*'],
        permissions: ['core:default'],
      },
      null,
      2,
    ) + '\n'
  );
}

// ── init 主逻辑 ──

export async function initProject(config: InitConfig = {}): Promise<InitResult> {
  const steps: string[] = [];
  const dir = config.dir ?? '.';
  const dryRun = config.dryRun ?? false;
  const preset = config.preset ?? 'full';
  let requiresManual = false;

  try {
    // 1. 检测 Tauri 项目结构
    const tauriProject = detectTauriProject(dir);
    if (!tauriProject) {
      return {
        ok: false,
        error: '未检测到受支持的 Tauri 2 项目（需要 src-tauri/Cargo.toml 且依赖 tauri 2.x）',
      };
    }
    steps.push(`检测到 Tauri 项目：${tauriProject.srcTauri}`);

    // 2. 默认使用固定 registry 版本；只有显式提供 --tauron-path 才切换到本地源码。
    const tauronRoot =
      config.tauronPath === undefined ? null : path.resolve(path.resolve(dir), config.tauronPath);
    if (
      tauronRoot !== null &&
      !pathExists(path.join(tauronRoot, 'crates', 'tauron-adapter', 'Cargo.toml'))
    ) {
      return {
        ok: false,
        error: `--tauron-path 指向的目录不是 Tauron 源码根：${tauronRoot}（缺少 crates/tauron-adapter/Cargo.toml）`,
      };
    }
    const relRoot = tauronRoot === null ? null : toPosixRelative(path.resolve(dir), tauronRoot);
    const relCargoRoot =
      tauronRoot === null ? null : toPosixRelative(tauriProject.srcTauri, tauronRoot);
    steps.push(
      tauronRoot === null
        ? `Tauron 来源：registry 固定版本 ${FRAMEWORK_VERSION}`
        : `Tauron 来源：本地源码 ${tauronRoot}`,
    );
    // 轮 17：pin 与 registry 现值不一致时**必须说出来**。生成的工程钉的是源码版本
    // （降到 1.0.2 更糟：装得上但面向 1.1.0 API 的生成码编译不过），所以发版之前
    // registry 模式的 `npm install` / `cargo` 一定失败。不说，用户拿到的是一个
    // 静默坏掉的工程；说了，他有两步可走（`--tauron-path` 或等发版）。
    if (tauronRoot === null && !REGISTRY_PIN_IS_PUBLISHED) {
      steps.push(
        `⚠ registry 上 @tauron/* 与 crates.io 的现值是 ${PUBLISHED_FRAMEWORK_VERSION}，本工程钉的是**尚未发布**的 ${FRAMEWORK_VERSION}：npm install / cargo 现在会失败。请用 --tauron-path 指向本仓库源码，或等 ${FRAMEWORK_VERSION} 发布后再跑。`,
      );
    }

    // 3. Cargo path 以 src-tauri/Cargo.toml 为基准；registry 模式 pin 到同版本。
    const depValue =
      relCargoRoot === null
        ? `{ version = "=${FRAMEWORK_VERSION}", default-features = false, features = ["tauri"] }`
        : `{ version = "=${FRAMEWORK_VERSION}", path = "${relCargoRoot}/crates/tauron-adapter", default-features = false, features = ["tauri"] }`;
    if (dryRun) {
      steps.push(`Cargo.toml：将添加 tauron-adapter = ${depValue}`);
    } else if (addCargoDependency(tauriProject.cargoToml, 'tauron-adapter', depValue)) {
      steps.push(`Cargo.toml：添加 tauron-adapter（固定 ${FRAMEWORK_VERSION}，Tauri 2 特性）`);
    } else {
      steps.push(`Cargo.toml：tauron-adapter 已存在或无法定位 [dependencies]，跳过`);
    }

    // 4. Rust 源码接线（root 注册形态）；dry-run 只分析，不写源码。
    const wiring = wireTauriBuilder(tauriProject.mainRs, dryRun);
    if (wiring === null) {
      steps.push(`Rust 源码：无法读取 ${tauriProject.mainRs}，跳过`);
      requiresManual = true;
    } else {
      for (const item of wiring.applied) {
        steps.push(`${dryRun ? 'Rust 源码（预览）' : 'Rust 源码'}：${item}`);
      }
      for (const item of wiring.manual) {
        steps.push(`⚠ 需手工处理：${item}`);
      }
      requiresManual = wiring.manual.length > 0;
    }

    // 5. 前端依赖固定到 registry；本地贡献开发使用 file: checkout。
    const pkgJsonPath = detectFrontendPackageJson(dir);
    if (pkgJsonPath) {
      const hostSpec =
        relRoot === null ? FRAMEWORK_VERSION : `file:${relRoot}/packages/tauron-host`;
      const uiSpec = relRoot === null ? FRAMEWORK_VERSION : `file:${relRoot}/packages/tauron-ui`;
      if (dryRun) {
        steps.push(
          `前端 package.json：将添加 @tauron/host（${hostSpec}）与 @tauron/ui（${uiSpec}）`,
        );
      } else {
        const hostAdded = addFrontendDependency(pkgJsonPath, '@tauron/host', hostSpec);
        const uiAdded = addFrontendDependency(pkgJsonPath, '@tauron/ui', uiSpec);
        steps.push(
          hostAdded || uiAdded
            ? `前端 package.json：已添加 @tauron/host / @tauron/ui（${hostSpec}）`
            : '前端 package.json：依赖已存在，保留原版本和配置',
        );
      }
    }

    // 6. 生成 client-config.json
    const configPath = config.configPath ?? 'client-config.json';
    // 相对路径以项目目录（--dir）为基准，避免写到当前工作目录
    const configTarget = path.isAbsolute(configPath) ? configPath : path.join(dir, configPath);
    if (pathExists(configTarget)) {
      steps.push(`client-config.json：${configTarget} 已存在，保留原文件`);
    } else {
      const configResult = generateClientConfig({ preset, outputPath: configTarget });
      if (configResult.errors.length > 0) {
        steps.push(`client-config.json：${configResult.errors.join('; ')}`);
      } else {
        if (!dryRun) {
          const written = writeFile(configTarget, configResult.content);
          if (!written.ok) {
            return { ok: false, error: written.error ?? `写入 ${configTarget} 失败` };
          }
        }
        steps.push(`生成 ${configTarget}`);
      }
    }

    // 7. 生成独立 Tauron capability；保留用户现有 default.json 与自定义 ACL。
    const capabilitiesDir = path.join(tauriProject.srcTauri, 'capabilities');
    const tauronCapabilityPath = path.join(capabilitiesDir, 'tauron.json');
    if (!dryRun) {
      if (!pathExists(tauronCapabilityPath)) {
        await ensureDir(capabilitiesDir);
        writeFileContent(tauronCapabilityPath, generateCapabilitiesJson());
      }
    }
    steps.push(
      pathExists(tauronCapabilityPath)
        ? 'capabilities/tauron.json 已存在，保留原权限配置'
        : '生成 capabilities/tauron.json（独立权限文件，保留现有 capability 文件）',
    );

    // 8. build.rs —— Tauri 工程缺它 cargo 构建必失败
    const buildRs = path.join(tauriProject.srcTauri, 'build.rs');
    if (!pathExists(buildRs)) {
      if (!dryRun) {
        addCargoDependency(
          tauriProject.cargoToml,
          'tauri-build',
          '{ version = "2", features = [] }',
          'build-dependencies',
        );
        writeFileContent(buildRs, 'fn main() {\n    tauri_build::build()\n}\n');
      }
      steps.push('生成 src-tauri/build.rs，并确保存在 tauri-build 构建依赖');
    } else {
      steps.push('src-tauri/build.rs 已存在，跳过');
    }

    // 9. 输出验证清单
    steps.push(``);
    steps.push(
      requiresManual
        ? '检查结论：保留了现有 Tauri 命令处理器；请按上面的说明手工合并 Tauron 命令后再运行。'
        : '检查结论：Tauri Builder 已接线；现有配置均保留，重复运行不会重复写入。',
    );
    steps.push(`验证命令：`);
    steps.push(`  cd src-tauri && cargo check`);
    if (pkgJsonPath) steps.push(`  在项目根目录运行对应包管理器的 install，更新前端依赖锁文件`);
    steps.push(`  tauron-app doctor`);

    return { ok: true, complete: !requiresManual, steps };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
