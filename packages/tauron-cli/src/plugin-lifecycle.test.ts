// plugin-lifecycle.ts 的"诚实性"回归测试（轮 12）。
//
// 这一组用例钉的不是功能，而是**不许出现伪造成功**：`dev` / `test` / `pack`
// 三条命令此前都返回 `success: true` + 声称做了某件事，而实际上什么都没做
// （不启动服务、不跑测试、不写归档）。这类缺陷编译期与功能测试都抓不到：
// 它们只有"对着代码问一句'它真的做了吗'"才会暴露。
//
// 断言方式刻意选成"正面断言真实字段 + 反面断言不再出现的字段"：
// 只防前者会被下一个人换个说法绕过去，只防后者会漏掉"换成别的假字段"。

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pluginDev, pluginTest, pluginPack, pluginSign } from './plugin-lifecycle.js';

const OPTS = { verbose: false, dryRun: false, force: false };

function dataOf(result: { data?: unknown }): Record<string, unknown> {
  return (result.data ?? {}) as Record<string, unknown>;
}

describe('plugin lifecycle 诚实性', () => {
  let dir: string;
  let prevCwd: string;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'tauron-cli-honest-'));
    prevCwd = process.cwd();
    process.chdir(dir);
  });

  afterEach(() => {
    process.chdir(prevCwd);
    rmSync(dir, { recursive: true, force: true });
  });

  const writeManifest = () =>
    writeFileSync(
      join(dir, 'tauron.plugin.json'),
      JSON.stringify({
        id: 'com.example.demo',
        name: 'demo',
        version: '1.0.0',
        type: 'js',
        entry: { main: 'src/index.js' },
      }),
    );

  it('plugin dev 不再谎报"已启动 dev server"（无服务、无 watcher、无端口）', () => {
    writeManifest();
    const result = pluginDev(OPTS);

    expect(result.success, 'dev server 未实现 → 不得报成功').toBe(false);
    expect(result.message).toContain('未实现');
    // 反面：此前这些话术与 `port: 8080` 都是编的。
    expect(result.message).not.toContain('Dev server started');
    const data = dataOf(result);
    expect(data.ready).toBe(false);
    expect(data.simulated).toBe(true);
    expect('port' in data, '不得给出编造的端口号').toBe(false);
    expect('watch' in data, '不得声称有 watch').toBe(false);
    // 正面：清单信息是真的读到了（不是空壳）。
    expect(data.pluginId).toBe('com.example.demo');
  });

  it('plugin test 不再把"测试文件个数"当成"通过的用例数"', () => {
    writeFileSync(join(dir, 'tauron.plugin.json'), '{}');
    // 造两个测试文件（必须在 `test/` 目录下——那是本命令唯一会看的地方）：
    // 旧实现会报 passed=2、failed=0。
    mkdirSync(join(dir, 'test'), { recursive: true });
    writeFileSync(join(dir, 'test', 'a.test.ts'), '// 从不被执行\n');
    writeFileSync(join(dir, 'test', 'b.test.js'), '// 从不被执行\n');
    const result = pluginTest(OPTS);

    expect(result.success, '没有测试运行器 → 不得报成功').toBe(false);
    // 两条分支都不得声称跑过：无 test/ 目录时如实说"没有测试目录"。
    expect(String(result.message)).not.toContain('Running tests');
    const data = dataOf(result);
    expect(data.executed, '必须如实标注未执行').toBe(false);
    expect(data.simulated).toBe(true);
    // 反面：绝不能给出"执行结果"形态的计数。
    expect('passed' in data, '不得给出编造的 passed').toBe(false);
    expect('failed' in data, '不得给出编造的 failed').toBe(false);
    expect(String(result.message)).not.toContain('Running tests');
    // 正面：文件发现是真的。
    expect((data.testFiles as string[]).length).toBe(2);
  });

  it('plugin pack 不产出归档且不声称产出（清单只作"计划"）', () => {
    writeManifest();
    const result = pluginPack(OPTS);

    expect(result.success, '没有归档写入器 → 不得报成功').toBe(false);
    expect(result.message).toContain('未实现');
    const data = dataOf(result);
    expect(data.package, '不得给出并不存在的产物路径').toBeNull();
    expect(data.simulated).toBe(true);
    expect(String(result.message)).not.toContain('Packed plugin');
    // 目录里真的没有任何 .tgz —— 这就是"它没写文件"的实证。
    expect(readdirSync(dir).filter((f) => f.endsWith('.tgz'))).toEqual([]);
    expect(existsSync(join(dir, 'com.example.demo-1.0.0.tgz'))).toBe(false);
    // 正面：清单与体积是真的算出来的（计划内容）。
    expect(data.plannedPackage).toBe('com.example.demo-1.0.0.tgz');
    expect(data.files).toContain('tauron.plugin.json');
  });

  it('plugin sign 在缺包时如实失败，并点明 pack 也不产出归档（不让用户空转）', () => {
    writeManifest();
    const result = pluginSign(OPTS);
    expect(result.success).toBe(false);
    expect(result.message).toContain('不产出');
    expect(dataOf(result).package).toBeNull();
  });
});
