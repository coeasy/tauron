#!/usr/bin/env node
/**
 * tauron CLI — bin entry point
 */

import { runCli } from '../dist/cli.js';

async function main() {
  const args = process.argv.slice(2);
  const options = {
    verbose: args.includes('--verbose') || args.includes('-v'),
    dryRun: args.includes('--dry-run'),
    force: args.includes('--force'),
  };

  // 只过滤掉「本 wrapper 自己消费」的选项，其余参数原样交给 `runCli`。
  //
  // ⚠️ 必须用**白名单**而不是「凡以 `-` 开头就丢」：
  // ① `--help` / `-h` / `--version` 在 `runCli` 的 switch 里有分支，丢掉它们
  //    会让 `tauron --version` 永远回 "No command specified"；
  // ② `--type=wasm` 是 `plugin new` 的参数，丢掉它会让 `--type wasm`
  //    静默退化成 js 插件（此前就是这样）。
  // （`-v` 是 verbose 的短旗，与 version 撞名，按 verbose 处理。）
  const CONSUMED = new Set(['--verbose', '-v', '--dry-run', '--force']);
  const filteredArgs = args.filter((a) => !CONSUMED.has(a));

  const result = await runCli(filteredArgs, options);

  if (options.verbose && result.data) {
    console.log(JSON.stringify(result.data, null, 2));
  }

  console.log(result.message);
  process.exit(result.success ? 0 : 1);
}

main().catch((error) => {
  console.error('Fatal error:', error);
  process.exit(1);
});
