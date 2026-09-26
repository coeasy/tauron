#!/usr/bin/env node
// @tauron/app-cli — CLI 可执行入口。
//
// 零依赖参数解析，支持子命令路由。
// 用法：tauron-app <command> [options]

import * as fs from 'node:fs';
import * as path from 'node:path';

import { generateClientConfig } from './client-config.js';
import { pluginScaffold, validatePluginConfig, validatePluginType } from './plugin.js';
import { themeGenerate, builtinLightTheme, builtinDarkTheme } from './theme.js';
import {
  collectPluginFiles,
  createPluginArchive,
  readPluginArchive,
  pluginPack,
  pluginSign,
  pluginPublish,
  validateFiles,
  validatePackConfig,
} from './pack.js';
import {
  ensureDir, writeFile, writeFiles, readJsonFile, writeJsonFile, pathExists,
} from './fs-operations.js';
import { initProject } from './init.js';
import { doctor } from './doctor.js';
import { createTemplate } from './template.js';
import type { PackConfig, PluginFileInfo, ResolvedPackConfig } from './pack.js';
import type { PluginConfig } from './plugin.js';

// ── 命令注册表 ──

type CommandHandler = (args: string[], options: Record<string, string | boolean>) => void | Promise<void>;

interface CommandDef {
  name: string;
  description: string;
  usage: string;
  handler: CommandHandler;
}

// ── 清单文件结构（仅声明 CLI 需要的字段） ──

/** 插件 `manifest.json`（见 plugin.ts 的 generatePluginManifest）。 */
interface PluginManifestFile {
  id?: string;
  name?: string;
  description?: string;
  version?: string;
  /** 插件类型（manifest 中字段名为 `type`）。 */
  type?: string;
  permissions?: string[];
  framework?: string;
  entry?: { js?: string; sidecar?: string; wasm?: string };
  platforms?: string[];
}

/** 打包清单（见 pack.ts 的 generatePackManifest）。 */
interface PackManifestFile {
  pluginId?: string;
  pluginVersion?: string;
  files?: PluginFileInfo[];
}

// ── 参数解析 ──

function parseArgs(argv: string[]): { command: string; args: string[]; options: Record<string, string | boolean> } {
  const raw = argv.slice(2); // skip node + script
  const positionals: string[] = [];
  const options: Record<string, string | boolean> = {};

  for (let i = 0; i < raw.length; i++) {
    const arg = raw[i];
    if (arg === undefined) continue;
    if (arg.startsWith('--')) {
      // 支持 --key=value 和 --key value 两种语法
      const body = arg.slice(2);
      const eqIdx = body.indexOf('=');
      if (eqIdx !== -1) {
        const key = body.slice(0, eqIdx);
        const value = body.slice(eqIdx + 1);
        options[key] = value;
      } else {
        const key = body;
        const next = raw[i + 1];
        if (next !== undefined && !next.startsWith('--') && next !== '') {
          options[key] = next;
          i++;
        } else {
          options[key] = true;
        }
      }
    } else if (arg.startsWith('-') && arg.length === 2) {
      const key = arg.slice(1);
      const next = raw[i + 1];
      if (next !== undefined && !next.startsWith('-') && next !== '') {
        options[key] = next;
        i++;
      } else {
        options[key] = true;
      }
    } else {
      positionals.push(arg);
    }
  }

  const [command = 'help', ...args] = positionals;
  return { command, args, options };
}

// ── 插件清单辅助 ──

/** 读取插件目录的 manifest.json 并构造经过校验的插件配置。 */
function loadPluginConfig(
  dir: string,
): { ok: true; config: PluginConfig } | { ok: false; error: string } {
  const manifestPath = path.join(dir, 'manifest.json');
  if (!pathExists(manifestPath)) {
    return { ok: false, error: `未找到插件清单：${manifestPath}` };
  }

  const manifest = readJsonFile<PluginManifestFile>(manifestPath);
  if (!manifest.ok || manifest.data === undefined) {
    return { ok: false, error: manifest.error ?? `插件清单读取失败：${manifestPath}` };
  }

  const raw = manifest.data;
  try {
    const config = validatePluginConfig({
      ...(raw.id !== undefined ? { id: raw.id } : {}),
      ...(raw.name !== undefined ? { name: raw.name } : {}),
      ...(raw.description !== undefined ? { description: raw.description } : {}),
      ...(raw.type !== undefined ? { pluginType: validatePluginType(raw.type) } : {}),
      ...(raw.version !== undefined ? { version: raw.version } : {}),
      ...(raw.permissions !== undefined ? { permissions: raw.permissions } : {}),
    });
    return { ok: true, config };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

/** 读取打包清单中的文件列表。 */
function loadPackFiles(
  file: string,
): { ok: true; files: PluginFileInfo[] } | { ok: false; error: string } {
  if (!pathExists(file)) {
    return { ok: false, error: `插件安装包或打包清单不存在：${file}` };
  }
  if (file.toLowerCase().endsWith('.tpkg')) {
    try {
      return { ok: true, files: readPluginArchive(fs.readFileSync(path.resolve(file))) };
    } catch (err) {
      return { ok: false, error: `插件安装包校验失败：${err instanceof Error ? err.message : String(err)}` };
    }
  }
  const manifest = readJsonFile<PackManifestFile>(file);
  if (!manifest.ok || manifest.data === undefined) {
    return { ok: false, error: manifest.error ?? `打包清单读取失败：${file}` };
  }
  const files = manifest.data.files;
  if (files === undefined || files.length === 0) {
    return { ok: false, error: `打包清单缺少 files 字段：${file}` };
  }
  try {
    return { ok: true, files: validateFiles(files) };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

// ── 输出辅助 ──

function printSuccess(msg: string): void {
  console.log(`✓ ${msg}`);
}

function printError(msg: string): void {
  console.error(`✗ ${msg}`);
}

function printInfo(msg: string): void {
  console.log(`ℹ ${msg}`);
}

function printWarn(msg: string): void {
  console.warn(`⚠ ${msg}`);
}

// ── 命令处理器 ──

const HELP_COMMANDS: CommandDef[] = [
  {
    name: 'init',
    description: '一键接入现有 Tauri 项目',
    usage: 'tauron-app init [--config <file>] [--dry-run]',
    handler: async (_args, options) => {
      const result = await initProject({
        configPath: (options.config as string) ?? 'client-config.json',
        dryRun: options['dry-run'] === true,
      });
      if (result.ok) {
        printSuccess('项目已接入 tauron');
        if (result.steps) {
          for (const step of result.steps) {
            printInfo(step);
          }
        }
      } else {
        printError(result.error ?? '接入失败');
      }
    },
  },
  {
    name: 'doctor',
    description: '诊断集成健康度',
    usage: 'tauron-app doctor [--dir <path>]',
    handler: async (_args, options) => {
      const dir = (options.dir as string) ?? '.';
      const result = await doctor(dir);
      console.log(JSON.stringify(result, null, 2));
      if (!result.ok) process.exitCode = 1;
    },
  },
  {
    name: 'client',
    description: '客户端配置管理',
    usage: 'tauron-app client config [--preset full|minimal|template] [--output <file>]',
    handler: async (args, options) => {
      if (args[0] !== 'config') {
        printError('未知子命令：tauron-app client <子命令>。可用：config');
        process.exitCode = 1;
        return;
      }
      const rawPreset = (options.preset as string) ?? 'full';
      if (rawPreset !== 'full' && rawPreset !== 'minimal' && rawPreset !== 'template') {
        printError(`未知预设 "${rawPreset}"，可选：full、minimal、template`);
        process.exitCode = 1;
        return;
      }
      const output = (options.output as string) ?? 'client-config.json';
      const result = generateClientConfig({ preset: rawPreset, outputPath: output });
      if (result.errors.length > 0) {
        for (const e of result.errors) {
          printError(e);
        }
        process.exitCode = 1;
        return;
      }
      const written = writeFile(result.outputPath, result.content);
      if (!written.ok) {
        printError(written.error ?? `配置写入失败：${result.outputPath}`);
        process.exitCode = 1;
        return;
      }
      printSuccess(`配置已生成：${result.outputPath}`);
    },
  },
  {
    name: 'theme',
    description: '主题管理',
    usage: 'tauron-app theme generate [--output <file>] [--css]',
    handler: async (_args, options) => {
      const output = (options.output as string) ?? 'themes.json';
      const withCss = options.css === true;
      const result = themeGenerate({
        outputPath: output,
        generateCss: withCss,
        themes: [builtinLightTheme(), builtinDarkTheme()],
      });
      if (result.ok) {
        printSuccess(`主题已生成：${result.files?.join(', ')}`);
      } else {
        printError(result.error ?? '主题生成失败');
      }
    },
  },
  {
    name: 'plugin',
    description: '插件管理',
    usage: 'tauron-app plugin new|create|dev|pack|sign|publish|check|audit',
    handler: async (args, options) => {
      const sub = args[0];
      switch (sub) {
        case 'new': {
          const id = (options.id as string) ?? '';
          const name = (options.name as string) ?? id;
          const type = (options.type as string) ?? 'js';
          if (!id) {
            printError('缺少 --id 参数');
            process.exitCode = 1;
            return;
          }
          const result = pluginScaffold({
            id,
            name,
            pluginType: type,
            version: '0.1.0',
            permissions: [],
            enabled: true,
          });
          if (result.ok) {
            const dir = options.dir as string | undefined;
            if (dir) {
              const baseDir = path.resolve(dir);
              await ensureDir(baseDir);
              for (const [filePath, content] of result.files) {
                await writeFiles(new Map([[filePath, content]]), baseDir);
              }
              printSuccess(`插件已创建：${baseDir}`);
            } else {
              console.log(JSON.stringify(result, null, 2));
            }
          } else {
            printError(result.error ?? '插件创建失败');
          }
          break;
        }
        case 'create': {
          const type = (options.type as string) ?? 'js';
          const name = (options.name as string) ?? 'my-plugin';
          const dir = options.dir as string | undefined;
          const dryRun = options['dry-run'] === true;
          
          try {
            const result = await createTemplate(type, name, {
              ...(dir !== undefined ? { baseDir: dir } : {}),
              overwrite: options.overwrite === true,
              dryRun,
            });
            
            if (result.ok) {
              printSuccess(`插件模板已创建：${result.path}`);
              if (result.files && result.files.length > 0) {
                printInfo(`文件数量：${result.files.length}`);
              }
            } else {
              printError(result.error ?? '插件模板创建失败');
              process.exitCode = 1;
            }
          } catch (err) {
            printError(err instanceof Error ? err.message : String(err));
            process.exitCode = 1;
          }
          break;
        }
        case 'pack': {
          const dir = (options.dir as string) ?? '.';
          const output = (options.output as string) ?? `${dir}.tpkg`;

          const pluginConfig = loadPluginConfig(dir);
          if (!pluginConfig.ok) {
            printError(pluginConfig.error);
            process.exitCode = 1;
            return;
          }

          const scanned = collectPluginFiles(dir, [output]);
          if (!scanned.ok) {
            printError(scanned.error ?? `扫描插件目录失败：${dir}`);
            process.exitCode = 1;
            return;
          }
          const packFiles = scanned.files.filter((file) => file.path !== 'manifest.json');
          const sourceManifest = readJsonFile<Record<string, unknown>>(path.join(dir, 'manifest.json'));
          if (!sourceManifest.ok || sourceManifest.data === undefined) {
            printError(sourceManifest.error ?? '插件 manifest 读取失败');
            process.exitCode = 1;
            return;
          }

          const config: PackConfig = {
            dir,
            outputPath: output,
            includeSource: options['include-source'] === true,
            compress: options['compress'] === true,
            files: packFiles,
            pluginConfig: pluginConfig.config,
            pluginManifest: sourceManifest.data,
          };

          let resolved: ResolvedPackConfig;
          try {
            resolved = validatePackConfig(config);
          } catch (err) {
            printError(err instanceof Error ? err.message : String(err));
            process.exitCode = 1;
            return;
          }

          const result = pluginPack(resolved);
          if (!result.ok) {
            printError(result.error ?? '打包失败');
            process.exitCode = 1;
            return;
          }

          let archive: Buffer;
          try {
            archive = createPluginArchive(resolved);
          } catch (err) {
            printError(err instanceof Error ? err.message : String(err));
            process.exitCode = 1;
            return;
          }
          const written = writeFile(output, archive);
          if (!written.ok) {
            printError(written.error ?? `写入插件安装包失败：${output}`);
            process.exitCode = 1;
            return;
          }

          printSuccess(`插件安装包已生成：${output}`);
          break;
        }
        case 'sign': {
          const file = (options.file as string) ?? '';
          const key = (options.key as string) ?? '';
          if (!file || !key) {
            printError('缺少 --file 和 --key 参数');
            process.exitCode = 1;
            return;
          }

          const packManifest = loadPackFiles(file);
          if (!packManifest.ok) {
            printError(packManifest.error);
            process.exitCode = 1;
            return;
          }

          let privateKey: string;
          try {
            privateKey = fs.readFileSync(path.resolve(key), 'utf-8');
          } catch (err) {
            printError(`私钥读取失败：${err instanceof Error ? err.message : String(err)}`);
            process.exitCode = 1;
            return;
          }

          const result = await pluginSign({
            dir: (options.dir as string) ?? path.dirname(path.resolve(file)),
            algorithm: (options.algorithm as string) ?? 'ed25519',
            kid: (options.kid as string) ?? 'tauron-001',
            privateKey,
            includeSource: options['include-source'] === true,
          }, packManifest.files);

          if (!result.ok || result.signature === undefined) {
            printError(result.error ?? '签名失败');
            process.exitCode = 1;
            return;
          }

          const signaturePath = `${file}.sig`;
          const written = writeJsonFile(signaturePath, result.signature);
          if (!written.ok) {
            printError(written.error ?? `签名写入失败：${signaturePath}`);
            process.exitCode = 1;
            return;
          }

          printSuccess(`签名成功：${signaturePath}`);
          break;
        }
        case 'publish': {
          const file = (options.file as string) ?? '';
          const url = (options.url as string) ?? '';
          const token = (options.token as string) ?? '';
          if (!file || !url || !token) {
            printError('publish 需要 --file <打包清单> --url <registry> --token <auth-token>');
            process.exitCode = 1;
            return;
          }
          // 校验打包清单存在（发布前预检，避免发布残缺包）
          const packManifest = loadPackFiles(file);
          if (!packManifest.ok) {
            printError(packManifest.error);
            process.exitCode = 1;
            return;
          }
          const result = pluginPublish({
            dir: (options.dir as string) ?? path.dirname(path.resolve(file)),
            targetUrl: url,
            token,
            overwrite: options.overwrite === true,
            includeSignature: options['include-signature'] === true,
          });
          if (!result.ok) {
            printError(result.error ?? '发布失败');
            process.exitCode = 1;
            return;
          }
          printSuccess(`发布请求已就绪：${result.url}`);
          printInfo('注意：CLI 仅生成发布端点，实际网络上传由 CI/发布服务完成（无网络依赖设计）。');
          break;
        }
        case 'check': {
          const manifestPath = (options.manifest as string) ?? './manifest.json';
          if (!pathExists(manifestPath)) {
            printError(`manifest 不存在：${manifestPath}`);
            process.exitCode = 1;
            return;
          }
          const manifest = readJsonFile<Record<string, unknown>>(manifestPath);
          if (!manifest.ok || manifest.data === undefined) {
            printError(manifest.error ?? 'manifest 读取失败');
            process.exitCode = 1;
            return;
          }
          // 基本校验
          const m = manifest.data;
          const errors: string[] = [];
          if (!m.id) errors.push('缺少 id');
          if (!m.name) errors.push('缺少 name');
          if (!m.framework) errors.push('缺少 framework');
          if (!m.type) errors.push('缺少 type');
          if (errors.length > 0) {
            for (const e of errors) printError(e);
            process.exitCode = 1;
          } else {
            printSuccess(`manifest 校验通过：${m.id}`);
          }
          break;
        }
        case 'audit': {
          const manifestPath = (options.manifest as string) ?? './manifest.json';
          if (!pathExists(manifestPath)) {
            printError(`manifest 不存在：${manifestPath}`);
            process.exitCode = 1;
            return;
          }
          const manifest = readJsonFile<Record<string, unknown>>(manifestPath);
          if (!manifest.ok || manifest.data === undefined) {
            printError(manifest.error ?? 'manifest 读取失败');
            process.exitCode = 1;
            return;
          }
          const m = manifest.data;
          const rawPermissions = m.permissions;
          const permissions = Array.isArray(rawPermissions)
            ? rawPermissions.filter((p): p is string => typeof p === 'string')
            : [];
          const highRisk = permissions.filter((p) => p.includes(':allow-') || p.includes(':write'));
          const report = {
            pluginId: m.id,
            permissions: permissions.length,
            highRiskCount: highRisk.length,
            highRisk: highRisk,
            hasSignature: !!m.signature,
            hasPublisher: !!m.publisher,
          };
          console.log(JSON.stringify(report, null, 2));
          if (highRisk.length > 0) {
            printWarn(`${highRisk.length} 个高危权限，建议审查`);
          } else {
            printSuccess('审计通过，无高危权限');
          }
          break;
        }
        case 'dev': {
          printInfo('plugin dev 需要 chokidar 依赖，当前版本仅支持 pack/check/audit');
          printWarn('请使用 tauron-app plugin check 校验插件');
          break;
        }
        default:
          printError(`未知子命令：tauron-app plugin ${sub ?? ''}`);
          process.exitCode = 1;
      }
    },
  },
  {
    name: 'codegen',
    description: '类型生成（manifest → d.ts）',
    usage: 'tauron-app codegen [--manifest <path>] [--output <path>]',
    handler: async (_args, options) => {
      const manifestPath = (options.manifest as string) ?? './manifest.json';
      const output = (options.output as string) ?? './types.d.ts';
      if (!pathExists(manifestPath)) {
        printError(`manifest 不存在：${manifestPath}`);
        process.exitCode = 1;
        return;
      }
      const manifest = readJsonFile<Record<string, unknown>>(manifestPath);
      if (!manifest.ok || manifest.data === undefined) {
        printError(manifest.error ?? 'manifest 读取失败');
        process.exitCode = 1;
        return;
      }
      const m = manifest.data;
      const typeDecls: string[] = [];
      typeDecls.push('// 由 tauron-app codegen 自动生成');
      typeDecls.push(`export interface PluginManifest {`);
      for (const [key, value] of Object.entries(m)) {
        const tsType = inferTsType(value);
        typeDecls.push(`  ${key}${typeof value === 'undefined' || value === null ? '?' : ''}: ${tsType};`);
      }
      typeDecls.push('}');
      typeDecls.push('');
      writeJsonFile(output, JSON.stringify(m, null, 2));
      const dtsPath = output.replace(/\.json$/, '.d.ts');
      const dtsContent = typeDecls.join('\n') + '\n';
      const result = writeFile(dtsPath, dtsContent);
      if (result.ok) {
        printSuccess(`类型已生成：${dtsPath}`);
      } else {
        printError(result.error ?? '类型生成失败');
      }
    },
  },
];

function inferTsType(value: unknown): string {
  if (typeof value === 'string') return 'string';
  if (typeof value === 'number') return 'number';
  if (typeof value === 'boolean') return 'boolean';
  if (Array.isArray(value)) return 'unknown[]';
  if (typeof value === 'object' && value !== null) return 'Record<string, unknown>';
  return 'unknown';
}

// ── 主入口 ──

export async function main(argv?: string[]): Promise<void> {
  const parsed = parseArgs(argv ?? process.argv);
  const { command, args, options } = parsed;

  if (command === 'help' || command === '--help' || command === '-h') {
    printUsage();
    return;
  }

  const cmdDef = HELP_COMMANDS.find((c) => c.name === command);
  if (!cmdDef) {
    printError(`未知命令：${command}`);
    printUsage();
    process.exitCode = 1;
    return;
  }

  try {
    await cmdDef.handler(args, options);
  } catch (err) {
    printError(err instanceof Error ? err.message : String(err));
    process.exitCode = 1;
  }
}

function printUsage(): void {
  console.log('tauron — Tauri 客户端插件化框架 CLI');
  console.log('');
  console.log('用法：');
  for (const cmd of HELP_COMMANDS) {
    console.log(`  ${cmd.usage}`);
    console.log(`      ${cmd.description}`);
  }
  console.log('');
  console.log('  tauron-app help    显示此帮助');
}

// 直接运行时执行
if (import.meta.url === `file://${process.argv[1]}` || process.argv[1]?.endsWith('cli.js') || process.argv[1]?.endsWith('cli.mjs')) {
  main().catch((err) => {
    printError(err instanceof Error ? err.message : String(err));
    process.exitCode = 1;
  });
}
