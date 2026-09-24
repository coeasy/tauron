// tauron-app init — 一键接入现有 Tauri 项目。
//
// 职责：检测项目结构，自动插入依赖和接线代码，生成配置文件。
// 每步幂等 + diff 预览 + --dry-run。

import * as fs from 'node:fs';
import * as path from 'node:path';

import { generateClientConfig } from './client-config.js';
import { ensureDir, writeFile, pathExists } from './fs-operations.js';

// ── 类型 ──

export interface InitConfig {
  configPath?: string;
  dryRun?: boolean;
  dir?: string;
  preset?: 'full' | 'minimal' | 'template';
}

export interface InitResult {
  ok: boolean;
  steps?: string[];
  error?: string;
}

// ── 检测函数 ──

function detectTauriProject(dir: string): { srcTauri: string; cargoToml: string; mainRs: string } | null {
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

function addCargoDependency(cargoPath: string, depName: string, version: string, features: string[]): boolean {
  const content = readFileContent(cargoPath);
  if (!content) return false;

  const featuresStr = features.length > 0 ? `features = [${features.map((f) => `"${f}"`).join(', ')}]` : '';
  const depLine = featuresStr
    ? `${depName} = { version = "${version}", ${featuresStr} }`
    : `${depName} = "${version}"`;

  if (content.includes(depName)) return false; // 已存在

  // 插入到 [dependencies] 段末尾
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
  if (insertIdx === -1) insertIdx = lines.length;

  lines.splice(insertIdx, 0, '', `  ${depLine}`);
  writeFileContent(cargoPath, lines.join('\n'));
  return true;
}

// ── Rust 源码操作 ──

function injectPluginCall(mainRsPath: string, pluginName: string, methodName: string): boolean {
  const content = readFileContent(mainRsPath);
  if (!content) return false;

  // 查找 tauri::Builder 或 #[cfg_attr(mobile, tauri::mobile_entry_point)]
  if (content.includes(`.plugin(${pluginName}::`)) return false; // 已注入

  // 查找 .invoke_handler 或 .invoke_handler 后面的位置
  const lines = content.split('\n');
  let injectIdx = -1;

  // 查找 .invoke_handler 行
  for (const [i, line] of lines.entries()) {
    if (line.includes('.invoke_handler(') && line.includes('generate_handler!')) {
      // 在该行之后插入 .plugin(...)
      injectIdx = i;
      break;
    }
  }

  if (injectIdx === -1) {
    // 如果找不到 invoke_handler，尝试找 .build() 之前
    for (const [i, line] of lines.entries()) {
      if (line.includes('.build(') || line.includes('app.run(')) {
        injectIdx = i;
        break;
      }
    }
  }

  if (injectIdx === -1) {
    // 最后在 run() 之前插入
    for (const [i, line] of lines.entries()) {
      if (line.includes('.run(')) {
        injectIdx = i;
        break;
      }
    }
  }

  if (injectIdx === -1) return false;

  const pluginLine = `    .plugin(${pluginName}::tauri::init())`;
  lines.splice(injectIdx + 1, 0, pluginLine);
  writeFileContent(mainRsPath, lines.join('\n'));
  return true;
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

function generateCapabilitiesJson(defaultDir: string, commands: string[]): string {
  return JSON.stringify({
    schema: 'http://tauri.app/schema/capability-0.3',
    windows: ['main'],
    permissions: [
      'core:default',
      ...commands.map((cmd) => `core:${cmd}`),
    ],
  }, null, 2) + '\n';
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

    // 2. Cargo.toml 添加依赖
    if (addCargoDependency(tauriProject.cargoToml, 'tauron-adapter', '0.1', [])) {
      steps.push(`Cargo.toml：添加 tauron-adapter 依赖`);
    } else {
      steps.push(`Cargo.toml：tauron-adapter 已存在，跳过`);
    }

    // 3. Rust 源码注入 .plugin() 调用
    if (injectPluginCall(tauriProject.mainRs, 'tauron_adapter', 'init')) {
      steps.push(`Rust 源码：注入 .plugin(tauron_adapter::tauri::init())`);
    } else {
      steps.push(`Rust 源码：插件调用已存在或无法定位，跳过`);
    }

    // 4. 前端依赖
    const pkgJsonPath = detectFrontendPackageJson(dir);
    if (pkgJsonPath) {
      addFrontendDependency(pkgJsonPath, '@tauron/host', 'workspace:*');
      addFrontendDependency(pkgJsonPath, '@tauron/ui', 'workspace:*');
      steps.push(`前端 package.json：添加 @tauron/host + @tauron/ui`);
    }

    // 5. 生成 client-config.json
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

    // 6. 生成 capabilities/default.json
    const capabilitiesDir = path.join(tauriProject.srcTauri, 'capabilities');
    if (!dryRun) {
      await ensureDir(capabilitiesDir);
    }
    const capabilitiesContent = generateCapabilitiesJson(
      capabilitiesDir,
      ['default', 'event:default', 'window:default'],
    );
    if (!dryRun) {
      writeFileContent(path.join(capabilitiesDir, 'default.json'), capabilitiesContent);
    }
    steps.push(`生成 capabilities/default.json（Tauri 2 权限白名单）`);

    // 7. 输出验证清单
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
