// tauron-app doctor — 集成健康度诊断。
//
// 检查：依赖版本匹配、tauri.conf.json CSP、capabilities 白名单、
// client-config.json 校验、权限词表新鲜度。

import * as fs from 'node:fs';
import * as path from 'node:path';

import { validateClientConfigFilePath } from './client-config.js';
import { pathExists, readJsonFile } from './fs-operations.js';

// ── 类型 ──

export interface DoctorCheck {
  name: string;
  status: 'pass' | 'warn' | 'fail' | 'skip';
  message: string;
  fix?: string;
}

export interface DoctorResult {
  ok: boolean;
  checks: DoctorCheck[];
  summary: {
    pass: number;
    warn: number;
    fail: number;
    skip: number;
  };
}

// ── 检查函数 ──

function checkCargoDependency(dir: string): DoctorCheck {
  const cargoPath = path.join(dir, 'src-tauri', 'Cargo.toml');
  if (!pathExists(cargoPath)) {
    return { name: 'Cargo.toml', status: 'fail', message: '未找到 src-tauri/Cargo.toml', fix: '确认项目是 Tauri 项目' };
  }
  const content = fs.readFileSync(cargoPath, 'utf-8');
  if (content.includes('tauron-adapter')) {
    return { name: 'tauron-adapter 依赖', status: 'pass', message: 'Cargo.toml 已包含 tauron-adapter' };
  }
  return {
    name: 'tauron-adapter 依赖',
    status: 'fail',
    message: 'Cargo.toml 缺少 tauron-adapter 依赖',
    fix: '运行 tauron-app init 自动添加',
  };
}

function checkRustPluginCall(dir: string): DoctorCheck {
  const libRs = path.join(dir, 'src-tauri', 'src', 'lib.rs');
  const mainRs = path.join(dir, 'src-tauri', 'src', 'main.rs');
  const target = pathExists(libRs) ? libRs : mainRs;
  if (!pathExists(target)) {
    return { name: 'Rust 入口', status: 'fail', message: '未找到 lib.rs 或 main.rs' };
  }
  const content = fs.readFileSync(target, 'utf-8');
  if (content.includes('tauron_adapter::tauri::init')) {
    return { name: '.plugin() 调用', status: 'pass', message: 'Rust 入口已注入插件初始化' };
  }
  return {
    name: '.plugin() 调用',
    status: 'fail',
    message: 'Rust 入口缺少 .plugin(tauron_adapter::tauri::init()) 调用',
    fix: '运行 tauron-app init 自动注入',
  };
}

function checkFrontendDeps(dir: string): DoctorCheck {
  const pkgPath = path.join(dir, 'package.json');
  if (!pathExists(pkgPath)) {
    return { name: '前端依赖', status: 'skip', message: '未找到 package.json' };
  }
  const pkg = readJsonFile<{ dependencies?: Record<string, string> }>(pkgPath);
  if (!pkg.ok || pkg.data === undefined) {
    return { name: '前端依赖', status: 'fail', message: 'package.json 读取失败' };
  }
  const deps = pkg.data.dependencies ?? {};
  const hasCore = !!deps['@tauron/host'];
  const hasUi = !!deps['@tauron/ui'];
  if (hasCore && hasUi) {
    return { name: '前端依赖', status: 'pass', message: 'package.json 已包含 @tauron/host + @tauron/ui' };
  }
  const missing: string[] = [];
  if (!hasCore) missing.push('@tauron/host');
  if (!hasUi) missing.push('@tauron/ui');
  return {
    name: '前端依赖',
    status: 'fail',
    message: `缺少前端依赖：${missing.join(', ')}`,
    fix: '运行 tauron-app init 自动添加',
  };
}

function checkClientConfig(dir: string): DoctorCheck {
  const configPath = path.join(dir, 'client-config.json');
  if (!pathExists(configPath)) {
    return {
      name: 'client-config.json',
      status: 'fail',
      message: '未找到 client-config.json',
      fix: '运行 tauron-app client config 生成',
    };
  }
  const content = fs.readFileSync(configPath, 'utf-8');
  try {
    const config = JSON.parse(content);
    // 基本结构检查
    const sections = ['registry', 'plugins', 'sandbox', 'security', 'lifecycle', 'i18n', 'recovery', 'observability', 'brand', 'upgrade'];
    const missingSections = sections.filter((s) => !(s in config));
    if (missingSections.length > 0) {
      return {
        name: 'client-config.json',
        status: 'warn',
        message: `缺少配置节：${missingSections.join(', ')}`,
        fix: '运行 tauron-app client config --preset full 重新生成',
      };
    }
    return { name: 'client-config.json', status: 'pass', message: '配置结构完整' };
  } catch (err) {
    return {
      name: 'client-config.json',
      status: 'fail',
      message: `JSON 解析失败：${err instanceof Error ? err.message : String(err)}`,
    };
  }
}

function checkCapabilities(dir: string): DoctorCheck {
  const capPath = path.join(dir, 'src-tauri', 'capabilities', 'default.json');
  if (!pathExists(capPath)) {
    return {
      name: 'capabilities/default.json',
      status: 'warn',
      message: '未找到 capabilities/default.json（Tauri 2 权限白名单）',
      fix: '运行 tauron-app init 生成',
    };
  }
  const content = fs.readFileSync(capPath, 'utf-8');
  try {
    const cap = JSON.parse(content);
    const permissions = cap.permissions ?? [];
    if (permissions.length === 0) {
      return {
        name: 'capabilities/default.json',
        status: 'warn',
        message: 'capabilities 白名单为空',
      };
    }
    return {
      name: 'capabilities/default.json',
      status: 'pass',
      message: `白名单包含 ${permissions.length} 个权限`,
    };
  } catch {
    return {
      name: 'capabilities/default.json',
      status: 'fail',
      message: 'capabilities JSON 解析失败',
    };
  }
}

function checkCsp(dir: string): DoctorCheck {
  const tauriConf = path.join(dir, 'src-tauri', 'tauri.conf.json');
  if (!pathExists(tauriConf)) {
    return { name: 'CSP 配置', status: 'skip', message: '未找到 tauri.conf.json' };
  }
  const content = fs.readFileSync(tauriConf, 'utf-8');
  const conf = JSON.parse(content);
  const csp = conf.app?.security?.csp ?? conf.security?.csp;
  if (!csp) {
    return {
      name: 'CSP 配置',
      status: 'warn',
      message: 'tauri.conf.json 未配置 CSP（Content Security Policy）',
      fix: '建议配置 CSP 以限制脚本来源',
    };
  }
  return { name: 'CSP 配置', status: 'pass', message: 'CSP 已配置' };
}

function checkPermissionIndex(dir: string): DoctorCheck {
  const indexPath = path.join(dir, 'schema', 'permissions.index.json');
  if (!pathExists(indexPath)) {
    return { name: '权限词表', status: 'skip', message: '未找到 schema/permissions.index.json' };
  }
  const content = fs.readFileSync(indexPath, 'utf-8');
  try {
    const index = JSON.parse(content);
    const count = Object.keys(index.permissions ?? {}).length;
    if (count > 0) {
      return { name: '权限词表', status: 'pass', message: `权限词表包含 ${count} 个权限` };
    }
    return { name: '权限词表', status: 'warn', message: '权限词表为空' };
  } catch {
    return { name: '权限词表', status: 'fail', message: '权限词表 JSON 解析失败' };
  }
}

function checkWorkspacePackages(dir: string): DoctorCheck {
  const corePkg = path.join(dir, 'packages', 'core', 'package.json');
  const uiPkg = path.join(dir, 'packages', 'ui', 'package.json');
  const cliPkg = path.join(dir, 'packages', 'cli', 'package.json');
  const packages = [
    { name: '@tauron/host', path: corePkg },
    { name: '@tauron/ui', path: uiPkg },
    { name: '@tauron/app-cli', path: cliPkg },
  ];
  const missing = packages.filter((p) => !pathExists(p.path));
  if (missing.length === 0) {
    return { name: '工作区包', status: 'pass', message: '所有工作区包已就位' };
  }
  return {
    name: '工作区包',
    status: 'warn',
    message: `缺少包：${missing.map((p) => p.name).join(', ')}`,
    fix: '确保在仓库根目录运行 tauron-app doctor',
  };
}

// ── 主逻辑 ──

export async function doctor(dir: string = '.'): Promise<DoctorResult> {
  const checks: DoctorCheck[] = [];

  checks.push(checkCargoDependency(dir));
  checks.push(checkRustPluginCall(dir));
  checks.push(checkFrontendDeps(dir));
  checks.push(checkClientConfig(dir));
  checks.push(checkCapabilities(dir));
  checks.push(checkCsp(dir));
  checks.push(checkPermissionIndex(dir));
  checks.push(checkWorkspacePackages(dir));

  const summary = {
    pass: checks.filter((c) => c.status === 'pass').length,
    warn: checks.filter((c) => c.status === 'warn').length,
    fail: checks.filter((c) => c.status === 'fail').length,
    skip: checks.filter((c) => c.status === 'skip').length,
  };

  return {
    ok: summary.fail === 0,
    checks,
    summary,
  };
}
