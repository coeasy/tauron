// ──────────────────────────────────────────────────────────────────────────
// 脚手架核心逻辑（开发计划 §4.17 `create-tauron`）。
//
// 职责：生成新项目文件、验证配置、模板快照。
//
// 关键约束：
// - 模板与框架包版本同源发版
// - 生成后 `npm install && npm run tauri dev` 一次成功
// - `--targets ios|android|web` 明确报错（ADR-16）
// ──────────────────────────────────────────────────────────────────────────

import { CAPABILITIES } from '@tauron/host';
import { placeholderIconFiles } from './icon-assets.js';

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 支持的框架类型。 */
export type Framework = 'react' | 'vue' | 'svelte' | 'vanilla';

/** 支持的目标平台。 */
export type Target = 'desktop' | 'mobile';

/** 支持的壳类型。 */
export type Shell = 'tauri' | 'electron';

/** 脚手架配置。 */
export interface ScaffoldConfig {
  /** 项目名称。 */
  name: string;
  /** 项目描述。 */
  description?: string;
  /** 项目名称（用于 import）。 */
  slug: string;
  /** UI 框架。 */
  framework: Framework;
  /** 目标平台。 */
  targets: Target[];
  /** 壳类型。 */
  shell: Shell;
  /** 启用的能力列表。 */
  capabilities: string[];
  /** 品牌标识（可选）。 */
  brand?: string;
  /**
   * tauron 源码检出根相对**生成工程根**的路径（可选）。
   *
   * 给出时生成本地源码 `path` / `file:` 依赖，适用于 Tauron 贡献开发；省略时
   * 生成固定 `1.0.1` registry 依赖，适用于第三方项目。
   */
  tauronPath?: string;
}

/**
 * 脚手架配置（调用方输入）。
 *
 * `framework` / `targets` / `shell` 保持为原始字符串，由 {@link validateConfig}
 * 校验并收窄（`targets` 中的 ADR-16 排除项会触发 {@link InvalidTargetError}）；
 * `slug` 由 `name` 派生，因此不属于输入。
 */
export interface ScaffoldConfigInput {
  name?: string;
  description?: string;
  framework?: string;
  targets?: string[];
  shell?: string;
  capabilities?: string[];
  brand?: string;
  tauronPath?: string;
}

/** 脚手架生成结果。 */
export interface ScaffoldResult {
  /** 生成的文本文件列表（路径 → 内容）。 */
  files: Map<string, string>;
  /**
   * 生成的**二进制**文件列表（路径 → 字节）。
   *
   * 单独一张表而不是把字节塞进 `files`：文本与二进制走不同的写入路径（utf-8 vs
   * 原始字节），混在一起会让「内容」这个类型变成联合类型，每个消费者都得判一次。
   */
  binaryFiles: Map<string, Uint8Array>;
  /** 是否成功。 */
  ok: boolean;
  /** 错误信息（如果失败）。 */
  error?: string;
}

/** 非法目标错误。 */
export class InvalidTargetError extends Error {
  constructor(target: string, reason: string) {
    super(`非法目标 "${target}"：${reason}`);
    this.name = 'InvalidTargetError';
  }
}

/** 配置验证错误。 */
export class ConfigValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConfigValidationError';
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 验证
// ──────────────────────────────────────────────────────────────────────────

/** 支持的框架列表。 */
export const SUPPORTED_FRAMEWORKS: readonly Framework[] = ['react', 'vue', 'svelte', 'vanilla'];

/** 支持的目标列表。 */
export const SUPPORTED_TARGETS: readonly Target[] = ['desktop', 'mobile'];

/** 支持的壳列表。 */
export const SUPPORTED_SHELLS: readonly Shell[] = ['tauri', 'electron'];

/** ADR-16：不支持的目标平台及原因。 */
export const UNSUPPORTED_TARGETS: Record<string, string> = {
  ios: 'ADR-16：移动端打包 Phase 3 单独立项，v1 不承诺 iOS',
  android: 'ADR-16：移动端打包 Phase 3 单独立项，v1 不承诺 Android',
  web: 'ADR-16：Web 目标产物明确排除，provider 抽象仅供契约测试',
};

/**
 * 验证项目名称。
 *
 * 规则：
 * - 非空
 * - 仅允许小写字母、数字、连字符
 * - 不以连字符开头或结尾
 * - 长度 3-50
 */
export function validateName(name: string): string {
  if (!name || name.trim() === '') {
    throw new ConfigValidationError('项目名称不能为空');
  }
  if (name.length < 3) {
    throw new ConfigValidationError('项目名称至少 3 个字符');
  }
  if (name.length > 50) {
    throw new ConfigValidationError('项目名称最多 50 个字符');
  }
  if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(name) && name.length > 1) {
    throw new ConfigValidationError(
      '项目名称仅允许小写字母、数字和连字符，且不能以连字符开头或结尾',
    );
  }
  if (/--/.test(name)) {
    throw new ConfigValidationError('项目名称不能包含连续连字符');
  }
  return name;
}

/**
 * 将项目名称转换为 slug（用于 import）。
 */
export function toSlug(name: string): string {
  // 先转小写，再替换非法字符
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '');
}

/**
 * 验证框架类型。
 */
export function validateFramework(framework: string): Framework {
  if (!SUPPORTED_FRAMEWORKS.includes(framework as Framework)) {
    throw new ConfigValidationError(
      `不支持的框架 "${framework}"，可选：${SUPPORTED_FRAMEWORKS.join(', ')}`,
    );
  }
  return framework as Framework;
}

/**
 * 验证目标平台。
 *
 * 对于 ADR-16 明确排除的目标，抛出 InvalidTargetError 并给理由。
 */
export function validateTargets(targets: string[]): Target[] {
  if (!Array.isArray(targets) || targets.length === 0) {
    throw new ConfigValidationError('至少需要一个目标平台');
  }
  const result: Target[] = [];
  for (const target of targets) {
    if (UNSUPPORTED_TARGETS[target]) {
      throw new InvalidTargetError(target, UNSUPPORTED_TARGETS[target]);
    }
    if (!SUPPORTED_TARGETS.includes(target as Target)) {
      throw new ConfigValidationError(
        `不支持的目标 "${target}"，可选：${SUPPORTED_TARGETS.join(', ')}`,
      );
    }
    result.push(target as Target);
  }
  return result;
}

/**
 * 验证壳类型。
 */
export function validateShell(shell: string): Shell {
  if (!SUPPORTED_SHELLS.includes(shell as Shell)) {
    throw new ConfigValidationError(`不支持的壳 "${shell}"，可选：${SUPPORTED_SHELLS.join(', ')}`);
  }
  return shell as Shell;
}

/**
 * 验证能力列表。
 *
 * 确保所有请求的能力都在 CAPABILITIES 中注册。
 */
export function validateCapabilities(capabilities: string[]): string[] {
  if (!Array.isArray(capabilities)) {
    throw new ConfigValidationError('capabilities 必须是数组');
  }
  const known = new Set(CAPABILITIES.map((c) => c.command));
  const unknown: string[] = [];
  for (const cap of capabilities) {
    if (!known.has(cap)) {
      unknown.push(cap);
    }
  }
  if (unknown.length > 0) {
    throw new ConfigValidationError(
      `未知能力：${unknown.join(', ')}。已注册能力：${CAPABILITIES.map((c) => c.command).join(', ')}`,
    );
  }
  return capabilities;
}

/**
 * 完整验证脚手架配置。
 */
export function validateConfig(config: ScaffoldConfigInput): ScaffoldConfig {
  const name = validateName(config.name ?? '');
  const framework = validateFramework(config.framework ?? 'react');
  const targets = validateTargets(config.targets ?? ['desktop']);
  const shell = validateShell(config.shell ?? 'tauri');
  const capabilities = validateCapabilities(config.capabilities ?? []);

  return {
    name,
    slug: toSlug(name),
    description: config.description ?? '',
    framework,
    targets,
    shell,
    capabilities,
    ...(config.brand !== undefined ? { brand: config.brand } : {}),
    ...(config.tauronPath !== undefined && config.tauronPath.trim() !== ''
      ? { tauronPath: config.tauronPath.trim() }
      : {}),
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 文件生成
// ──────────────────────────────────────────────────────────────────────────

/** 框架包版本（与 @tauron/host 同源发版）。 */
const FRAMEWORK_VERSION = '1.0.1';

/** 去掉路径尾部的 `/` 与 `\`。 */
function trimTrailingSlash(p: string): string {
  return p.replace(/[/\\]+$/, '');
}

/** 判断是否为 POSIX 形式的绝对路径（含 Windows 盘符 `P:/...` 形式）。 */
function isAbsolutePosix(p: string): boolean {
  return p.startsWith('/') || /^[A-Za-z]:\//.test(p);
}

/**
 * `src-tauri/` 视角下的 `tauronPath`。
 *
 * `tauronPath` 以**生成工程根**为基准，而 `Cargo.toml` 在 `src-tauri/` 下一层，
 * 所以要先 `..` 回到工程根；跨盘符（绝对路径）时不需要也不该加这一层。
 */
function cargoTauronPath(tauronPath: string): string {
  const trimmed = trimTrailingSlash(tauronPath);
  if (trimmed === '') return '..';
  return isAbsolutePosix(trimmed) ? trimmed : `../${trimmed}`;
}

/**
 * 生成 package.json 内容。
 *
 * npm 依赖固定到框架同版本；Tauron 源码开发者可用 tauronPath 覆盖为 file: 依赖。
 */
export function generatePackageJson(config: ScaffoldConfig): string {
  const hostDependency =
    config.tauronPath !== undefined
      ? `file:${trimTrailingSlash(config.tauronPath)}/packages/tauron-host`
      : FRAMEWORK_VERSION;
  const pkg: Record<string, unknown> = {
    name: config.name,
    version: '0.1.0',
    private: true,
    description: config.description || `Open Client app: ${config.name}`,
    type: 'module',
    // 脚本命名取 Tauri 官方模板的规范形状（`dev` = 前端 dev server、
    // `tauri` = Tauri CLI），这样 `tauri.conf.json` 的 `beforeDevCommand` /
    // `beforeBuildCommand` 与第三方教程里看到的写法一致，不会各说各话。
    // 一键起开发环境因此是：`npm run tauri dev`（npm 会把 `dev` 透传给 `tauri` 脚本）。
    scripts: {
      dev: 'vite',
      build: 'vite build',
      preview: 'vite preview',
      tauri: 'tauri',
      test: 'vitest run',
      typecheck: 'tsc --noEmit',
    },
    dependencies: {
      '@tauron/host': hostDependency,
      ...(config.framework === 'react' ? { react: '^18.3.1', 'react-dom': '^18.3.1' } : {}),
      ...(config.framework === 'vue' ? { vue: '^3.4.0' } : {}),
      // svelte 编译进产物、不进运行时依赖，因此只出现在 devDependencies。
    },
    devDependencies: {
      // `tauri` script（→ `tauri dev` / `tauri build`）的执行体：Tauri CLI 从 npm
      // 取，不在系统 PATH 里，所以必须进 devDependencies 才跑得起来。
      '@tauri-apps/cli': '^2.11.1',
      // `dev` / `build` 两个 script 的执行体：`tauri.conf.json` 的 before*Command
      // 也会调到它们，缺了就起不来前端。
      vite: '^7.0.0',
      ...(config.framework === 'react' ? { '@vitejs/plugin-react': '^4.3.4' } : {}),
      ...(config.framework === 'vue' ? { '@vitejs/plugin-vue': '^5.2.1' } : {}),
      ...(config.framework === 'svelte'
        ? { '@sveltejs/vite-plugin-svelte': '^4.0.4', svelte: '^4.2.0' }
        : {}),
      typescript: '^5.8.0',
      vitest: '^3.0.0',
    },
    engines: { node: '>=22' },
  };
  return JSON.stringify(pkg, null, 2) + '\n';
}

/**
 * 生成 Tauri 配置文件。
 */
export function generateTauriConfig(config: ScaffoldConfig): string {
  const tauriConfig = {
    productName: config.name,
    version: '0.1.0',
    identifier: `com.tauron.${config.slug}`,
    build: {
      // 这两个钩子是一键路径的关键：没有它们 `tauri dev` 会去连一个没人监听的
      // devUrl，`tauri build` 会去读一个不存在的 `dist/`。
      beforeDevCommand: 'npm run dev',
      beforeBuildCommand: 'npm run build',
      frontendDist: '../dist',
      devUrl: 'http://localhost:1420',
    },
    app: {
      windows: [
        {
          title: config.description || config.name,
          width: 1024,
          height: 768,
          minWidth: 800,
          minHeight: 600,
        },
      ],
      security: {
        csp: "default-src 'self'",
      },
    },
    bundle: {
      active: true,
      targets: 'all',
      // 显式声明图标：不声明时 `tauri-bundler` 也按这些默认名去找，但写出来能让
      // 「脚手架产出的图标是哪几个」可审——占位图由 generateIconFiles() 生成。
      icon: [
        'icons/32x32.png',
        'icons/128x128.png',
        'icons/128x128@2x.png',
        'icons/icon.icns',
        'icons/icon.ico',
      ],
      ...(config.brand !== undefined
        ? { windows: { webviewInstallMode: { type: 'embedBootstrapper' } } }
        : {}),
    },
  };
  return JSON.stringify(tauriConfig, null, 2) + '\n';
}

/**
 * 生成 `vite.config.ts`。
 *
 * 三处都不是可选项：
 * - `server.port` 必须与 `tauri.conf.json` 的 `devUrl` 一致（`strictPort` 保证
 *   端口被占时**报错**而不是悄悄换端口——换了端口 Tauri 就连不上，表现为白屏）；
 * - `server.watch.ignored` 排除 `src-tauri/`，否则 Rust 侧编译产物会触发前端热重载风暴；
 * - `build.outDir` 必须是 `dist`，因为 `tauri.conf.json` 的 `frontendDist` 是 `../dist`。
 *
 * `base: './'` 与示例工程同源：产物资源用相对路径，Tauri 的 `tauri://localhost`
 * 自定义协议下绝对路径会 404。
 */
export function generateViteConfig(config: ScaffoldConfig): string {
  const pluginImport =
    config.framework === 'react'
      ? `import react from '@vitejs/plugin-react';`
      : config.framework === 'vue'
        ? `import vue from '@vitejs/plugin-vue';`
        : config.framework === 'svelte'
          ? `import { svelte } from '@sveltejs/vite-plugin-svelte';`
          : '';
  const pluginExpr =
    config.framework === 'react'
      ? 'react()'
      : config.framework === 'vue'
        ? 'vue()'
        : config.framework === 'svelte'
          ? 'svelte()'
          : '';
  const plugins = pluginExpr === '' ? '[]' : `[${pluginExpr}]`;

  return `import { defineConfig } from 'vite';
${pluginImport}${pluginImport === '' ? '' : '\n'}
// 端口与 src-tauri/tauri.conf.json 的 devUrl 必须一致；strictPort 让端口占用
// 变成显式失败，而不是悄悄换端口导致 Tauri 白屏。
export default defineConfig({
  base: './',
  clearScreen: false,
  plugins: ${plugins},
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
  },
});
`;
}

/**
 * 生成 tsconfig.json 内容。
 *
 * **不自造 `extends`**：`@tauron/base-tsconfig` 这个包在本仓不存在（既没发版也没
 * 目录），`extends` 一个不存在的包会让 `tsc --noEmit` 直接报错。这里写自包含的
 * 编译选项，保证 `pnpm typecheck` 真能跑。
 */
export function generateTsconfig(config: ScaffoldConfig): string {
  const tsconfig = {
    compilerOptions: {
      target: 'ES2022',
      lib: ['ES2022', 'DOM', 'DOM.Iterable'],
      module: 'ESNext',
      moduleResolution: 'bundler',
      strict: true,
      skipLibCheck: true,
      noEmit: true,
      outDir: 'dist',
      rootDir: 'src',
      ...(config.framework === 'react' ? { jsx: 'react-jsx' } : {}),
    },
    include: ['src/**/*.ts', 'src/**/*.tsx'],
  };
  return JSON.stringify(tsconfig, null, 2) + '\n';
}

/**
 * 生成框架入口文件。
 */
export function generateAppEntry(config: ScaffoldConfig): string {
  switch (config.framework) {
    case 'react':
      return generateReactEntry(config);
    case 'vue':
      return generateVueEntry(config);
    case 'svelte':
      return generateSvelteEntry(config);
    case 'vanilla':
      return generateVanillaEntry(config);
  }
}

function generateReactEntry(config: ScaffoldConfig): string {
  return `// ${config.name} - React 入口
import React from 'react';
import ReactDOM from 'react-dom/client';

function App() {
  return React.createElement('div', null,
    React.createElement('h1', null, '${config.name}'),
    React.createElement('p', null, 'Hello from Open Client!'),
  );
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  React.createElement(React.StrictMode, null, React.createElement(App)),
);
`;
}

function generateVueEntry(config: ScaffoldConfig): string {
  return `// ${config.name} - Vue 入口
import { createApp } from 'vue';

const App = {
  template: '<div><h1>${config.name}</h1><p>Hello from Open Client!</p></div>',
};

createApp(App).mount('#root');
`;
}

function generateSvelteEntry(config: ScaffoldConfig): string {
  return `// ${config.name} - Svelte 入口
// 注意：Svelte 需要 .svelte 文件，此处为占位
console.log('${config.name} - Svelte app');
`;
}

function generateVanillaEntry(config: ScaffoldConfig): string {
  return `// ${config.name} - Vanilla JS 入口
document.addEventListener('DOMContentLoaded', () => {
  const root = document.getElementById('root');
  if (root) {
    root.innerHTML = '<h1>${config.name}</h1><p>Hello from Open Client!</p>';
  }
});
`;
}

/**
 * 生成 capabilities.json 内容。
 */
export function generateCapabilitiesJson(config: ScaffoldConfig): string {
  const caps = {
    version: 1,
    capabilities: config.capabilities,
    generatedAt: new Date().toISOString(),
  };
  return JSON.stringify(caps, null, 2) + '\n';
}

/**
 * 生成 .gitignore 内容。
 */
export function generateGitignore(): string {
  return `# Dependencies
node_modules/

# Build output
dist/
target/
build/

# IDE
.vscode/
.idea/
*.swp
*.swo

# OS
.DS_Store
Thumbs.db

# Environment
.env
.env.local

# Logs
*.log
`;
}

/**
 * 生成项目文件列表。
 *
 * 返回路径 → 内容的映射。
 */
export function generateFiles(config: ScaffoldConfig): Map<string, string> {
  const files = new Map<string, string>();

  files.set('package.json', generatePackageJson(config));
  files.set('tsconfig.json', generateTsconfig(config));
  files.set('.gitignore', generateGitignore());
  files.set('vite.config.ts', generateViteConfig(config));
  files.set('src/capabilities.json', generateCapabilitiesJson(config));

  // 框架入口
  const entryExt = config.framework === 'svelte' ? 'svelte' : 'ts';
  files.set(`src/main.${entryExt}`, generateAppEntry(config));

  // HTML 入口必须放在**工程根**：vite 的默认 root 是工程根，`index.html` 放到
  // `src/` 下会让 `vite build` 找不到入口（rollup 直接报 "Could not resolve entry"）。
  // 脚本按 vite 约定用 `/src/...` 绝对形式引用。
  files.set(
    'index.html',
    `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${config.name}</title>
</head>
<body>
  <div id="root"></div>
  <script type="module" src="/src/main.${entryExt}"></script>
</body>
</html>
`,
  );

  // Tauri 配置
  if (config.shell === 'tauri') {
    files.set('src-tauri/tauri.conf.json', generateTauriConfig(config));
    files.set('src-tauri/Cargo.toml', generateCargoToml(config));
    files.set('src-tauri/build.rs', generateTauriBuildRs());
    files.set('src-tauri/src/main.rs', generateMainRust(config));
    // Tauri v2 的规则是「不匹配任何 capability 的 webview 完全没有 IPC 访问」，
    // 所以这个文件不是可选项——缺了它主窗连 `core:default` 都拿不到，界面直接空白。
    files.set('src-tauri/capabilities/default.json', generateTauriCapabilities(config));
  }

  return files;
}

/**
 * 生成 `src-tauri/icons/` 下的**占位**图标（二进制）。
 *
 * 存在的理由不是好看，而是**不给这组文件，工程连编译都过不去**：
 * `tauri-build` 在 Windows 上生成资源文件时要读 `icons/icon.ico`，
 * macOS 打包要 `icon.icns`，Linux 要 `32x32.png` / `128x128.png`。
 *
 * 产出的是纯色占位图（见 `icon-assets.ts`），调用方必须提示使用方替换成自己的
 * 品牌图标——不这么做就等于把「占位」当「交付」。
 *
 * 非 Tauri 壳不产图标（Electron 壳的图标不在 `src-tauri/` 下）。
 */
export function generateIconFiles(config: ScaffoldConfig): Map<string, Uint8Array> {
  if (config.shell !== 'tauri') return new Map();
  const prefixed = new Map<string, Uint8Array>();
  for (const [name, bytes] of placeholderIconFiles()) {
    prefixed.set(`src-tauri/icons/${name}`, bytes);
  }
  return prefixed;
}

/**
 * 生成 `src-tauri/build.rs`。
 *
 * Tauri 工程的构建脚本：`tauri-build` 靠它产出 `gen/schemas/*` 与上下文常量。
 * 缺这个文件 `cargo check` 会在 `tauri::generate_context!()` 处失败。
 */
export function generateTauriBuildRs(): string {
  return `fn main() {
    tauri_build::build()
}
`;
}

/**
 * 生成 `src-tauri/capabilities/default.json`（Tauri v2 IPC 授权）。
 *
 * 注意这里**不是** `src/capabilities.json`：后者是 tauron 自己的「本应用声明用到
 * 哪些 `host_*` 命令」清单，两者语义不同，不能互相替代。
 *
 * `windows` 必须同时覆盖 `plugin-*`：`host_window_create` 铸出的插件面板窗 label
 * 恒为 `plugin-<插件 id>`，不在任何能力文件里就等于该窗口没有任何 IPC 访问权。
 */
export function generateTauriCapabilities(config: ScaffoldConfig): string {
  const capability = {
    $schema: '../gen/schemas/desktop-schema.json',
    identifier: 'default',
    description: `${config.name} 主窗与插件面板窗的 IPC 授权：host_* 命令族走 root 注册（裸命令名），不受 Tauri 插件 ACL 管辖；本文件授予 Tauri 核心命令面（事件监听/窗口操作等）。`,
    windows: ['main', 'plugin-*'],
    permissions: ['core:default'],
  };
  return JSON.stringify(capability, null, 2) + '\n';
}

/**
 * 生成 `src-tauri/Cargo.toml`。
 *
 * 两种坐标形态：本地贡献开发使用 `tauronPath` 的 path 依赖；普通用户固定安装
 * registry 上的同版本 crate。
 *
 * `plugin-install` 进 `default` 特性是刻意与 `examples/minimal-app` 对齐：
 * 装插件是「多插件框架」的动词，不该默认缺席；它的运行期前提
 * （`TAURON_PLUGIN_INSTALL_DIR` / 签名密钥）由生成的 `main.rs` 如实表达。
 */
function generateCargoToml(config: ScaffoldConfig): string {
  const deps =
    config.tauronPath !== undefined
      ? `tauron-shell = { version = "=${FRAMEWORK_VERSION}", path = "${cargoTauronPath(config.tauronPath)}/crates/tauron-shell", features = ["tauri"] }
tauron-adapter = { version = "=${FRAMEWORK_VERSION}", path = "${cargoTauronPath(config.tauronPath)}/crates/tauron-adapter", features = ["tauri"] }`
      : `tauron-shell = { version = "=${FRAMEWORK_VERSION}", features = ["tauri"] }
tauron-adapter = { version = "=${FRAMEWORK_VERSION}", default-features = false, features = ["tauri"] }`;

  return `[package]
name = "${config.slug}"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri = { version = "2", features = ["wry"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# schemars 可选依赖 indexmap 1.x：显式打开 std，否则 Tauri 解析出的图缺 std 特性。
# 这一行与 examples/minimal-app/src-tauri/Cargo.toml 同源（实测 cargo check 可过）。
indexmap = { version = "1.9.3", features = ["std"] }
# 框架层：plugin_invoke / plugin_cancel / plugin_emit 三条信封命令
# 应用层：host_* 命令族（默认 78 条 + plugin-install 2 条 = 80 条）
${deps}

[features]
default = ["plugin-install"]
# 只取底座（57 条命令）：不建 PluginRuntimeState、不注册插件命令。
#   cargo check --features substrate-only --all-targets
substrate-only = []
plugin-install = ["tauron-adapter/plugin-install"]

[build-dependencies]
tauri-build = { version = "2", features = [] }
`;
}

/**
 * 生成 `src-tauri/src/main.rs`。
 *
 * 这里生成的是 `examples/minimal-app/src-tauri/src/main.rs` 的**同一形态**（去掉
 * 示例专用的演示插件安装），而不是裸 `tauri::Builder::default().run(...)`：
 * 后者与 tauron 零关联，装完等于没接。
 *
 * 两种编译期形态：
 * - 默认（全量）：`state_init_with_adapter_config` + `tauron_generate_handler![]`
 *   （80 条），并在窗口销毁时回收该窗的订阅/队列与 pending 调用；
 * - `--features substrate-only`：只 `manage(SubstrateState)` +
 *   `tauron_substrate_handler![]`（57 条）。
 */
function generateMainRust(config: ScaffoldConfig): string {
  return `//! ${config.name} —— tauron 宿主入口（由 \`tauron-app new\` 生成）。
//!
//! 注册形态：**应用层 root 注册**（\`invoke_handler(tauron_generate_handler![])\`），
//! 命令以裸名暴露，前端 \`TauriBackend\` 配 \`commandPrefix: ''\`。
//! 生产客户端若改用 \`.plugin(tauron_adapter::tauri::init())\`（\`plugin:tauron|*\`
//! 路由），必须同步为 \`tauron\` 插件配置 capability——\`crates/tauron-adapter\`
//! 目前**没有** \`permissions/\` 定义，那条路由启用能力检查时缺权限条目
//! （见 docs/architecture/app-layer-wire.md §1）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 装配配置来源：设了 \`TAURON_CLIENT_CONFIG=/path/to/client-config.json\` 就按它加载，
/// 否则退回默认装配并**如实打印**原因（不静默降级）。
#[cfg(not(feature = "substrate-only"))]
fn load_adapter_config() -> tauron_adapter::AdapterConfig {
    let Some(path) = std::env::var("TAURON_CLIENT_CONFIG").ok().filter(|p| !p.trim().is_empty())
    else {
        let adapter = tauron_adapter::AdapterConfig::default();
        #[cfg(feature = "plugin-install")]
        if let Some(install) = load_plugin_install_config() {
            return adapter.with_plugin_install(install.0, install.1, install.2);
        }
        #[cfg(feature = "plugin-install")]
        eprintln!("[tauron] 缺少 TAURON_PLUGIN_INSTALL_DIR、TAURON_PLUGIN_SIGNER_KEYS 或有效 TAURON_ACL_SIGNING_KEY，签名插件安装已关闭");
        return adapter;
    };

    match tauron_adapter::ClientConfig::from_file(std::path::Path::new(&path)) {
        Ok(cfg) => {
            if std::env::var_os("RUST_LOG").is_none() {
                std::env::set_var("RUST_LOG", cfg.log_level());
            }
            println!(
                "[tauron] 已加载客户端配置 {path}（logLevel={}, 插件路径 {} 项）",
                cfg.log_level(),
                cfg.plugin_paths.as_ref().map_or(0, Vec::len),
            );
            let adapter = tauron_adapter::AdapterConfig::from_client_config(&cfg, None);
            #[cfg(feature = "plugin-install")]
            if let Some(install) = load_plugin_install_config() {
                return adapter.with_plugin_install(install.0, install.1, install.2);
            }
            #[cfg(feature = "plugin-install")]
            eprintln!("[tauron] 缺少 TAURON_PLUGIN_INSTALL_DIR、TAURON_PLUGIN_SIGNER_KEYS 或有效 TAURON_ACL_SIGNING_KEY，签名插件安装已关闭");
            adapter
        }
        Err(e) => {
            eprintln!("[tauron] 客户端配置 \`{path}\` 加载失败，回落默认装配：{}", e.message);
            tauron_adapter::AdapterConfig::default()
        }
    }
}

#[cfg(all(not(feature = "substrate-only"), feature = "plugin-install"))]
fn load_plugin_install_config() -> Option<(std::path::PathBuf, std::collections::BTreeMap<String, Vec<u8>>, Vec<u8>)> {
    let root = std::env::var_os("TAURON_PLUGIN_INSTALL_DIR").map(std::path::PathBuf::from)?;
    let signer_hex = std::env::var("TAURON_PLUGIN_SIGNER_KEYS").ok()?;
    let acl_hex = std::env::var("TAURON_ACL_SIGNING_KEY").ok()?;
    let mut keys = std::collections::BTreeMap::new();
    for entry in signer_hex.split(';').filter(|entry| !entry.trim().is_empty()) {
        let (kid, encoded) = entry.split_once(':')?;
        if kid.trim().is_empty() { return None; }
        keys.insert(kid.trim().to_string(), decode_hex(encoded.trim())?);
    }
    let acl = decode_hex(acl_hex.trim())?;
    if keys.is_empty() || acl.len() < 32 { return None; }
    Some((root, keys, acl))
}

#[cfg(all(not(feature = "substrate-only"), feature = "plugin-install"))]
fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.is_empty() || raw.len() % 2 != 0 { return None; }
    raw.as_bytes().chunks_exact(2).map(|pair| {
        let value = std::str::from_utf8(pair).ok()?;
        u8::from_str_radix(value, 16).ok()
    }).collect()
}

fn main() {
    #[cfg(feature = "plugin-install")]
    let builder = {
        let root = std::env::var_os("TAURON_PLUGIN_INSTALL_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("tauron-plugins"));
        tauron_adapter::tauri::with_plugin_asset_protocol(tauri::Builder::default(), root)
    };
    #[cfg(not(feature = "plugin-install"))]
    let builder = tauri::Builder::default();

    // ── 默认：底座 + 插件运行时（80 条命令）──
    #[cfg(not(feature = "substrate-only"))]
    let builder = builder
        // 用 state_init_with_adapter_config 而不是 state_init：前者把 ClientConfig
        // 真正接进注册表配置，「配置化选择加载」（plugin_filter）才会生效。
        .plugin(tauron_adapter::tauri::state_init_with_adapter_config(load_adapter_config()))
        .invoke_handler(tauron_adapter::tauron_generate_handler![])
        // 窗口销毁 → 回收该窗的订阅/队列与 pending 调用（零悬挂订阅）。
        // tauri::plugin::Builder 没有窗口事件钩子，只有 App Builder 有，
        // 因此这条回收必须由宿主在这里接线，不接就是泄漏。
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                use tauri::Manager;
                let state = window.state::<tauron_adapter::CommandState>();
                tauron_adapter::tauri::cleanup_closed_window(&state, window.label());
            }
        });

    // ── substrate-only：只有底座状态与底座命令族（57 条）──
    #[cfg(feature = "substrate-only")]
    let builder = builder
        .setup(|app| {
            use tauri::Manager;
            // 按生产形状传数据目录：不给的话崩溃检测只有内存态，进程一退计数即失。
            let config = tauron_adapter::AdapterConfig {
                recovery_data_dir: app.path().app_config_dir().ok(),
                ..tauron_adapter::AdapterConfig::default()
            };
            app.manage(tauron_adapter::SubstrateState::with_adapter_config(&config));
            Ok(())
        })
        .invoke_handler(tauron_adapter::tauron_substrate_handler![]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running ${config.slug}");
}
`;
}

// ──────────────────────────────────────────────────────────────────────────
// 脚手架
// ──────────────────────────────────────────────────────────────────────────

/**
 * 执行脚手架生成。
 *
 * 验证配置并生成所有文件。
 */
export function scaffold(config: ScaffoldConfigInput): ScaffoldResult {
  try {
    const validated = validateConfig(config);
    return {
      files: generateFiles(validated),
      binaryFiles: generateIconFiles(validated),
      ok: true,
    };
  } catch (err) {
    return {
      files: new Map(),
      binaryFiles: new Map(),
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
