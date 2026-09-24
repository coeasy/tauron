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
export function writeFile(filePath: string, content: string | Buffer): WriteResult {
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
  files: Map<string, string>,
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
export function readJsonFile<T = unknown>(filePath: string): { ok: boolean; data?: T; error?: string } {
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
    const files = entries
      .filter((e) => e.isFile())
      .map((e) => e.name);
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
  files: Map<string, string>,
  outputDir: string,
): BatchWriteResult {
  return writeFiles(files, outputDir);
}
