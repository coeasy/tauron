import { describe, it, expect } from 'vitest';
import { runDoctor, formatDoctorReport } from './doctor.js';

// runDoctor 真实派生 6 个工具链探测进程（node/npm/pnpm/cargo/rustc/tauri
// --version）。高载机器（CI / 并行全量测试）上每个 spawn 可达秒级，整测
// 会超出 vitest 默认 5s——这里给显式预算，断言本身不变。
const DOCTOR_TIMEOUT = 60_000;

describe('runDoctor', () => {
  it('returns a valid report', () => {
    const report = runDoctor();
    expect(report).toBeDefined();
    expect(report.nodeVersion).toBeDefined();
    expect(report.os).toBeDefined();
    expect(report.platform).toBeDefined();
    expect(Array.isArray(report.checks)).toBe(true);
  }, DOCTOR_TIMEOUT);

  it('includes node version check', () => {
    const report = runDoctor();
    const nodeCheck = report.checks.find((c) => c.name === 'Node.js');
    expect(nodeCheck).toBeDefined();
    expect(['pass', 'warn', 'fail']).toContain(nodeCheck?.status);
  }, DOCTOR_TIMEOUT);

  it('includes package manager check', () => {
    const report = runDoctor();
    const pmCheck = report.checks.find((c) => c.name === 'Package Manager');
    expect(pmCheck).toBeDefined();
  }, DOCTOR_TIMEOUT);

  it('includes OS check', () => {
    const report = runDoctor();
    const osCheck = report.checks.find((c) => c.name === 'Operating System');
    expect(osCheck).toBeDefined();
    expect(osCheck?.status).toBe('pass');
  }, DOCTOR_TIMEOUT);

  it('has at least 3 checks', () => {
    const report = runDoctor();
    expect(report.checks.length).toBeGreaterThanOrEqual(3);
  }, DOCTOR_TIMEOUT);
});

describe('formatDoctorReport', () => {
  it('formats report as string', () => {
    const report = runDoctor();
    const output = formatDoctorReport(report);
    expect(typeof output).toBe('string');
    expect(output).toContain('tauron doctor');
    expect(output).toContain('Environment Report');
  }, DOCTOR_TIMEOUT);

  it('includes check statuses', () => {
    const report = runDoctor();
    const output = formatDoctorReport(report);
    expect(output).toMatch(/[✓⚠✗]/);
  }, DOCTOR_TIMEOUT);
});
