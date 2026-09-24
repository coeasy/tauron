// ──────────────────────────────────────────────────────────────────────────
// 插件脚手架逻辑（开发计划 §4.17 `plugin new/dev/pack/sign/publish`）。
//
// 职责：生成插件项目文件、验证插件配置、打包配置。
//
// 关键约束：
// - plugin dev 目录监听不得越界读取
// - 模板与框架包版本同源发版
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 插件类型。 */
export type PluginType = 'js' | 'process' | 'wasm';

/** 插件配置。 */
export interface PluginConfig {
  /** 插件 ID（反域名格式）。 */
  id: string;
  /** 插件名称。 */
  name: string;
  /** 描述。 */
  description?: string;
  /** 插件类型。 */
  pluginType: PluginType;
  /** 版本号。 */
  version: string;
  /** 权限列表。 */
  permissions: string[];
  /** 作者。 */
  author?: string;
  /** 许可证。 */
  license?: string;
  /** 是否启用。 */
  enabled: boolean;
}

/**
 * 插件配置（调用方输入）。
 *
 * 所有字段均为原始值（例如来自 CLI 参数或 JSON），由
 * {@link validatePluginConfig} 校验并收窄为 {@link PluginConfig}。
 */
export interface PluginConfigInput {
  id?: string;
  name?: string;
  description?: string;
  /** 原始类型字符串，校验后收窄为 {@link PluginType}。 */
  pluginType?: string;
  version?: string;
  permissions?: string[];
  author?: string;
  license?: string;
  enabled?: boolean;
}

/** 插件脚手架结果。 */
export interface PluginScaffoldResult {
  /** 生成的文件列表（路径 → 内容）。 */
  files: Map<string, string>;
  /** 是否成功。 */
  ok: boolean;
  /** 错误信息（如果失败）。 */
  error?: string;
}

/** 插件打包配置。 */
export interface PluginPackConfig {
  /** 插件目录路径。 */
  dir: string;
  /** 输出文件路径。 */
  outputPath?: string;
  /** 是否包含源码。 */
  includeSource: boolean;
  /** 是否压缩。 */
  compress: boolean;
}

/** 插件目录监听配置。 */
export interface PluginDevWatchConfig {
  /** 插件目录。 */
  dir: string;
  /** 根目录（越界检查的基准）。 */
  rootDir: string;
  /** 忽略模式。 */
  ignorePatterns: string[];
  /** 最大文件数。 */
  maxFiles: number;
  /** 最大文件大小（字节）。 */
  maxFileSize: number;
}

// ──────────────────────────────────────────────────────────────────────────
// 验证
// ──────────────────────────────────────────────────────────────────────────

/** 支持的插件类型。 */
export const SUPPORTED_PLUGIN_TYPES: readonly PluginType[] = ['js', 'process', 'wasm'];

/** 插件 ID 正则（反域名格式）。 */
const PLUGIN_ID_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/;

/**
 * 验证插件 ID。
 *
 * 规则：
 * - 反域名格式（至少两个点分隔的段）
 * - 每段仅允许小写字母、数字、连字符、点
 * - 不以点或连字符开头或结尾
 * - 长度 3-100
 */
export function validatePluginId(id: string): string {
  if (!id || id.trim() === '') {
    throw new Error('插件 ID 不能为空');
  }
  if (id.length < 3) {
    throw new Error('插件 ID 至少 3 个字符');
  }
  if (id.length > 100) {
    throw new Error('插件 ID 最多 100 个字符');
  }
  if (!PLUGIN_ID_RE.test(id)) {
    throw new Error(
      '插件 ID 必须是反域名格式（如 com.example.plugin），每段仅允许小写字母、数字、连字符',
    );
  }
  return id;
}

/**
 * 验证插件名称。
 */
export function validatePluginName(name: string): string {
  if (!name || name.trim() === '') {
    throw new Error('插件名称不能为空');
  }
  if (name.length > 100) {
    throw new Error('插件名称最多 100 个字符');
  }
  return name;
}

/**
 * 验证版本号。
 *
 * 规则：semver 格式（如 1.0.0）。
 */
export function validateVersion(version: string): string {
  if (!version || version.trim() === '') {
    throw new Error('版本号不能为空');
  }
  if (!/^\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$/.test(version)) {
    throw new Error('版本号必须是 semver 格式（如 1.0.0 或 1.0.0-beta.1）');
  }
  return version;
}

/**
 * 验证插件类型。
 */
export function validatePluginType(type: string): PluginType {
  if (!SUPPORTED_PLUGIN_TYPES.includes(type as PluginType)) {
    throw new Error(
      `不支持的插件类型 "${type}"，可选：${SUPPORTED_PLUGIN_TYPES.join(', ')}`,
    );
  }
  return type as PluginType;
}

/**
 * 验证权限列表。
 *
 * 权限名必须是已知的 Tauri 权限标识。
 */
export function validatePermissions(permissions: string[]): string[] {
  if (!Array.isArray(permissions)) {
    throw new Error('权限列表必须是数组');
  }
  // 检查格式（不能包含非法字符）
  for (const perm of permissions) {
    if (!/^[a-zA-Z][a-zA-Z0-9:.-]*$/.test(perm)) {
      throw new Error(`权限名 "${perm}" 包含非法字符`);
    }
  }
  // 检查重复
  const seen = new Set<string>();
  const duplicates: string[] = [];
  for (const perm of permissions) {
    if (seen.has(perm)) {
      duplicates.push(perm);
    }
    seen.add(perm);
  }
  if (duplicates.length > 0) {
    throw new Error(`重复的权限：${duplicates.join(', ')}`);
  }
  return permissions;
}

/**
 * 完整验证插件配置。
 */
export function validatePluginConfig(config: PluginConfigInput): PluginConfig {
  const id = validatePluginId(config.id ?? '');
  const name = validatePluginName(config.name ?? '');
  const pluginType = validatePluginType(config.pluginType ?? 'js');
  const version = validateVersion(config.version ?? '0.1.0');
  const permissions = validatePermissions(config.permissions ?? []);

  return {
    id,
    name,
    description: config.description ?? '',
    pluginType,
    version,
    permissions,
    enabled: config.enabled ?? true,
    ...(config.author !== undefined ? { author: config.author } : {}),
    ...(config.license !== undefined ? { license: config.license } : {}),
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 文件生成
// ──────────────────────────────────────────────────────────────────────────

/** 框架包版本（与 @tauron/host 同源发版）。 */
const FRAMEWORK_VERSION = '0.1.0';

/**
 * 生成插件 manifest.json 内容。
 */
export function generatePluginManifest(config: PluginConfig): string {
  // 格式必须与 crates/tauron-host/src/manifest.rs 的 PluginManifest 完全匹配。
  // 关键字段：`type`（非 pluginType）、`framework`（非 frameworkVersion）、
  // `entry`（JS/Process/WASM 必填）、`deny_unknown_fields` 拒绝额外字段。
  const manifest: Record<string, unknown> = {
    id: config.id,
    name: config.name,
    version: config.version,
    type: config.pluginType,
    framework: `^${FRAMEWORK_VERSION}`,
  };

  // entry（JS/Process/WASM 必填，Rust 校验）
  switch (config.pluginType) {
    case 'js':
      manifest.entry = { js: 'src/index.js' };
      break;
    case 'process':
      manifest.entry = { sidecar: `bin/${config.id.replace(/[^a-zA-Z0-9]/g, '_')}.exe` };
      break;
    case 'wasm':
      manifest.entry = { wasm: 'dist/plugin.wasm' };
      break;
  }

  if (config.permissions.length > 0) {
    manifest.permissions = config.permissions;
  }

  manifest.platforms = ['win', 'mac', 'linux'];

  return JSON.stringify(manifest, null, 2) + '\n';
}

/**
 * 生成插件 package.json 内容。
 */
export function generatePluginPackageJson(config: PluginConfig): string {
  const pkg = {
    name: config.id,
    version: config.version,
    private: true,
    description: config.description || `${config.name} plugin`,
    type: 'module',
    scripts: {
      dev: 'tauron plugin dev',
      pack: 'tauron plugin pack',
      publish: 'tauron plugin publish',
      test: 'vitest run',
      typecheck: 'tsc --noEmit',
    },
    dependencies: {
      '@tauron/host': `workspace:*`,
      '@tauron/app-plugin-sdk': `workspace:*`,
    },
    devDependencies: {
      // 插件脚本调用 `tauron plugin dev|pack|publish`，
      // 因此必须显式依赖提供 `tauron` bin 的 CLI，否则脚本会 "command not found"。
      '@tauron/cli': `workspace:*`,
      typescript: '^5.8.0',
      vitest: '^3.0.0',
    },
    engines: { node: '>=22' },
  };
  return JSON.stringify(pkg, null, 2) + '\n';
}

/**
 * 生成插件入口文件。
 */
export function generatePluginEntry(config: PluginConfig): string {
  switch (config.pluginType) {
    case 'js':
      return generateJsPluginEntry(config);
    case 'process':
      return generateProcessPluginEntry(config);
    case 'wasm':
      return generateWasmPluginEntry(config);
  }
}

function generateJsPluginEntry(config: PluginConfig): string {
  return `import { createPlugin } from '@tauron/app-plugin-sdk';

// ${config.name} - JS 插件入口
// 使用 createPlugin() 注册生命周期钩子和命令

const plugin = createPlugin({
  id: '${config.id}',
  name: '${config.name}',
  version: '${config.version}',

  async activate(ctx) {
    ctx.log('Plugin activated: ${config.name}');

    // 注册命令
    ctx.commands.register('hello', async (args) => {
      return { message: 'Hello from ${config.name}!', args };
    });

    // 注册设置 Tab
    ctx.settings.registerTab({
      id: 'main',
      title: '${config.name} 设置',
      schema: {
        type: 'object',
        properties: {
          greeting: { type: 'string', default: 'World' },
        },
      },
    });

    // 发布事件
    await ctx.events.publish('plugin:${config.id}:ready', { ok: true });
  },

  async deactivate(ctx) {
    ctx.log('Plugin deactivated: ${config.name}');
    ctx.commands.unregister('hello');
  },
});

export default plugin;
`;
}

function generateProcessPluginEntry(config: PluginConfig): string {
  return `// ${config.name} - Process 插件入口
// 进程插件通过 stdin/stdout 与宿主通信

// 监听 stdin 消息
process.stdin.setEncoding('utf8');
process.stdin.on('data', (data) => {
  const message = JSON.parse(data);
  handleMessage(message);
});

function handleMessage(message: { type: string; payload: unknown }) {
  if (message.type === 'activate') {
    console.log('Process plugin activated: ${config.name}');
    sendResponse({ type: 'activated', plugin: '${config.id}' });
  }
}

function sendResponse(response: Record<string, unknown>) {
  process.stdout.write(JSON.stringify(response) + '\\n');
}
`;
}

function generateWasmPluginEntry(config: PluginConfig): string {
  return `// ${config.name} - WASM 插件入口
// WASM 插件通过 host_fn 与宿主通信

// WASM 导出函数
export function activate() {
  console.log('WASM plugin activated: ${config.name}');
}

export function deactivate() {
  console.log('WASM plugin deactivated: ${config.name}');
}
`;
}

/**
 * 生成插件 tsconfig.json 内容。
 */
export function generatePluginTsconfig(): string {
  const tsconfig = {
    extends: '@tauron/base-tsconfig',
    compilerOptions: {
      outDir: 'dist',
      rootDir: 'src',
    },
    include: ['src/**/*.ts'],
  };
  return JSON.stringify(tsconfig, null, 2) + '\n';
}

/**
 * 生成插件 .gitignore 内容。
 */
export function generatePluginGitignore(): string {
  return `# Dependencies
node_modules/

# Build output
dist/

# IDE
.vscode/
.idea/

# OS
.DS_Store
Thumbs.db

# Logs
*.log
`;
}

/**
 * 生成插件项目文件列表。
 */
export function generatePluginFiles(config: PluginConfig): Map<string, string> {
  const files = new Map<string, string>();

  files.set('manifest.json', generatePluginManifest(config));
  files.set('package.json', generatePluginPackageJson(config));
  files.set('tsconfig.json', generatePluginTsconfig());
  files.set('.gitignore', generatePluginGitignore());
  files.set('src/index.ts', generatePluginEntry(config));

  return files;
}

/**
 * 执行插件脚手架生成。
 */
export function pluginScaffold(config: PluginConfigInput): PluginScaffoldResult {
  try {
    const validated = validatePluginConfig(config);
    const files = generatePluginFiles(validated);
    return { files, ok: true };
  } catch (err) {
    return {
      files: new Map(),
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 目录监听（越界检查）
// ──────────────────────────────────────────────────────────────────────────

/**
 * 验证文件路径是否在允许的目录范围内。
 *
 * 防止 plugin dev 目录监听越界读取。
 */
export function isPathWithinDir(filePath: string, dir: string): boolean {
  // 规范化路径（移除 ./、../ 等）
  const normalizedFile = normalizePath(filePath);
  const normalizedDir = normalizePath(dir);

  // 确保 dir 以 / 结尾
  const dirPrefix = normalizedDir.endsWith('/') ? normalizedDir : normalizedDir + '/';

  return normalizedFile.startsWith(dirPrefix) || normalizedFile === normalizedDir;
}

/**
 * 规范化路径（移除冗余的 /、./ 等，处理 ..）。
 */
function normalizePath(path: string): string {
  // 统一分隔符
  let normalized = path.replace(/\\/g, '/');
  // 移除 ./
  normalized = normalized.replace(/^\.\//, '');
  // 移除多余的空格
  normalized = normalized.trim();
  // 处理 ..
  const parts = normalized.split('/');
  const result: string[] = [];
  for (const part of parts) {
    if (part === '..') {
      if (result.length > 0 && result[result.length - 1] !== '..') {
        result.pop();
      }
    } else if (part !== '.' && part !== '') {
      result.push(part);
    }
  }
  return '/' + result.join('/');
}

/**
 * 验证插件 dev 监听配置。
 *
 * 检查所有配置项是否在安全范围内。
 */
export function validateDevWatchConfig(config: PluginDevWatchConfig): { ok: boolean; error?: string } {
  if (!config.dir || config.dir.trim() === '') {
    return { ok: false, error: '目录路径不能为空' };
  }

  if (config.dir.includes('..')) {
    return { ok: false, error: '目录路径不能包含 ..' };
  }

  if (!config.rootDir || config.rootDir.trim() === '') {
    return { ok: false, error: '根目录不能为空' };
  }

  if (!isPathWithinDir(config.dir, config.rootDir)) {
    return { ok: false, error: '插件目录必须在根目录范围内（越界读取被拒绝）' };
  }

  if (config.maxFiles <= 0) {
    return { ok: false, error: '最大文件数必须大于 0' };
  }

  if (config.maxFileSize <= 0) {
    return { ok: false, error: '最大文件大小必须大于 0' };
  }

  if (config.maxFileSize > 100 * 1024 * 1024) {
    return { ok: false, error: '最大文件大小不能超过 100MB' };
  }

  return { ok: true };
}

/**
 * 检查文件是否匹配忽略模式。
 */
export function isIgnored(filePath: string, patterns: string[]): boolean {
  for (const pattern of patterns) {
    if (matchesGlob(filePath, pattern)) {
      return true;
    }
  }
  return false;
}

/**
 * 简单的 glob 匹配。
 *
 * 支持 *、**、? 通配符。
 */
function matchesGlob(filePath: string, pattern: string): boolean {
  // 将 glob 模式转换为正则
  const regex = globToRegex(pattern);
  return regex.test(filePath);
}

/**
 * 将 glob 模式转换为正则表达式。
 */
function globToRegex(pattern: string): RegExp {
  let regexStr = '';
  for (let i = 0; i < pattern.length; i++) {
    const char = pattern.charAt(i);
    if (char === '*') {
      if (i + 1 < pattern.length && pattern[i + 1] === '*') {
        // ** 匹配任意字符（包括 /）
        regexStr += '.*';
        i++; // 跳过下一个 *
      } else {
        // * 匹配任意字符（不包括 /）
        regexStr += '[^/]*';
      }
    } else if (char === '?') {
      // ? 匹配单个字符（不包括 /）
      regexStr += '[^/]';
    } else if ('.+^${}()|[]\\'.includes(char)) {
      // 转义正则特殊字符
      regexStr += '\\' + char;
    } else {
      regexStr += char;
    }
  }
  return new RegExp(`^${regexStr}$`);
}
