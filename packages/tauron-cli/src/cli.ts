/**
 * tauron CLI — 主入口
 *
 * 命令：
 * - tauron doctor — 环境诊断
 * - tauron create <name> — 创建新应用
 * - tauron plugin new <name> — 创建新插件
 * - tauron plugin dev — 开发模式
 * - tauron plugin test — 测试插件
 * - tauron plugin pack — 打包插件
 * - tauron plugin sign — 签名插件
 * - tauron plugin publish — 发布插件
 */

import { runDoctor, formatDoctorReport } from './doctor.js';
import { createApp } from './scaffold.js';
import { pluginNew } from './plugin.js';
import { pluginDev, pluginTest, pluginPack, pluginSign, pluginPublish } from './plugin-lifecycle.js';
import { writeGeneratedFiles } from './scaffold-writer.js';
import type { AppConfig, PluginConfig, CliOptions, PluginType } from './types.js';

export interface CliResult {
  success: boolean;
  message: string;
  data?: unknown;
}

/**
 * 执行 CLI 命令
 */
export async function runCli(args: string[], options: CliOptions = { verbose: false, dryRun: false, force: false }): Promise<CliResult> {
  if (args.length === 0) {
    return { success: false, message: 'No command specified. Use "tauron --help" for usage.' };
  }

  const command = args[0];

  switch (command) {
    case 'doctor':
      return runDoctorCommand(options);
    case 'create':
      return createCommand(args[1], options);
    case 'plugin':
      return pluginCommand(args[1], args.slice(2), options);
    case 'help':
    case '--help':
    case '-h':
      return { success: true, message: getHelpText() };
    case 'version':
    case '--version':
    case '-v':
      return { success: true, message: 'tauron v0.1.0' };
    default:
      return { success: false, message: `Unknown command: ${command}. Use "tauron --help" for usage.` };
  }
}

/**
 * doctor 命令
 */
function runDoctorCommand(options: CliOptions): CliResult {
  const report = runDoctor();
  return { success: true, message: formatDoctorReport(report), data: report };
}

/**
 * create 命令
 */
function createCommand(name: string | undefined, options: CliOptions): CliResult {
  if (!name) {
    return { success: false, message: 'Usage: tauron create <app-name> [--template vanilla|react|vue|svelte]' };
  }

  const config: AppConfig = {
    name,
    template: 'vanilla',
    pluginTypes: ['js'],
  };

  const result = createApp(config, options);
  return finalizeScaffold('app', name, result.files, result.dirName, options);
}

/**
 * 生成 → 落盘 → 如实汇报。
 *
 * 此前 `create` / `plugin new` 只调生成器就返回
 * `Created app 'x' with N files`——而 `scaffold.ts` / `plugin.ts` **从不碰
 * 文件系统**，所以那条消息是纯谎报（与轮 12 修掉的 `plugin dev/test/pack`
 * 属同一类）。这里把三件事分开：生成器只产内容，落盘归
 * `writeGeneratedFiles`，消息按真实结果分三种。
 */
function finalizeScaffold(
  kind: 'app' | 'plugin',
  name: string,
  files: Record<string, string>,
  dirName: string,
  options: CliOptions,
): CliResult {
  const outcome = writeGeneratedFiles(files, dirName, options);
  const count = Object.keys(files).length;

  if (outcome.blockedBy === 'dry-run') {
    return {
      success: true,
      message: `Would create ${kind} '${name}' with ${count} files in ${outcome.targetDir}（--dry-run：只报告，未写盘）`,
      data: { ...outcome, files },
    };
  }

  if (outcome.blockedBy === 'exists') {
    return {
      success: false,
      message: `${outcome.targetDir} 已存在，未覆盖。确认要覆盖请加 --force。`,
      data: outcome,
    };
  }

  return {
    success: true,
    message: `Created ${kind} '${name}' with ${count} files in ${outcome.targetDir}`,
    data: { ...outcome, files },
  };
}

/**
 * plugin 命令
 */
function pluginCommand(subcommand: string | undefined, args: string[], options: CliOptions): CliResult {
  if (!subcommand) {
    return { success: false, message: 'Usage: tauron plugin <new|dev|test|pack|sign|publish>' };
  }

  switch (subcommand) {
    case 'new':
      return pluginNewCommand(args, options);
    case 'dev':
      return pluginDev(options);
    case 'test':
      return pluginTest(options);
    case 'pack':
      return pluginPack(options);
    case 'sign':
      return pluginSign(options);
    case 'publish':
      return pluginPublish(options);
    default:
      return { success: false, message: `Unknown plugin subcommand: ${subcommand}` };
  }
}

/**
 * 解析 `plugin new` 的 `--type`。
 *
 * 两种写法都要支持（帮助文本写的是 `[--type js|process|wasm]`，即空格形式；
 * 但历史上只实现了 `--type=js` 一种，且参数还被 bin 的过滤器丢掉了，
 * 于是 `--type wasm` 静默产出 js 插件）。非法值**如实失败**而不是悄悄降级。
 */
function parsePluginType(args: string[]): { type: PluginType } | { error: string } {
  const VALID: PluginType[] = ['js', 'process', 'wasm'];

  const eq = args.find((a) => a.startsWith('--type='));
  const spaced = args.indexOf('--type');
  const raw = eq ? eq.slice('--type='.length) : spaced >= 0 ? args[spaced + 1] : undefined;

  if (raw === undefined) return { type: 'js' };
  if (!VALID.includes(raw as PluginType)) {
    return { error: `Unknown plugin type: ${raw}. Use one of: ${VALID.join(' | ')}` };
  }
  return { type: raw as PluginType };
}

/**
 * plugin new 命令
 */
function pluginNewCommand(args: string[], options: CliOptions): CliResult {
  const name = args[0];
  if (!name) {
    return { success: false, message: 'Usage: tauron plugin new <plugin-name> [--type js|process|wasm]' };
  }

  const parsed = parsePluginType(args);
  if ('error' in parsed) {
    return { success: false, message: parsed.error };
  }

  const config: PluginConfig = {
    name,
    type: parsed.type,
    permissions: ['store:read', 'http:fetch'],
  };

  const result = pluginNew(config, options);
  return finalizeScaffold('plugin', name, result.files, result.dirName, options);
}

/**
 * 获取帮助文本
 */
function getHelpText(): string {
  return `tauron v0.1.0 — tauron CLI

Usage:
  tauron <command> [options]

Commands:
  doctor              Environment diagnostics
  create <name>       Create a new tauron app
  plugin new <name>   Create a new plugin
  plugin dev          Start plugin dev server
  plugin test         Run plugin tests
  plugin pack         Pack plugin for distribution
  plugin sign         Sign plugin package (simplified SHA-256 digest; asymmetric signing not wired yet)
  plugin publish      Publish plugin to marketplace

Options:
  --verbose           Verbose output
  --dry-run           Show what would be done without doing it
  --force             Force overwrite existing files
  --help, -h          Show this help
  --version, -v       Show version
`;
}
