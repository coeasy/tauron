/**
 * Plugin lifecycle commands（设计文档 §7）
 *
 * dev/test/pack/sign/publish 命令实现。
 */

import { existsSync, readFileSync, writeFileSync, readdirSync, statSync } from 'fs';
import { createHash } from 'crypto';
import { join, resolve } from 'path';
import type { CliOptions } from './types.js';

/**
 * Plugin dev command - start dev server with watch mode
 */
/**
 * Plugin dev command - **未实现，而且现在会如实说出来**。
 *
 * 轮 12 修正：此前这里返回 `success: true` + "Dev server started for plugin 'x'
 * (watch mode)" + `{ watch: true, port: 8080 }`，但**没有启动任何服务、没有任何
 * watcher、8080 也是编的**——调用方（很可能是一个刚写完插件的开发者）会据此以为
 * 有个开发服务器在跑，然后花时间排查"为什么浏览器连不上"。
 *
 * `port` 这种字段尤其危险：它是**看起来最像真的**的那种编造。
 */
export function pluginDev(options: CliOptions): CliResult {
  const cwd = process.cwd();
  const manifestPath = join(cwd, 'tauron.plugin.json');

  if (!existsSync(manifestPath)) {
    return {
      success: false,
      message: 'Plugin dev: no tauron.plugin.json found. Run "tauron plugin new" to create a plugin first.',
      data: { ready: false, simulated: true, reason: '未找到插件清单，且本 CLI 没有 dev server' },
    };
  }

  const manifest = JSON.parse(readFileSync(manifestPath, 'utf-8'));
  return {
    success: false,
    message:
      '未实现：本 CLI 不启动 dev server、不监听端口、不做文件 watch。' +
      `已读到插件清单 '${manifest.name}'，但没有任何服务被启动——` +
      '请在宿主应用里加载该插件目录进行调试。',
    data: {
      pluginId: manifest.id,
      name: manifest.name,
      type: manifest.type,
      entry: manifest.entry,
      ready: false,
      simulated: true,
      reason: 'dev server 未实现：无 HTTP 服务、无 watcher、无热重载；本命令只读清单',
    },
  };
}

/**
 * Plugin test command - **未实现，而且不再编造测试结果**。
 *
 * 轮 12 修正：此前这里返回 `success: true` + "Running tests..." +
 * `{ passed: testFiles.length, failed: 0 }`——它**从不执行任何测试**，却把
 * "测试文件个数"当成"通过的用例数"上报。这比"未实现"更坏：CI 会因此变绿，
 * 而绿色意味着"有人验过了"。
 *
 * 现在：如实报 `executed: false`，并**不给出** `passed`/`failed` 字段——
 * 不存在的执行结果不该有字段可填。
 */
export function pluginTest(options: CliOptions): CliResult {
  const cwd = process.cwd();
  const testDir = join(cwd, 'test');

  if (!existsSync(testDir)) {
    return {
      success: false,
      message: 'Plugin test: no test directory found. Run "tauron plugin new" to create tests.',
      data: { testFiles: [], testDir, executed: false, simulated: true, reason: '没有测试目录，且本 CLI 没有测试运行器' },
    };
  }

  const testFiles = readdirSync(testDir).filter((f) => f.endsWith('.test.ts') || f.endsWith('.test.js'));

  return {
    success: false,
    message:
      `未实现：本 CLI 不执行插件测试（没有测试运行器）。已发现 ${testFiles.length} 个测试文件——` +
      '请直接用 vitest/jest 运行它们；本命令不会给出任何通过/失败计数。',
    data: {
      testFiles,
      testDir,
      executed: false,
      simulated: true,
      reason: '测试执行器未实现：只做文件发现，不运行、不汇总结果',
    },
  };
}

/**
 * Plugin pack command - **未实现：不产出任何归档文件**。
 *
 * 轮 12 修正：此前这里返回 `success: true` + "Packed plugin 'x' into y.tgz
 * (N files)"，但**一个字节都没写**（`collectFilesForPack` 只列清单、
 * `calculateSize` 只求和）。调用方会去找那个 `.tgz`，然后发现根本不存在。
 *
 * 现在：如实报 `success: false` + `package: null`，把清单当**计划**给出来
 * （`files`/`size` 是"将要打包的内容"，不是"已打包的内容"）。
 *
 * 真实现（后续项，不在本轮）：用内置 `node:zlib` 写 ustar + gzip 是可行的
 * （无需新依赖），但要处理好 >100 字节路径的 `prefix` 字段与长名回退——
 * 半成品归档比"未实现"更坏（它会带着错误的字节流被签名、被分发）。
 */
export function pluginPack(options: CliOptions): CliResult {
  const cwd = process.cwd();
  const manifestPath = join(cwd, 'tauron.plugin.json');

  if (!existsSync(manifestPath)) {
    return {
      success: false,
      message: 'Plugin pack: no tauron.plugin.json found. Run "tauron plugin new" to create a plugin first.',
      data: { package: null, files: [], size: 0, simulated: true },
    };
  }

  const manifest = JSON.parse(readFileSync(manifestPath, 'utf-8'));
  const packageName = `${manifest.id}-${manifest.version}.tgz`;

  // 只**列**要打包的文件（不写归档）。
  const files = collectFilesForPack(cwd, manifest);

  return {
    success: false,
    message:
      `未实现：本 CLI 不生成归档文件（没有 tar/gzip 写入器），因此没有产出 ${packageName}。` +
      `下面是**将要**打包的内容清单（${files.length} 个文件），供你在自己的流水线里打包。`,
    data: {
      package: null,
      plannedPackage: packageName,
      files,
      size: calculateSize(files, cwd),
      manifest,
      simulated: true,
      reason: '归档写入器未实现：不产出 .tgz、不写任何文件',
    },
  };
}

/**
 * Plugin sign command - sign plugin package.
 *
 * **诚实实现（轮 11 修正）**：此前这里用 `hash * 31` 的手写摘要，却把结果标成
 * `algorithm: 'ed25519'`，并且消息声称生成了 `.sig` 文件而代码**从不写文件**——
 * 两处都是对调用方的不实陈述。
 *
 * 现在：算**真实 SHA-256 摘要**（`node:crypto`，零新依赖），线形里如实标注
 * `algorithm: 'sha256-digest'` 与 `simulated: true`，并把 `.sig` **真的写出来**。
 * 非对称签名（Ed25519）不在本包内实现——需要真签名请用
 * `tauron-app plugin sign`（走 `@tauron/market` 的 Ed25519，同生态消费）。
 */
export function pluginSign(options: CliOptions): CliResult {
  const cwd = process.cwd();
  const manifestPath = join(cwd, 'tauron.plugin.json');

  if (!existsSync(manifestPath)) {
    return {
      success: false,
      message: 'Plugin sign: no tauron.plugin.json found. Run "tauron plugin new" to create a plugin first.',
      data: { package: null, signature: null, simulated: true },
    };
  }

  const manifest = JSON.parse(readFileSync(manifestPath, 'utf-8'));
  const packageName = `${manifest.id}-${manifest.version}.tgz`;
  const packagePath = join(cwd, packageName);

  if (!existsSync(packagePath)) {
    return {
      success: false,
      message:
        `Plugin sign: package '${packageName}' not found. 注意 "tauron plugin pack" 目前也**不产出**归档` +
        '（归档写入器未实现）——请用你自己的流水线打包，再对产物跑本命令。',
      data: { package: null, signature: null, simulated: true },
    };
  }

  const packageContent = readFileSync(packagePath);
  // 真实内容摘要（确定性：同一包 → 同一摘要；改一个字节 → 摘要必变）。
  const digest = contentDigest(packageContent);
  const signatureFile = `${packageName}.sig`;
  const payload = {
    algorithm: 'sha256-digest',
    value: digest,
    simulated: true,
    note: '内容摘要，不是非对称签名；真签名请用 tauron-app plugin sign（@tauron/market Ed25519）',
    package: packageName,
    pluginId: manifest.id,
    version: manifest.version,
  };
  writeFileSync(join(cwd, signatureFile), JSON.stringify(payload, null, 2) + '\n');

  return {
    success: true,
    message: `Wrote content digest for '${manifest.name}' to ${signatureFile} (sha256-digest, simulated: true; not a signature)`,
    data: {
      package: packageName,
      signature: signatureFile,
      algorithm: 'sha256-digest',
      signatureValue: digest,
      simulated: true,
    },
  };
}

/**
 * Plugin publish command - publish plugin to marketplace.
 *
 * **诚实实现（轮 11 修正）**：此前这里注释写着 `// Simulate publishing`，却向调用方
 * 返回 `success: true` + `Published plugin 'X' v1.0.0 to marketplace` + 一个
 * **编造的** `registryUrl: 'https://marketplace.tauron.dev'`——**一个网络请求都没发**。
 * 这是比"未实现"更坏的形态：脚本会据此认为发布成功并继续走下去。
 *
 * 现在：本仓库**没有商城上传客户端**，因此如实返回 `success: false` +
 * `published: false`，`registryUrl` 恒为 `null`；仍会算出即将上传的登记项
 * （`entry`）供调用方预览，但明确标 `simulated: true`，不声称任何东西已被上传。
 *
 * 真发布路径：由 `@tauron/market` 的客户端能力 + 宿主 `host_market_*` 命令承担
 * （当前适配层那三条也是 simulated 桩，见 `docs/architecture/app-layer-wire.md` §3）。
 */
export function pluginPublish(options: CliOptions): CliResult {
  const cwd = process.cwd();
  const manifestPath = join(cwd, 'tauron.plugin.json');

  if (!existsSync(manifestPath)) {
    return {
      success: true,
      message: 'Plugin publish: no tauron.plugin.json found. Run "tauron plugin new" to create a plugin first.',
      data: { registryUrl: null, entry: null },
    };
  }

  const manifest = JSON.parse(readFileSync(manifestPath, 'utf-8'));
  const packageName = `${manifest.id}-${manifest.version}.tgz`;
  const packagePath = join(cwd, packageName);

  if (!existsSync(packagePath)) {
    return {
      success: true,
      message: `Plugin publish: package '${packageName}' not found. Run "tauron plugin pack" first.`,
      data: { registryUrl: null, entry: null },
    };
  }

  // 即将上传的登记项（预览用）。不含"已发布"语义：`publishedAt` 由商城盖章，
  // 客户端算出来的时间戳不算数，故这里也不放。
  const registryEntry = {
    id: manifest.id,
    version: manifest.version,
    name: manifest.name,
    author: manifest.author,
    permissions: manifest.permissions,
    package: packageName,
  };

  return {
    success: false,
    message:
      `Plugin publish: not implemented for '${manifest.name}' v${manifest.version} — ` +
      '本仓库没有商城上传客户端，未发起任何网络请求（不会伪造上传成功）。' +
      `待上传包：${packageName}。`,
    data: {
      registryUrl: null,
      entry: registryEntry,
      package: packageName,
      simulated: true,
      published: false,
    },
  };
}

/**
 * Helper: collect files for packaging
 */
function collectFilesForPack(cwd: string, manifest: any): string[] {
  const files: string[] = ['tauron.plugin.json'];

  // Add entry file
  if (manifest.entry?.main) {
    files.push(manifest.entry.main);
  } else if (manifest.entry) {
    files.push(...Object.values(manifest.entry).map((v: any) => v as string));
  }

  // Add files from includes
  if (manifest.includes) {
    for (const pattern of manifest.includes) {
      const patternFiles = globMatch(cwd, pattern);
      files.push(...patternFiles);
    }
  }

  return files;
}

/**
 * Helper: calculate total size of files
 */
function calculateSize(files: string[], cwd: string): number {
  let totalSize = 0;
  for (const file of files) {
    try {
      const fullPath = join(cwd, file);
      const stats = statSync(fullPath);
      totalSize += stats.size;
    } catch {
      // File might not exist
    }
  }
  return totalSize;
}

/**
 * Helper: 真实 SHA-256 摘要（hex）。**不是签名**——非对称签名见
 * {@link pluginSign} 的文档说明（本包不做密码学签名，也不谎报算法名）。
 */
function contentDigest(content: Buffer): string {
  return createHash('sha256').update(content).digest('hex');
}

/**
 * Helper: simple glob matching
 */
function globMatch(cwd: string, pattern: string): string[] {
  const results: string[] = [];
  const parts = pattern.split('*');

  if (parts.length === 1) {
    const fullPath = join(cwd, pattern);
    if (existsSync(fullPath)) {
      results.push(pattern);
    }
  } else {
    // Simple glob support for *.ext patterns
    const dir = pattern.includes('/')
      ? join(cwd, pattern.split('/').slice(0, -1).join('/'))
      : cwd;
    if (existsSync(dir)) {
      const ext = pattern.split('.').pop() || '';
      const files = readdirSync(dir).filter((f) => f.endsWith(`.${ext}`));
      for (const f of files) {
        // Return path relative to cwd
        const fullPath = join(dir, f);
        const relPath = fullPath.startsWith(cwd)
          ? fullPath.slice(cwd.length).replace(/^[\\/]/, '')
          : f;
        results.push(relPath);
      }
    }
  }

  return results;
}

export interface CliResult {
  success: boolean;
  message: string;
  data?: unknown;
}
