/**
 * 把生成器产出的「内存文件映射」真正写到盘上。
 *
 * 为什么单独一个模块：`scaffold.ts` / `plugin.ts` 是**纯生成器**——只拼内容、
 * 不碰文件系统，因此它们的单测可以完全脱离磁盘。落盘策略（目录冲突、
 * `--dry-run`、`--force`）集中在这里，只有一处需要评审。
 *
 * 背景（轮 13 修正）：此前 `create` / `plugin new` 只调生成器就返回
 * `Created app 'x' with N files`，而生成器**从不写盘**——命令是纯谎报，
 * 与轮 12 修掉的 `plugin dev/test/pack` 属同一类缺陷。
 */

import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';

/** 没落盘时的原因；真的写了盘则为 `null`。 */
export type WriteBlockReason = 'dry-run' | 'exists' | null;

export interface WriteOutcome {
  /** 是否真的写了盘 */
  wrote: boolean;
  /** 目标目录（绝对路径） */
  targetDir: string;
  /** 已写入的相对路径（按字典序） */
  written: string[];
  /** 未落盘的原因 */
  blockedBy: WriteBlockReason;
}

/**
 * 写入生成的文件。
 *
 * @param files   相对路径 → 文件内容
 * @param dirName 目标目录名（相对 `cwd`）
 * @param options `dryRun` 只报告不写；`force` 是覆盖已存在目录的**显式授权**；
 *                `cwd` 缺省为 `process.cwd()`
 */
export function writeGeneratedFiles(
  files: Record<string, string>,
  dirName: string,
  options: { dryRun: boolean; force: boolean; cwd?: string },
): WriteOutcome {
  const targetDir = resolve(options.cwd ?? process.cwd(), dirName);

  if (options.dryRun) {
    return { wrote: false, targetDir, written: [], blockedBy: 'dry-run' };
  }

  // 已存在的目录默认不覆盖：脚手架覆盖用户已有工程是不可逆的破坏。
  // `--force` 就是这条规则要求的显式授权。
  if (existsSync(targetDir) && !options.force) {
    return { wrote: false, targetDir, written: [], blockedBy: 'exists' };
  }

  const entries = Object.entries(files).sort(([a], [b]) => a.localeCompare(b));
  const written: string[] = [];
  for (const [rel, content] of entries) {
    const full = join(targetDir, rel);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, content, 'utf8');
    written.push(rel);
  }

  return { wrote: true, targetDir, written, blockedBy: null };
}
