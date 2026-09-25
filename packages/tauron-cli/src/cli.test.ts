import { describe, it, expect, afterEach } from 'vitest';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runCli } from './cli.js';
import type { CliOptions } from './types.js';

// 脚手架命令会**真的写盘**，所以必须把落盘根目录指到临时目录——
// 否则 `runCli(['create', 'my-app'])` 会在仓库里建出 `my-app/`。
const tmpDirs: string[] = [];

function mkTmp(): string {
  const dir = mkdtempSync(join(tmpdir(), 'tauron-cli-'));
  tmpDirs.push(dir);
  return dir;
}

function opts(cwd: string, extra: Partial<CliOptions> = {}): CliOptions {
  return { verbose: false, dryRun: false, force: false, cwd, ...extra };
}

afterEach(() => {
  for (const dir of tmpDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe('runCli', () => {
  it('returns help for no arguments', async () => {
    const result = await runCli([]);
    expect(result.success).toBe(false);
    expect(result.message).toContain('No command specified');
  });

  it('returns help for help command', async () => {
    const result = await runCli(['--help']);
    expect(result.success).toBe(true);
    expect(result.message).toContain('tauron v0.1.0');
  });

  it('returns version for version command', async () => {
    const result = await runCli(['--version']);
    expect(result.success).toBe(true);
    expect(result.message).toContain('v0.1.0');
  });

  it('runs doctor command', async () => {
    const result = await runCli(['doctor']);
    expect(result.success).toBe(true);
    expect(result.message).toContain('tauron doctor');
    // doctor 真实派生 6 个工具链探测进程，高载机器上整测可达数十秒。
  }, 60_000);

  // 轮 13 修正：此前这两条只调生成器就返回 "Created app/plugin …"，而
  // `scaffold.ts` / `plugin.ts` **从不碰文件系统**——命令是纯谎报。
  // 现在断言文件真的落到盘上（写进临时目录，不污染仓库）。
  it('runs create command（真的落盘）', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'my-app'], opts(cwd));
    expect(result.success).toBe(true);
    expect(result.message).toContain('Created app');
    expect(existsSync(join(cwd, 'my-app', 'package.json'))).toBe(true);
  });

  it('runs plugin new command（真的落盘）', async () => {
    const cwd = mkTmp();
    const result = await runCli(['plugin', 'new', 'my-plugin'], opts(cwd));
    expect(result.success).toBe(true);
    expect(result.message).toContain('Created plugin');
    expect(existsSync(join(cwd, 'my-plugin', 'package.json'))).toBe(true);
    expect(existsSync(join(cwd, 'my-plugin', 'src', 'index.js'))).toBe(true);
  });

  it('create --dry-run：只报告，不写盘', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'my-app'], opts(cwd, { dryRun: true }));
    expect(result.success).toBe(true);
    expect(result.message).toContain('--dry-run');
    expect(existsSync(join(cwd, 'my-app'))).toBe(false);
  });

  // 轮 13：`--type` 此前既没被 bin 放行、也只实现了 `--type=x` 一种写法，
  // 于是 `--type wasm` 静默产出 js 插件。两种写法都要生效，非法值要如实失败。
  it('plugin new --type wasm（空格写法）真的产出 wasm 骨架', async () => {
    const cwd = mkTmp();
    const result = await runCli(['plugin', 'new', 'calc', '--type', 'wasm'], opts(cwd));
    expect(result.success).toBe(true);
    expect(existsSync(join(cwd, 'calc', 'Cargo.toml'))).toBe(true);
    expect(existsSync(join(cwd, 'calc', 'src', 'lib.rs'))).toBe(true);
  });

  it('plugin new --type=process（等号写法）真的产出 process 骨架', async () => {
    const cwd = mkTmp();
    const result = await runCli(['plugin', 'new', 'sys', '--type=process'], opts(cwd));
    expect(result.success).toBe(true);
    expect(existsSync(join(cwd, 'sys', 'main.js'))).toBe(true);
  });

  it('plugin new --type 非法值：如实失败，不静默降级成 js', async () => {
    const cwd = mkTmp();
    const result = await runCli(['plugin', 'new', 'bad', '--type', 'bogus'], opts(cwd));
    expect(result.success).toBe(false);
    expect(result.message).toContain('Unknown plugin type');
    expect(existsSync(join(cwd, 'bad'))).toBe(false);
  });

  it('目标目录已存在且未加 --force：如实失败，不覆盖', async () => {
    const cwd = mkTmp();
    await runCli(['create', 'my-app'], opts(cwd));
    const again = await runCli(['create', 'my-app'], opts(cwd));
    expect(again.success).toBe(false);
    expect(again.message).toContain('--force');
  });

  // 轮 19：`--template` 此前**从未被解析**——`createCommand` 把 template 硬编码成
  // 'vanilla'，于是 `tauron create app --template react` 静默产出 vanilla 工程。
  // 与上一轮修掉的 `--type` 属同一类缺陷（帮助文本宣传了、实现里没有）。
  it('create --template react（空格写法）真的产出 React 骨架', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'web-app', '--template', 'react'], opts(cwd));
    expect(result.success).toBe(true);
    // React 入口含 JSX，必须落在 .tsx
    expect(existsSync(join(cwd, 'web-app', 'src', 'index.tsx'))).toBe(true);
    expect(existsSync(join(cwd, 'web-app', 'src', 'App.tsx'))).toBe(true);
    // 默认（vanilla）不该出现
    expect(existsSync(join(cwd, 'web-app', 'src', 'index.ts'))).toBe(false);
  });

  it('create --template=react（等号写法）同样生效', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'web-app2', '--template=react'], opts(cwd));
    expect(result.success).toBe(true);
    expect(existsSync(join(cwd, 'web-app2', 'src', 'index.tsx'))).toBe(true);
  });

  it('create 不带 --template 时默认 vanilla', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'plain'], opts(cwd));
    expect(result.success).toBe(true);
    expect(existsSync(join(cwd, 'plain', 'src', 'index.ts'))).toBe(true);
    expect(existsSync(join(cwd, 'plain', 'src', 'index.tsx'))).toBe(false);
  });

  it('create --template vue：已声明但未实现 → 如实失败，不静默降级成 vanilla', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'v', '--template', 'vue'], opts(cwd));
    expect(result.success).toBe(false);
    expect(result.message).toContain('not implemented');
    expect(existsSync(join(cwd, 'v'))).toBe(false);
  });

  it('create --template 非法值：如实失败并列出可选值', async () => {
    const cwd = mkTmp();
    const result = await runCli(['create', 'x', '--template', 'angular'], opts(cwd));
    expect(result.success).toBe(false);
    expect(result.message).toContain('Unknown template');
    expect(result.message).toContain('vanilla');
    expect(existsSync(join(cwd, 'x'))).toBe(false);
  });

  it('returns error for unknown command', async () => {
    const result = await runCli(['unknown']);
    expect(result.success).toBe(false);
    expect(result.message).toContain('Unknown command');
  });

  // 轮 12：dev / test / pack 三条命令**未实现**，因此必须以 `success: false`
  // 如实失败。此前这三条都返回 `success: true` 并声称做了事（启动 dev server /
  // 跑测试 / 写归档），而实际什么都没做——CI 会因此变绿。这些用例就是那次的锁。
  it('runs plugin dev command（未实现 → 如实失败，不谎报已启动）', async () => {
    const result = await runCli(['plugin', 'dev']);
    expect(result.success).toBe(false);
    expect(result.message).not.toContain('Dev server started');
  });

  it('runs plugin test command（未实现 → 如实失败，不编造通过数）', async () => {
    const result = await runCli(['plugin', 'test']);
    expect(result.success).toBe(false);
    expect(result.message).not.toContain('Running tests');
  });

  it('runs plugin pack command（未实现 → 如实失败，不声称产出归档）', async () => {
    const result = await runCli(['plugin', 'pack']);
    expect(result.success).toBe(false);
    expect(result.message).not.toContain('Packed plugin');
  });

  it('runs plugin sign command（无清单 → 如实失败，不假装签了）', async () => {
    const result = await runCli(['plugin', 'sign']);
    expect(result.success).toBe(false);
  });

  it('runs plugin publish command（无 manifest 路径：只提示先 new）', async () => {
    const result = await runCli(['plugin', 'publish']);
    // 这里只覆盖"没有 tauron.plugin.json"的分支；**真发布路径**（有包时不得谎报
    // 上传成功）在 plugin-sign.test.ts 里断言。
    expect(result.success).toBe(true);
    expect(result.message).toContain('tauron.plugin.json');
  });

  it('returns error for unknown plugin subcommand', async () => {
    const result = await runCli(['plugin', 'unknown']);
    expect(result.success).toBe(false);
  });

  it('returns error for create without name', async () => {
    const result = await runCli(['create']);
    expect(result.success).toBe(false);
    expect(result.message).toContain('Usage');
  });

  it('returns error for plugin new without name', async () => {
    const result = await runCli(['plugin', 'new']);
    expect(result.success).toBe(false);
    expect(result.message).toContain('Usage');
  });
});
