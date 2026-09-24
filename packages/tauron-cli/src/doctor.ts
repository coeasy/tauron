/**
 * tauron doctor — 环境诊断
 *
 * 检查运行时环境、依赖版本、文件系统权限等。
 */

import { execSync } from 'child_process';

export interface DoctorReport {
  nodeVersion: string;
  npmVersion: string;
  pnpmVersion: string | null;
  cargoVersion: string | null;
  rustcVersion: string | null;
  tauriCliVersion: string | null;
  os: string;
  platform: string;
  checks: DoctorCheck[];
}

export interface DoctorCheck {
  name: string;
  status: 'pass' | 'warn' | 'fail';
  message: string;
  details?: string;
}

function tryExec(command: string): string | null {
  try {
    return execSync(command, { encoding: 'utf-8', timeout: 5000 }).trim();
  } catch {
    return null;
  }
}

export function runDoctor(): DoctorReport {
  const nodeVersion = tryExec('node --version') ?? 'unknown';
  const npmVersion = tryExec('npm --version') ?? 'unknown';
  const pnpmVersion = tryExec('pnpm --version');
  const cargoVersion = tryExec('cargo --version');
  const rustcVersion = tryExec('rustc --version');
  const tauriCliVersion = tryExec('tauri --version');

  const checks: DoctorCheck[] = [];

  // Node version check
  const nodeCheck = checkNodeVersion(nodeVersion);
  checks.push(nodeCheck);

  // Package manager check
  checks.push({
    name: 'Package Manager',
    status: pnpmVersion ? 'pass' : 'warn',
    message: pnpmVersion ? `pnpm ${pnpmVersion}` : 'pnpm not found, using npm',
  });

  // Rust toolchain check
  if (cargoVersion && rustcVersion) {
    checks.push({
      name: 'Rust Toolchain',
      status: 'pass',
      message: `${cargoVersion}, ${rustcVersion}`,
    });
  } else {
    checks.push({
      name: 'Rust Toolchain',
      status: 'warn',
      message: 'Rust toolchain not found (needed for tauron-shell)',
    });
  }

  // Tauri CLI check
  if (tauriCliVersion) {
    checks.push({
      name: 'Tauri CLI',
      status: 'pass',
      message: tauriCliVersion,
    });
  } else {
    checks.push({
      name: 'Tauri CLI',
      status: 'warn',
      message: 'Tauri CLI not found (needed for desktop builds)',
    });
  }

  // OS check
  checks.push({
    name: 'Operating System',
    status: 'pass',
    message: `${process.platform} ${process.arch}`,
  });

  return {
    nodeVersion,
    npmVersion,
    pnpmVersion,
    cargoVersion,
    rustcVersion,
    tauriCliVersion,
    os: process.platform,
    platform: `${process.platform} ${process.arch}`,
    checks,
  };
}

function checkNodeVersion(version: string): DoctorCheck {
  if (version === 'unknown') {
    return {
      name: 'Node.js',
      status: 'fail',
      message: 'Node.js not found',
    };
  }

  const match = version.match(/v(\d+)\.(\d+)/);
  if (!match || !match[1]) {
    return {
      name: 'Node.js',
      status: 'warn',
      message: `Unexpected version format: ${version}`,
    };
  }

  const major = parseInt(match[1]!, 10);
  if (major < 18) {
    return {
      name: 'Node.js',
      status: 'warn',
      message: `${version} — recommended: v18+`,
      details: 'tauron requires Node.js 18 or later for ES modules and crypto.randomUUID()',
    };
  }

  return {
    name: 'Node.js',
    status: 'pass',
    message: version,
  };
}

/**
 * 格式化诊断报告
 */
export function formatDoctorReport(report: DoctorReport): string {
  const lines: string[] = [];
  lines.push('tauron doctor — Environment Report');
  lines.push('==================================');
  lines.push('');

  for (const check of report.checks) {
    const icon = check.status === 'pass' ? '✓' : check.status === 'warn' ? '⚠' : '✗';
    lines.push(`${icon} ${check.name}: ${check.message}`);
    if (check.details) {
      lines.push(`  ${check.details}`);
    }
  }

  lines.push('');
  const failCount = report.checks.filter((c) => c.status === 'fail').length;
  const warnCount = report.checks.filter((c) => c.status === 'warn').length;

  if (failCount > 0) {
    lines.push(`✗ ${failCount} check(s) failed — please fix before continuing.`);
  } else if (warnCount > 0) {
    lines.push(`⚠ ${warnCount} warning(s) — tauron may work with reduced functionality.`);
  } else {
    lines.push('✓ All checks passed — environment is ready.');
  }

  return lines.join('\n');
}
