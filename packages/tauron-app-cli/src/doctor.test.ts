import { describe, expect, it } from 'vitest';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { doctor, type DoctorCheck } from './doctor.js';

// src/ → 包根 → packages/ → 仓库根
const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

function workspaceCheck(checks: DoctorCheck[]): DoctorCheck {
  const found = checks.find((c) => c.name === '工作区包');
  if (!found) throw new Error('doctor 未输出「工作区包」检查项');
  return found;
}

/**
 * 轮 16 修正：`checkWorkspacePackages` 曾探测 `packages/core|ui|cli`——仓库里
 * 根本没有这三个目录（目录名是 `tauron-*`，`@tauron/*` 是包名），于是在框架仓
 * 上跑 doctor 也恒报「缺少包」。这是**负向谎报**：把健康的仓库说成缺东西，
 * 集成方据此会去"修"一个不存在的问题。
 */
describe('doctor 工作区包探测', () => {
  it('框架仓根目录：三个包必须报就位（不得谎报缺少）', async () => {
    const result = await doctor(workspaceRoot);
    const check = workspaceCheck(result.checks);
    expect(check.status, check.message).toBe('pass');
    expect(check.message).toContain('就位');
  }, 30_000);

  it('集成方应用目录（无 packages/）：如实 skip，不得报 warn', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'tauron-doctor-app-'));
    try {
      // 只验本项：doctor() 在应用目录上还会因缺 src-tauri 报 fail，那是另一回事。
      const result = await doctor(dir);
      const check = workspaceCheck(result.checks);
      expect(check.status, check.message).toBe('skip');
      expect(check.message).toContain('本项不适用');
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  /**
   * 轮 17：框架仓的判据换成 `crates/tauron-adapter/Cargo.toml` 指纹，
   * 于是两件事同时被钉住——
   * ① 自带 `packages/` 的第三方 monorepo **不再**被当成坏框架仓（负向谎报）；
   * ② 真框架仓里裁掉工作区包必须报 **fail**：此前是 warn，而
   *    `ok = summary.fail === 0`，缺包永远拉不住退出码，等于诊断白跑。
   */
  it('第三方 monorepo（有 packages/ 但不是框架仓）：skip 而不是谎报缺包', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'tauron-doctor-consumer-'));
    try {
      mkdirSync(join(dir, 'packages', 'my-app'), { recursive: true });
      writeFileSync(
        join(dir, 'packages', 'my-app', 'package.json'),
        JSON.stringify({ name: 'my-app' }),
        'utf8',
      );
      const check = workspaceCheck((await doctor(dir)).checks);
      expect(check.status, check.message).toBe('skip');
      expect(check.message).toContain('本项不适用');
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('框架仓缺工作区包：报 fail（warn 会让 doctor 的退出码恒为 0）', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'tauron-doctor-partial-'));
    try {
      mkdirSync(join(dir, 'crates', 'tauron-adapter'), { recursive: true });
      writeFileSync(
        join(dir, 'crates', 'tauron-adapter', 'Cargo.toml'),
        '[package]\nname = "tauron-adapter"\n',
        'utf8',
      );
      mkdirSync(join(dir, 'packages', 'tauron-host'), { recursive: true });
      writeFileSync(
        join(dir, 'packages', 'tauron-host', 'package.json'),
        JSON.stringify({ name: '@tauron/host' }),
        'utf8',
      );
      const check = workspaceCheck((await doctor(dir)).checks);
      expect(check.status, check.message).toBe('fail');
      expect(check.message).toContain('@tauron/ui');
      expect(check.message).toContain('@tauron/app-cli');
      expect((await doctor(dir)).ok).toBe(false);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
