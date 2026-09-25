/**
 * tauron CLI — 应用脚手架生成器
 *
 * 纯生成器：只拼字符串，**不碰文件系统**（落盘归 `scaffold-writer.ts`）。
 *
 * ## 生成物必须自洽可编译
 *
 * 这里引用的每个符号都要在真实包里存在。历史教训：本文件曾生成
 * `import { TauronClient } from '@tauron/core'`（`src/index.ts`）与
 * `import { defineConfig } from '@tauron/core'`（`tauron.config.ts`）——
 * 两者在 `@tauron/core` 里**都不存在**；React 模板还引用了
 * `<TauronProvider>`（该组件要求 `backend` prop，模板没传）、把 JSX 写进 `.ts`
 * 文件、`package.json` 里没有 react 依赖、也没有 `App.tsx`。
 * 结果是 `tauron create` 产出的工程一装就编译不过，而 CLI 照样打印
 * `Created app 'x' with N files`。
 *
 * 改动本文件时的自检清单：
 * 1. 对照 `packages/tauron-core/src/index.ts`（及 `adapter-*`）的**真实导出**；
 * 2. 生成的每个 import 都能解析；
 * 3. `package.json` 的 dependencies 覆盖入口里用到的每个包；
 * 4. 含 JSX 的文件必须是 `.tsx`，且 `tsconfig` 要开 `jsx`。
 */

import type { AppConfig, CliOptions } from './types.js';

/** 已实现的模板。`vue` / `svelte` 在 `AppConfig` 里已声明但**尚无骨架实现**。 */
export const IMPLEMENTED_TEMPLATES = ['vanilla', 'react'] as const;
export type ImplementedTemplate = (typeof IMPLEMENTED_TEMPLATES)[number];

/**
 * 创建应用骨架。
 */
export function createApp(config: AppConfig, options: CliOptions): AppScaffoldResult {
  const files: Record<string, string> = {};
  const name = config.name;
  const isReact = config.template === 'react';

  // package.json —— 依赖清单必须覆盖入口里用到的每一个包
  files['package.json'] = JSON.stringify(
    {
      name,
      version: '0.1.0',
      description: config.description ?? `tauron app: ${name}`,
      type: 'module',
      scripts: {
        dev: 'vite',
        build: 'tsc --noEmit && vite build',
        preview: 'vite preview',
        test: 'vitest',
        typecheck: 'tsc --noEmit',
      },
      dependencies: {
        '@tauron/core': '^0.1.0',
        '@tauron/types': '^0.1.0',
        ...(isReact
          ? {
              '@tauron/adapter-react': '^0.1.0',
              react: '^18.3.1',
              'react-dom': '^18.3.1',
            }
          : {}),
      },
      devDependencies: {
        typescript: '^5.8.0',
        vite: '^5.0.0',
        vitest: '^3.0.0',
        ...(isReact ? { '@types/react': '^18.3.0', '@types/react-dom': '^18.3.0' } : {}),
      },
    },
    null,
    2,
  );

  // vite 入口页：没有它 `vite dev` / `vite build` 会直接报
  // "Could not resolve entry module index.html"
  files['index.html'] = generateIndexHtml(config);

  // vite 配置
  files['vite.config.ts'] = generateViteConfig();

  // tauron.config.ts
  files['tauron.config.ts'] = generateTauronConfig(config);

  // 应用入口：React 模板含 JSX，必须是 .tsx（写进 .ts 是编译错误）
  const entryPath = isReact ? 'src/index.tsx' : 'src/index.ts';
  files[entryPath] = generateAppEntry(config);

  // React 模板的根组件（入口 `import App from './App'` 需要有落点）
  if (isReact) {
    files['src/App.tsx'] = generateReactApp(config);
  }

  // src/main.ts（vite 入口引用的模块）
  files['src/main.ts'] = generateMain(config);

  // tsconfig.json —— 必须含 DOM lib（入口用 document）与 JSX 选项
  files['tsconfig.json'] = JSON.stringify(
    {
      compilerOptions: {
        target: 'ES2022',
        module: 'ESNext',
        moduleResolution: 'Bundler',
        lib: ['ES2022', 'DOM', 'DOM.Iterable'],
        strict: true,
        noEmit: true,
        noUncheckedIndexedAccess: true,
        exactOptionalPropertyTypes: true,
        verbatimModuleSyntax: true,
        isolatedModules: true,
        skipLibCheck: true,
        ...(isReact ? { jsx: 'react-jsx' } : {}),
      },
      include: isReact
        ? ['src/**/*.ts', 'src/**/*.tsx', 'tauron.config.ts', 'vite.config.ts']
        : ['src/**/*.ts', 'tauron.config.ts', 'vite.config.ts'],
    },
    null,
    2,
  );

  // .gitignore
  files['.gitignore'] = generateGitignore();

  // README.md
  files['README.md'] = generateAppReadme(config);

  return { files, dirName: name };
}

export interface AppScaffoldResult {
  files: Record<string, string>;
  dirName: string;
}

/**
 * 生成 `tauron.config.ts`。
 *
 * **不 import `defineConfig`**——`@tauron/core` 没有这个导出（历史上这里生成过
 * 一行 `import { defineConfig } from '@tauron/core'`，直接导致脚手架工程编译失败）。
 * 当前 CLI 只生成、不消费本文件，因此用一个纯对象字面量即可。
 */
function generateTauronConfig(config: AppConfig): string {
  const pluginLines = config.pluginTypes
    ? config.pluginTypes.map((t) => `    '${t}': { enabled: true },`).join('\n')
    : `    js: { enabled: true },
    process: { enabled: false },
    wasm: { enabled: false },`;

  return `// ${config.name} — tauron 应用配置
//
// 注意：当前 CLI **只生成不读取**本文件；它是给宿主/工具链预留的声明位。
// 想用类型标注可以 import type（不会引入运行期依赖）。
export default {
  plugins: {
${pluginLines}
  },
  updater: {
    enabled: true,
    channel: 'stable',
  },
  telemetry: {
    enabled: false,
  },
};
`;
}

/**
 * 生成应用入口。
 */
function generateAppEntry(config: AppConfig): string {
  if (config.template === 'react') {
    return `// ${config.name} — React 入口
//
// \`TauronProvider\` 的 \`backend\` 是**必填** prop（见 @tauron/adapter-react 的
// \`TauronProviderProps\`），漏传会编译失败。
import { createTauriBackend } from '@tauron/core';
import { TauronProvider } from '@tauron/adapter-react';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';

const container = document.getElementById('root');
if (!container) {
  throw new Error('#root 未找到：请检查 index.html');
}

createRoot(container).render(
  <StrictMode>
    <TauronProvider backend={createTauriBackend()}>
      <App />
    </TauronProvider>
  </StrictMode>,
);
`;
  }

  return `// ${config.name} — tauron 应用入口
//
// 只使用 \`@tauron/core\` 的**真实导出**（createTauriBackend / invokePlugin /
// listenEvent / isTauri）。历史上这里 import 过并不存在的 \`TauronClient\`。
import { createTauriBackend, invokePlugin, listenEvent, isTauri } from '@tauron/core';

const backend = createTauriBackend();

/** 调用某个插件的方法（信封协议，见 docs/architecture/app-layer-wire.md）。 */
export function callPlugin(pluginId: string, method: string, payload?: unknown) {
  return invokePlugin(backend, pluginId, method, payload);
}

/** 订阅事件，返回取消订阅函数。 */
export function onEvent(topic: string, handler: (payload: unknown) => void) {
  return listenEvent(backend, topic, handler);
}

if (isTauri()) {
  console.log('[${config.name}] tauri 运行时已就绪');
} else {
  console.warn('[${config.name}] 非 Tauri 运行时：IPC 调用会失败（后端返回通道不可用）');
}
`;
}

/**
 * 生成 React 根组件。
 */
function generateReactApp(config: AppConfig): string {
  return `import { usePluginId } from '@tauron/adapter-react';

/**
 * 根组件。
 *
 * \`usePluginId()\` 在插件 iframe 内返回插件 ID，在宿主主窗返回 \`null\`。
 */
export default function App() {
  const pluginId = usePluginId();

  return (
    <main style={{ fontFamily: 'system-ui, sans-serif', maxWidth: 720, margin: '2rem auto', padding: '0 1rem' }}>
      <h1>${config.name}</h1>
      <p>插件 ID：{pluginId ?? '（宿主主窗，非插件上下文）'}</p>
    </main>
  );
}
`;
}

/**
 * 生成 main.ts（vite 入口引用的模块）。
 */
function generateMain(config: AppConfig): string {
  return `// ${config.name} — vite 入口模块（index.html 的 <script type="module"> 指向这里）
import './index';
`;
}

/**
 * 生成 index.html（vite 入口页）。
 */
function generateIndexHtml(config: AppConfig): string {
  return `<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>${config.name}</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
`;
}

/**
 * 生成 vite.config.ts。
 */
function generateViteConfig(): string {
  return `import { defineConfig } from 'vite';

export default defineConfig({
  // 插件 iframe 是 opaque origin，产物资源必须走相对路径
  base: './',
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // tauri dev 依赖固定端口；src-tauri 的编译产物不参与前端热更新
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: 'es2022',
  },
});
`;
}

/**
 * 生成 .gitignore
 */
function generateGitignore(): string {
  return `node_modules/
dist/
*.log
.env
.DS_Store
`;
}

/**
 * 生成应用 README
 */
function generateAppReadme(config: AppConfig): string {
  return `# ${config.name}

${config.description ?? `tauron app: ${config.name}`}

模板：\`${config.template}\`

## Getting Started

\`\`\`bash
npm install
npm run dev
\`\`\`

## Build

\`\`\`bash
npm run build
\`\`\`

## Test

\`\`\`bash
npm test
\`\`\`

## 说明

- 依赖 \`@tauron/core\` 的信封协议（\`plugin_invoke\` / \`plugin_cancel\` / \`plugin_emit\`）；
- 需要宿主侧已注册命令面（见 tauron 的 \`tauron-shell\` / \`tauron-adapter\`）；
- \`@tauron/*\` 包当前**未发布到 npm**，本地开发请用 \`workspace:*\` 或 \`file:\` 依赖。
`;
}
