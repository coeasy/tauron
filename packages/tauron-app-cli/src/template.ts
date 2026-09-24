// ──────────────────────────────────────────────────────────────────────────
// @tauron/app-cli — 插件模板（`tauron-app plugin create`）。
//
// 职责：
// - 内置模板目录（js / process / wasm）的元数据
// - 由模板实例化出插件项目文件（写盘或预览）
// - 自定义模板目录的复制
//
// 模板内容复用 `plugin.ts` 的脚手架逻辑，保证与框架包同源发版。
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';

import { pluginScaffold } from './plugin.js';
import { ensureDir, writeFiles } from './fs-operations.js';
import type { PluginConfigInput, PluginScaffoldResult } from './plugin.js';

// ── 类型 ──

/** 模板类型（与插件类型同义）。 */
export type TemplateType = 'js' | 'process' | 'wasm';

/** 支持的模板类型。 */
export const SUPPORTED_TEMPLATE_TYPES: readonly TemplateType[] = ['js', 'process', 'wasm'];

/** 模板实例化选项。 */
export interface TemplateOptions {
  /** 输出根目录（默认当前工作目录）；模板会生成到 `<baseDir>/<name>`。 */
  baseDir?: string;
  /** 目标目录已存在且非空时是否覆盖。 */
  overwrite?: boolean;
  /** 仅预览（不写盘）。 */
  dryRun?: boolean;
  /** 插件版本（默认 `0.1.0`）。 */
  version?: string;
  /** 插件描述。 */
  description?: string;
  /** 权限列表（默认空）。 */
  permissions?: string[];
}

/** 模板内的单个文件。 */
export interface TemplateFileInfo {
  /** 相对路径（`/` 分隔）。 */
  path: string;
  /** 字节数。 */
  size: number;
}

/** 模板元数据。 */
export interface TemplateInfo {
  /** 模板类型。 */
  type: TemplateType;
  /** 展示名。 */
  name: string;
  /** 说明。 */
  description: string;
  /** 生成的文件数。 */
  fileCount: number;
}

/** 模板实例化结果。 */
export type TemplateResult =
  | {
      ok: true;
      /** 目标目录（绝对路径）。 */
      path: string;
      /** 生成的文件（绝对路径）。 */
      files: string[];
    }
  | {
      ok: false;
      /** 失败原因。 */
      error: string;
    };

// ── 常量 ──

/** 模板默认版本（与框架包同源发版）。 */
const DEFAULT_TEMPLATE_VERSION = '0.1.0';

/** 模板展示元数据。 */
const TEMPLATE_CATALOG: Record<TemplateType, { name: string; description: string }> = {
  js: {
    name: 'JavaScript 插件',
    description: '通过 @tauron/app-plugin-sdk 注册生命周期钩子、命令与事件',
  },
  process: {
    name: '进程插件',
    description: '独立进程通过 stdin/stdout 与宿主通信',
  },
  wasm: {
    name: 'WASM 插件',
    description: '编译为 WebAssembly 后在宿主沙箱内运行',
  },
};

// ── 验证 ──

/**
 * 验证模板名称。
 *
 * 规则：非空、长度 ≤ 100、不包含路径分隔符或 `..`（名称会用于派生目录名）。
 */
export function validateTemplateName(name: string): string {
  const trimmed = name.trim();
  if (trimmed === '') {
    throw new Error('模板名称不能为空');
  }
  if (trimmed.length > 100) {
    throw new Error(`模板名称长度 ${trimmed.length} 超过 100`);
  }
  if (trimmed.includes('/') || trimmed.includes('\\') || trimmed.includes('..')) {
    throw new Error(`模板名称不能包含路径分隔符或 ..：${name}`);
  }
  return trimmed;
}

/** 验证模板类型。 */
function validateTemplateType(type: string): TemplateType {
  const match = SUPPORTED_TEMPLATE_TYPES.find((supported) => supported === type);
  if (match === undefined) {
    throw new Error(
      `不支持的模板类型 "${type}"，可选：${SUPPORTED_TEMPLATE_TYPES.join(', ')}`,
    );
  }
  return match;
}

// ── 模板目录 ──

/**
 * 列出内置模板。
 */
export function listTemplates(): TemplateInfo[] {
  return SUPPORTED_TEMPLATE_TYPES.map((type) => {
    const meta = TEMPLATE_CATALOG[type];
    return {
      type,
      name: meta.name,
      description: meta.description,
      fileCount: listTemplateFiles(type).length,
    };
  });
}

/**
 * 查询单个模板的元数据；模板类型不支持时返回 `null`。
 */
export function getTemplateInfo(type: string): TemplateInfo | null {
  return listTemplates().find((t) => t.type === type) ?? null;
}

/**
 * 列出模板将生成的文件（在内存中生成，不写盘）。
 */
export function listTemplateFiles(type: TemplateType): TemplateFileInfo[] {
  const result = generatePluginScaffold(type, 'template-preview');
  if (!result.ok) {
    return [];
  }
  return Array.from(result.files, ([filePath, content]) => ({
    path: filePath,
    size: Buffer.byteLength(content, 'utf-8'),
  }));
}

// ── 生成 ──

/**
 * 依据模板类型生成插件文件（委托 `plugin.ts` 的脚手架逻辑）。
 *
 * 插件 ID 由名称派生：`com.tauron.<slug>`。
 */
export function generatePluginScaffold(
  type: TemplateType,
  name: string,
  options: TemplateOptions = {},
): PluginScaffoldResult {
  const input: PluginConfigInput = {
    id: `com.tauron.${slugify(name)}`,
    name,
    pluginType: type,
    version: options.version ?? DEFAULT_TEMPLATE_VERSION,
    permissions: options.permissions ?? [],
    enabled: true,
    ...(options.description !== undefined ? { description: options.description } : {}),
  };
  return pluginScaffold(input);
}

/**
 * 将模板实例化到 `<baseDir>/<name>`。
 *
 * - `overwrite` 为 `false` 时，目标目录已存在且非空会直接失败（不覆盖用户文件）。
 * - `dryRun` 为 `true` 时只返回将要写入的路径，不写盘。
 */
export async function createTemplate(
  type: string,
  name: string,
  options: TemplateOptions = {},
): Promise<TemplateResult> {
  let templateType: TemplateType;
  let templateName: string;
  try {
    templateType = validateTemplateType(type);
    templateName = validateTemplateName(name);
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }

  const scaffoldResult = generatePluginScaffold(templateType, templateName, options);
  if (!scaffoldResult.ok) {
    return { ok: false, error: scaffoldResult.error ?? '插件模板生成失败' };
  }

  const targetDir = path.resolve(options.baseDir ?? '.', slugify(templateName));
  const overwrite = options.overwrite ?? false;
  const dryRun = options.dryRun ?? false;

  if (!overwrite && fs.existsSync(targetDir) && fs.readdirSync(targetDir).length > 0) {
    return {
      ok: false,
      error: `目标目录已存在且非空：${targetDir}（使用 --overwrite 覆盖）`,
    };
  }

  if (dryRun) {
    return {
      ok: true,
      path: targetDir,
      files: [...scaffoldResult.files.keys()].map((rel) => path.join(targetDir, rel)),
    };
  }

  const ensured = ensureDir(targetDir);
  if (!ensured.ok) {
    return { ok: false, error: ensured.error ?? `创建目录失败：${targetDir}` };
  }

  const written = writeFiles(scaffoldResult.files, targetDir);
  if (!written.ok) {
    const failure = written.failed[0];
    return {
      ok: false,
      error:
        failure !== undefined
          ? `写入文件失败：${failure.path}（${failure.error}）`
          : '写入插件模板失败',
    };
  }

  return { ok: true, path: targetDir, files: written.written };
}

/**
 * 复制自定义模板目录到目标目录。
 *
 * 逐文件复制（而非整目录 cp），以便控制覆盖行为并返回实际写入的路径。
 */
export function copyTemplate(
  sourceDir: string,
  targetDir: string,
  options: Pick<TemplateOptions, 'overwrite' | 'dryRun'> = {},
): TemplateResult {
  const source = path.resolve(sourceDir);
  const target = path.resolve(targetDir);

  if (!fs.existsSync(source)) {
    return { ok: false, error: `模板目录不存在：${sourceDir}` };
  }
  if (!fs.statSync(source).isDirectory()) {
    return { ok: false, error: `模板路径不是目录：${sourceDir}` };
  }

  const overwrite = options.overwrite ?? false;
  const dryRun = options.dryRun ?? false;

  if (!overwrite && fs.existsSync(target) && fs.readdirSync(target).length > 0) {
    return {
      ok: false,
      error: `目标目录已存在且非空：${target}（使用 overwrite 覆盖）`,
    };
  }

  const relativeFiles = listRelativeFiles(source);

  if (!dryRun) {
    const ensured = ensureDir(target);
    if (!ensured.ok) {
      return { ok: false, error: ensured.error ?? `创建目录失败：${target}` };
    }
  }

  const files: string[] = [];
  try {
    for (const rel of relativeFiles) {
      const dest = path.join(target, rel);
      if (!dryRun) {
        const parent = ensureDir(path.dirname(dest));
        if (!parent.ok) {
          return { ok: false, error: parent.error ?? `创建目录失败：${path.dirname(dest)}` };
        }
        fs.copyFileSync(path.join(source, rel), dest);
      }
      files.push(dest);
    }
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }

  return { ok: true, path: target, files };
}

// ── 内部辅助 ──

/** 将名称转换为目录 / ID 片段（小写字母、数字、连字符）。 */
function slugify(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return slug === '' ? 'plugin' : slug;
}

/** 递归列出目录下的文件（相对路径，`/` 分隔）。 */
function listRelativeFiles(root: string): string[] {
  const out: string[] = [];
  const walk = (absDir: string): void => {
    for (const entry of fs.readdirSync(absDir, { withFileTypes: true })) {
      const abs = path.join(absDir, entry.name);
      if (entry.isDirectory()) {
        walk(abs);
        continue;
      }
      if (entry.isFile()) {
        out.push(path.relative(root, abs).split(path.sep).join('/'));
      }
    }
  };
  walk(root);
  return out;
}
