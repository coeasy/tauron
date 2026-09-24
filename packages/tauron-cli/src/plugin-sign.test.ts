/**
 * `pluginSign` 的诚实性测试（轮 11 修正）。
 *
 * 修正前的两个不实陈述（都有明确证据）：
 * 1. `algorithm: 'ed25519'`，实际算的是 `hash * 31` 的手写摘要；
 * 2. 消息声称写出 `.sig` 文件，代码里**没有任何写文件**。
 *
 * 这两条都属于"伪造优于诚实失败"，因此这里把"不许再犯"写成断言：
 * 算法名必须与实际计算一致、`simulated` 必须为 true、`.sig` 必须真的落盘。
 */
import { createHash } from 'crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { pluginPublish, pluginSign } from './plugin-lifecycle.js';
import type { CliOptions } from './types.js';

/** `CliOptions` 三个字段都是必填（`pluginSign` 只读 cwd，不看它们）。 */
const OPTS: CliOptions = { verbose: false, dryRun: false, force: false };

/** `CliResult.data` 是 `unknown`，取值前收窄成记录，避免 `any` 逃逸。 */
const dataOf = (result: { data?: unknown }): Record<string, unknown> =>
  (result.data ?? {}) as Record<string, unknown>;

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'tauron-cli-sign-'));
  process.chdir(dir);
});

afterEach(() => {
  process.chdir(tmpdir());
  rmSync(dir, { recursive: true, force: true });
});

/** 造一个可签的最小工作区：manifest + 包文件。 */
function seed(packageBytes: Buffer): { pkg: string } {
  writeFileSync(
    join(dir, 'tauron.plugin.json'),
    JSON.stringify({ id: 'com.example.sign', name: 'Sign Demo', version: '1.0.0' }),
  );
  const pkg = 'com.example.sign-1.0.0.tgz';
  writeFileSync(join(dir, pkg), packageBytes);
  return { pkg };
}

describe('pluginSign（诚实摘要，不谎报算法）', () => {
  it('缺 manifest / 缺包时如实返回 null，不伪造签名', () => {
    expect(dataOf(pluginSign(OPTS))['signature']).toBeNull();

    writeFileSync(
      join(dir, 'tauron.plugin.json'),
      JSON.stringify({ id: 'x', name: 'X', version: '1' }),
    );
    expect(dataOf(pluginSign(OPTS))['signature']).toBeNull();
  });

  it('摘要必须是真实 SHA-256（对已知向量逐字节相等）', () => {
    const bytes = Buffer.from('tauron', 'utf8');
    const { pkg } = seed(bytes);
    const data = dataOf(pluginSign(OPTS));

    const expected = createHash('sha256').update(bytes).digest('hex');
    expect(data['signatureValue']).toBe(expected);
    // SHA-256 一定是 64 位 hex；旧的 `hash*31` 只有 8 位。
    expect(String(data['signatureValue'])).toMatch(/^[0-9a-f]{64}$/);
    expect(pkg).toBe(data['package']);
  });

  it('不得再谎报 ed25519：算法名与实际计算一致且如实标 simulated', () => {
    seed(Buffer.from('payload', 'utf8'));
    const result = pluginSign(OPTS);
    const data = dataOf(result);

    expect(data['algorithm']).toBe('sha256-digest');
    expect(data['algorithm']).not.toBe('ed25519');
    expect(data['simulated']).toBe(true);
    expect(result.message).toMatch(/not a signature/);
  });

  it('.sig 文件必须真的落盘，内容与返回值一致', () => {
    seed(Buffer.from('payload-2', 'utf8'));
    const data = dataOf(pluginSign(OPTS));

    const written = JSON.parse(
      readFileSync(join(dir, String(data['signature'])), 'utf8'),
    ) as Record<string, unknown>;
    expect(written['algorithm']).toBe('sha256-digest');
    expect(written['value']).toBe(data['signatureValue']);
    expect(written['pluginId']).toBe('com.example.sign');
  });

  it('包内容改一位 → 摘要必变（辨别力）', () => {
    seed(Buffer.from('aaaa', 'utf8'));
    const first = dataOf(pluginSign(OPTS));
    writeFileSync(join(dir, 'com.example.sign-1.0.0.tgz'), Buffer.from('aaab', 'utf8'));
    const second = dataOf(pluginSign(OPTS));
    expect(second['signatureValue']).not.toBe(first['signatureValue']);
  });
});

/**
 * `pluginPublish` 同样属于"伪造成功"重灾区（轮 11 修正）。
 *
 * 修正前：注释写着 `// Simulate publishing`，返回值却是
 * `success: true` + `Published ... to marketplace` + 一个编造的
 * `registryUrl: 'https://marketplace.tauron.dev'`——**零网络请求**。
 * 脚本会据此认为发布成功并继续往下走，比"未实现"更坏。
 */
describe('pluginPublish（不许伪造上传成功）', () => {
  it('有 manifest 无包：提示先 pack，且不给 registryUrl', () => {
    writeFileSync(
      join(dir, 'tauron.plugin.json'),
      JSON.stringify({ id: 'com.example.sign', name: 'Sign Demo', version: '1.0.0' }),
    );
    const result = pluginPublish(OPTS);
    expect(result.success).toBe(true);
    expect(result.message).toContain('tauron plugin pack');
    expect(dataOf(result)['registryUrl']).toBeNull();
  });

  it('有包时如实报未实现：不得声称已发布、不得编造商城 URL', () => {
    seed(Buffer.from('payload', 'utf8'));
    const result = pluginPublish(OPTS);
    const data = dataOf(result);

    // 核心诚实性：没有上传客户端 → 失败，而不是"成功"。
    expect(result.success).toBe(false);
    expect(result.message).toMatch(/not implemented/);
    expect(result.message).toMatch(/不会伪造/);
    expect(result.message).not.toMatch(/^Published/);

    // 不得编造任何商城地址 / 发布时间。
    expect(data['registryUrl']).toBeNull();
    expect(data['published']).toBe(false);
    expect(data['simulated']).toBe(true);
    expect(data['entry']).not.toHaveProperty('publishedAt');

    // 预览用的登记项仍然给（内容必须来自 manifest，不是编的）。
    const entry = data['entry'] as Record<string, unknown>;
    expect(entry['id']).toBe('com.example.sign');
    expect(entry['package']).toBe('com.example.sign-1.0.0.tgz');
  });
});
