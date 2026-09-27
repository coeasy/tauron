// ──────────────────────────────────────────────────────────────────────────
// @tauron/app-cli — 文件系统操作（§4.17）。
//
// 设计原则：
// - I/O 与逻辑分离：现有模块（scaffold/plugin/brand/pack）是纯函数，
//   本模块负责将结果写入磁盘 / 从磁盘读取配置。
// - 所有函数返回结构化结果（`{ ok, path?, error? }`），不抛异常。
// - 目录创建幂等（`ensureDir` 可多次调用）。
// - 支持相对路径（相对于 `cwd`）和绝对路径。
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

/** 文件写入结果。 */
export interface WriteResult {
  ok: boolean;
  path?: string;
  error?: string;
}

/** 批量写入结果。 */
export interface BatchWriteResult {
  ok: boolean;
  written: string[];
  failed: { path: string; error: string }[];
}

/** 确保目录存在（递归创建）。 */
export function ensureDir(dirPath: string): WriteResult {
  try {
    const resolved = path.resolve(dirPath);
    if (!fs.existsSync(resolved)) {
      fs.mkdirSync(resolved, { recursive: true });
    }
    return { ok: true, path: resolved };
  } catch (e) {
    return {
      ok: false,
      error: `创建目录失败 "${dirPath}"：${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

/**
 * 写入单个文件（自动创建父目录）。
 */
export function writeFile(filePath: string, content: string | Uint8Array): WriteResult {
  try {
    const resolved = path.resolve(filePath);
    const dir = path.dirname(resolved);
    const dirResult = ensureDir(dir);
    if (!dirResult.ok) {
      return dirResult;
    }
    fs.writeFileSync(resolved, content, typeof content === 'string' ? 'utf-8' : undefined);
    return { ok: true, path: resolved };
  } catch (e) {
    return {
      ok: false,
      error: `写入文件失败 "${filePath}"：${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

/**
 * 批量写入文件（Map<path, content>）。
 *
 * 用法：
 * ```typescript
 * const files = generatePluginFiles(config); // Map<string, string>
 * const result = writeFiles(files, './my-plugin');
 * ```
 */
export function writeFiles(
  files: Map<string, string | Uint8Array>,
  baseDir: string,
): BatchWriteResult {
  const written: string[] = [];
  const failed: { path: string; error: string }[] = [];

  for (const [relativePath, content] of files) {
    const fullPath = path.join(baseDir, relativePath);
    const result = writeFile(fullPath, content);
    if (result.ok) {
      written.push(result.path!);
    } else {
      failed.push({ path: relativePath, error: result.error! });
    }
  }

  return { ok: failed.length === 0, written, failed };
}

/**
 * 读取 JSON 配置文件。
 */
export function readJsonFile<T = unknown>(
  filePath: string,
): { ok: boolean; data?: T; error?: string } {
  try {
    const resolved = path.resolve(filePath);
    const content = fs.readFileSync(resolved, 'utf-8');
    const data = JSON.parse(content) as T;
    return { ok: true, data };
  } catch (e) {
    return {
      ok: false,
      error: `读取 JSON 失败 "${filePath}"：${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

/**
 * 写入 JSON 文件（格式化输出）。
 */
export function writeJsonFile(filePath: string, data: unknown): WriteResult {
  try {
    const json = JSON.stringify(data, null, 2);
    return writeFile(filePath, json);
  } catch (e) {
    return {
      ok: false,
      error: `序列化 JSON 失败：${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

/**
 * 检查路径是否存在。
 */
export function pathExists(filePath: string): boolean {
  return fs.existsSync(path.resolve(filePath));
}

/**
 * 读取目录下的文件列表。
 */
export function listFiles(dirPath: string): { ok: boolean; files?: string[]; error?: string } {
  try {
    const resolved = path.resolve(dirPath);
    if (!fs.existsSync(resolved)) {
      return { ok: false, error: `目录不存在 "${dirPath}"` };
    }
    const entries = fs.readdirSync(resolved, { withFileTypes: true });
    const files = entries.filter((e) => e.isFile()).map((e) => e.name);
    return { ok: true, files };
  } catch (e) {
    return {
      ok: false,
      error: `读取目录失败 "${dirPath}"：${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

/**
 * 复制文件。
 */
export function copyFile(srcPath: string, destPath: string): WriteResult {
  try {
    const src = path.resolve(srcPath);
    const dest = path.resolve(destPath);
    if (!fs.existsSync(src)) {
      return { ok: false, error: `源文件不存在 "${srcPath}"` };
    }
    const dirResult = ensureDir(path.dirname(dest));
    if (!dirResult.ok) {
      return dirResult;
    }
    fs.copyFileSync(src, dest);
    return { ok: true, path: dest };
  } catch (e) {
    return {
      ok: false,
      error: `复制文件失败 "${srcPath}" → "${destPath}"：${e instanceof Error ? e.message : String(e)}`,
    };
  }
}

/**
 * 脚手架写入辅助：将 `scaffold()` / `pluginScaffold()` / `brandBuild()` 的结果写入磁盘。
 *
 * 用法：
 * ```typescript
 * import { pluginScaffold } from '@tauron/app-cli';
 * import { writeScaffoldResult } from '@tauron/app-cli/fs';
 *
 * const result = pluginScaffold(config);
 * const writeResult = writeScaffoldResult(result.files, './output');
 * ```
 */
export function writeScaffoldResult(
  files: Map<string, string | Uint8Array>,
  outputDir: string,
): BatchWriteResult {
  return writeFiles(files, outputDir);
}

// ── tauron 检出根定位 ──

/**
 * 从 `startDir` 起逐级上溯，定位 tauron 源码检出根。
 *
 * 判据是 `crates/tauron-adapter/Cargo.toml` 存在——这是「这个目录就是 tauron
 * 检出根」的最小充分特征（`cargo` 侧所有 path 依赖都从它展开）。
 *
 * 默认从**本模块自身所在目录**起溯，因此无论跑 `src/`（vitest）还是 `dist/`
 * （构建产物）都能找到同一个根。
 *
 * 返回 `null` 表示当前不在 tauron 检出树内——调用方必须据此**如实失败**，
 * 而不是写下一份解析不了的依赖坐标。
 */
export function findTauronRoot(startDir?: string): string | null {
  let dir = path.resolve(startDir ?? path.dirname(fileURLToPath(import.meta.url)));
  for (;;) {
    if (fs.existsSync(path.join(dir, 'crates', 'tauron-adapter', 'Cargo.toml'))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/**
 * 计算 `fromDir` → `toPath` 的相对路径，并统一成 POSIX 分隔符。
 *
 * `Cargo.toml` 的 `path =` 与 npm 的 `file:` 都按字面拼接，Windows 的反斜杠
 * 会让它们（以及被提交进版本库的配置）不可移植，所以这里强制 `/`。
 *
 * **跨盘符回落**：Windows 上 `path.relative` 在两盘之间**给不出**相对路径（会返回
 * 目标盘的绝对路径）。此时只能回落成 POSIX 形式的绝对路径——Cargo 的 path 依赖与
 * npm 的 `file:` 都接受绝对路径，总好过产出一份拼错的相对路径。
 */
export function toPosixRelative(fromDir: string, toPath: string): string {
  const resolvedTarget = path.resolve(toPath);
  const rel = path.relative(path.resolve(fromDir), resolvedTarget);
  const base = path.isAbsolute(rel) ? resolvedTarget : rel;
  return base.split(path.sep).join('/');
}
