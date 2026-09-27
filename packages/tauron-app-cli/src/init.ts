// tauron-app init — 一键接入现有 Tauri 项目。
//
// 职责：检测项目结构，自动插入依赖和接线代码，生成配置文件。
// 每步幂等 + diff 预览 + --dry-run。

import * as fs from 'node:fs';
import * as path from 'node:path';

import { generateClientConfig } from './client-config.js';
import {
  ensureDir,
  writeFile,
  pathExists,
  findTauronRoot,
  toPosixRelative,
} from './fs-operations.js';

// ── 类型 ──

export interface InitConfig {
  configPath?: string;
  dryRun?: boolean;
  dir?: string;
  preset?: 'full' | 'minimal' | 'template';
  /**
   * tauron 源码检出根（可选）。
   *
   * 省略时从 **CLI 自身位置**逐级上溯探测——即「在 tauron 仓库内跑这一命令」的场景。
   * 若目标工程与 CLI 不在同一检出树内，必须显式给出，否则探测失败会如实报错。
   */
  tauronPath?: string;
}

export interface InitResult {
  ok: boolean;
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
  // Tauri 2 项目使用 lib.rs，Tauri 1 使用 main.rs
  if (pathExists(libRs)) {
    return { srcTauri, cargoToml, mainRs: libRs };
  }
  return { srcTauri, cargoToml, mainRs: mainRs };
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
 * 在 `[dependencies]` 段末尾追加一条依赖（幂等：同名已存在则跳过）。
 *
 * `depValue` 是完整的一行右侧（例如
 * `{ path = "../../crates/tauron-adapter", default-features = false, features = ["tauri"] }`）。
 * **不做版本号拼装**：tauron 的 crate 还没发布到 crates.io，写 `version` 只会产出
 * 一份 `cargo` 解析不了的坐标——这不是"待完善"，是错。
 */
function addCargoDependency(cargoPath: string, depName: string, depValue: string): boolean {
  const content = readFileContent(cargoPath);
  if (!content) return false;

  // 按「独立依赖名」判重，避免 `tauron-adapter` 命中 `tauron-adapter-extra`
  const dupRe = new RegExp(`^\\s*${depName.replace(/[-]/g, '\\-')}\\s*=`, 'm');
  if (dupRe.test(content)) return false;

  const lines = content.split('\n');
  let inDeps = false;
  let insertIdx = -1;
  for (const [i, line] of lines.entries()) {
    if (line.trim() === '[dependencies]') {
      inDeps = true;
      continue;
    }
    if (inDeps) {
      if (line.trim().startsWith('[')) break;
      if (line.trim() !== '' && !line.trim().startsWith('#')) {
        insertIdx = i;
      }
    }
  }
  if (insertIdx === -1) return false;

  lines.splice(insertIdx + 1, 0, `${depName} = ${depValue}`);
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
function wireTauriBuilder(mainRsPath: string): BuilderWiring | null {
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

  if (inserts.length > 0) {
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

  try {
    // 1. 检测 Tauri 项目结构
    const tauriProject = detectTauriProject(dir);
    if (!tauriProject) {
      return {
        ok: false,
        error: '未检测到 Tauri 项目结构（需要 src-tauri/Cargo.toml）',
      };
    }
    steps.push(`检测到 Tauri 项目：${tauriProject.srcTauri}`);

    // 2. 定位 tauron 检出根 —— 定位不到就**如实失败**，不写解析不了的依赖坐标。
    //    显式 --tauron-path 优先；判据仍是 crates/tauron-adapter/Cargo.toml 存在，
    //    给错目录会当场报错而不是写出一个指向空目录的依赖。
    const explicitRoot = config.tauronPath !== undefined ? path.resolve(config.tauronPath) : null;
    const tauronRoot = explicitRoot ?? findTauronRoot();
    if (tauronRoot === null) {
      return {
        ok: false,
        error:
          '未能在当前目录树定位 tauron 源码检出根（判据 crates/tauron-adapter/Cargo.toml）。tauron 尚未发布到 crates.io / npm，接入必须指向本机检出根；可用 --tauron-path 显式指定。',
      };
    }
    if (!pathExists(path.join(tauronRoot, 'crates', 'tauron-adapter', 'Cargo.toml'))) {
      return {
        ok: false,
        error: `--tauron-path 指向的目录不是 tauron 检出根：${tauronRoot}（判据 crates/tauron-adapter/Cargo.toml 不存在）`,
      };
    }
    const relRoot = toPosixRelative(path.resolve(dir), tauronRoot);
    const relCargoRoot = toPosixRelative(tauriProject.srcTauri, tauronRoot);
    steps.push(`tauron 检出根：${tauronRoot}（相对本项目 ${relRoot}）`);

    // 3. Cargo.toml 添加 path 依赖（不写 version：那个坐标今天解析不了）
    // Cargo resolves dependency paths from src-tauri/Cargo.toml; frontend file: specs
    // below are resolved from the project-root package.json and therefore use relRoot.
    const depValue = `{ path = "${relCargoRoot}/crates/tauron-adapter", default-features = false, features = ["tauri"] }`;
    if (dryRun) {
      steps.push(`Cargo.toml：将添加 tauron-adapter = ${depValue}`);
    } else if (addCargoDependency(tauriProject.cargoToml, 'tauron-adapter', depValue)) {
      steps.push(
        `Cargo.toml：添加 tauron-adapter（path 依赖，78 条命令；需要签名安装再加 plugin-install 特性）`,
      );
    } else {
      steps.push(`Cargo.toml：tauron-adapter 已存在或无法定位 [dependencies]，跳过`);
    }

    // 4. Rust 源码接线（root 注册形态）
    if (dryRun) {
      steps.push('Rust 源码：将注入 root 注册的 .plugin(..) 与 .invoke_handler(..)');
    } else {
      const wiring = wireTauriBuilder(tauriProject.mainRs);
      if (wiring === null) {
        steps.push(`Rust 源码：无法读取 ${tauriProject.mainRs}，跳过`);
      } else {
        for (const item of wiring.applied) {
          steps.push(`Rust 源码：${item}`);
        }
        for (const item of wiring.manual) {
          steps.push(`⚠ 需手工处理：${item}`);
        }
      }
    }

    // 5. 前端依赖（file: 指向检出根，而不是 workspace:* —— 后者只在同一 pnpm
    //    workspace 内成立，跨仓库必失败）
    const pkgJsonPath = detectFrontendPackageJson(dir);
    if (pkgJsonPath) {
      const hostSpec = `file:${relRoot}/packages/tauron-host`;
      const uiSpec = `file:${relRoot}/packages/tauron-ui`;
      if (dryRun) {
        steps.push(
          `前端 package.json：将添加 @tauron/host（${hostSpec}）与 @tauron/ui（${uiSpec}）`,
        );
      } else {
        addFrontendDependency(pkgJsonPath, '@tauron/host', hostSpec);
        addFrontendDependency(pkgJsonPath, '@tauron/ui', uiSpec);
        steps.push(`前端 package.json：@tauron/host / @tauron/ui 指向 ${relRoot}/packages/`);
      }
    }

    // 6. 生成 client-config.json
    const configPath = config.configPath ?? 'client-config.json';
    // 相对路径以项目目录（--dir）为基准，避免写到当前工作目录
    const configTarget = path.isAbsolute(configPath) ? configPath : path.join(dir, configPath);
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

    // 7. 生成 capabilities/default.json
    const capabilitiesDir = path.join(tauriProject.srcTauri, 'capabilities');
    if (!dryRun) {
      await ensureDir(capabilitiesDir);
      writeFileContent(path.join(capabilitiesDir, 'default.json'), generateCapabilitiesJson());
    }
    steps.push(
      `生成 capabilities/default.json（Tauri 2 IPC 授权：core:default + 覆盖 plugin-* 窗）`,
    );

    // 8. build.rs —— Tauri 工程缺它 cargo 构建必失败
    const buildRs = path.join(tauriProject.srcTauri, 'build.rs');
    if (!pathExists(buildRs)) {
      if (!dryRun) writeFileContent(buildRs, 'fn main() {\n    tauri_build::build()\n}\n');
      steps.push('生成 src-tauri/build.rs');
    } else {
      steps.push('src-tauri/build.rs 已存在，跳过');
    }

    // 9. 输出验证清单
    steps.push(``);
    steps.push(`✓ 接入完成。请运行以下命令验证：`);
    steps.push(`  cd src-tauri && cargo check`);
    steps.push(`  tauron-app doctor`);

    return { ok: true, steps };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
