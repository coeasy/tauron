// ──────────────────────────────────────────────────────────────────────────
// 脚手架核心逻辑（开发计划 §4.17 `create-tauron`）。
//
// 职责：生成新项目文件、验证配置、模板快照。
//
// 关键约束：
// - 模板与框架包版本同源发版
// - 生成后 `pnpm i && pnpm tauri dev` 一次成功
// - `--targets ios|android|web` 明确报错（ADR-16）
// ──────────────────────────────────────────────────────────────────────────

import type { Capability } from '@tauron/host';
import { CAPABILITIES } from '@tauron/host';

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
}

/** 脚手架生成结果。 */
export interface ScaffoldResult {
  /** 生成的文件列表（路径 → 内容）。 */
  files: Map<string, string>;
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
    throw new ConfigValidationError(
      `不支持的壳 "${shell}"，可选：${SUPPORTED_SHELLS.join(', ')}`,
    );
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
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 文件生成
// ──────────────────────────────────────────────────────────────────────────

/** 框架包版本（与 @tauron/host 同源发版）。 */
const FRAMEWORK_VERSION = '0.1.0';

/**
 * 生成 package.json 内容。
 */
export function generatePackageJson(config: ScaffoldConfig): string {
  const pkg: Record<string, unknown> = {
    name: config.name,
    version: '0.1.0',
    private: true,
    description: config.description || `Open Client app: ${config.name}`,
    type: 'module',
    scripts: {
      dev: 'tauri dev',
      build: 'tauri build',
      preview: 'vite preview',
      test: 'vitest run',
      typecheck: 'tsc --noEmit',
    },
    dependencies: {
      '@tauron/host': `workspace:*`,
      ...(config.framework === 'react' ? { react: '^18.3.1', 'react-dom': '^18.3.1' } : {}),
      ...(config.framework === 'vue' ? { vue: '^3.4.0' } : {}),
      ...(config.framework === 'svelte' ? { svelte: '^4.2.0' } : {}),
    },
    devDependencies: {
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
      ...(config.brand !== undefined ? { windows: { webviewInstallMode: { type: 'embedBootstrapper' } } } : {}),
    },
  };
  return JSON.stringify(tauriConfig, null, 2) + '\n';
}

/**
 * 生成 tsconfig.json 内容。
 */
export function generateTsconfig(config: ScaffoldConfig): string {
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
  files.set('src/capabilities.json', generateCapabilitiesJson(config));

  // 框架入口
  const entryExt = config.framework === 'svelte' ? 'svelte' : 'ts';
  files.set(`src/main.${entryExt}`, generateAppEntry(config));

  // HTML 入口
  files.set('src/index.html', `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${config.name}</title>
</head>
<body>
  <div id="root"></div>
  <script type="module" src="/main.${entryExt}"></script>
</body>
</html>
`);

  // Tauri 配置
  if (config.shell === 'tauri') {
    files.set('src-tauri/tauri.conf.json', generateTauriConfig(config));
    files.set('src-tauri/Cargo.toml', generateCargoToml(config));
    files.set('src-tauri/src/main.rs', generateMainRust());
  }

  return files;
}

function generateCargoToml(config: ScaffoldConfig): string {
  return `[package]
name = "${config.slug}"
version = "0.1.0"
edition = "2021"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
`;
}

function generateMainRust(): string {
  return `fn main() {
  tauri::Builder::default()
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
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
    const files = generateFiles(validated);
    return { files, ok: true };
  } catch (err) {
    return {
      files: new Map(),
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
