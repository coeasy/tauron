/**
 * create-tauron-app — 应用脚手架
 *
 * 生成新的 tauron 应用项目结构。
 */

import type { AppConfig, CliOptions } from './types.js';

/**
 * 创建新应用
 */
export function createApp(config: AppConfig, options: CliOptions): AppScaffoldResult {
  const files: Record<string, string> = {};
  const name = config.name;

  // package.json
  files['package.json'] = JSON.stringify(
    {
      name,
      version: '0.1.0',
      description: config.description ?? `tauron app: ${name}`,
      type: 'module',
      scripts: {
        dev: 'vite',
        build: 'tsc && vite build',
        preview: 'vite preview',
        test: 'vitest',
        typecheck: 'tsc --noEmit',
      },
      dependencies: {
        '@tauron/core': '^0.1.0',
        '@tauron/types': '^0.1.0',
      },
      devDependencies: {
        typescript: '^5.8.0',
        vite: '^5.0.0',
        vitest: '^3.0.0',
      },
    },
    null,
    2,
  );

  // tauron.config.ts
  files['tauron.config.ts'] = generateTauronConfig(config);

  // src/index.ts
  files['src/index.ts'] = generateAppEntry(config);

  // src/main.ts
  files['src/main.ts'] = generateMain(config);

  // tsconfig.json
  files['tsconfig.json'] = JSON.stringify(
    {
      compilerOptions: {
        target: 'ES2022',
        module: 'ESNext',
        moduleResolution: 'Bundler',
        strict: true,
        noUncheckedIndexedAccess: true,
        exactOptionalPropertyTypes: true,
        verbatimModuleSyntax: true,
        isolatedModules: true,
        skipLibCheck: true,
      },
      include: ['src/**/*.ts', 'tauron.config.ts'],
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
 * 生成 tauron.config.ts
 */
function generateTauronConfig(config: AppConfig): string {
  return `import { defineConfig } from '@tauron/core';

export default defineConfig({
  plugins: {
    ${
      config.pluginTypes
        ? config.pluginTypes
            .map((t) => `      '${t}': { enabled: true },`)
            .join('\n')
        : `      js: { enabled: true },
      process: { enabled: false },
      wasm: { enabled: false },`
    }
  },
  updater: {
    enabled: true,
    channel: 'stable',
  },
  telemetry: {
    enabled: false,
  },
});
`;
}

/**
 * 生成应用入口
 */
function generateAppEntry(config: AppConfig): string {
  if (config.template === 'react') {
    return `// ${config.name} — React entry
import React from 'react';
import ReactDOM from 'react-dom/client';
import { TauronProvider } from '@tauron/adapter-react';
import App from './App';

const root = ReactDOM.createRoot(document.getElementById('root')!);
root.render(
  <React.StrictMode>
    <TauronProvider>
      <App />
    </TauronProvider>
  </React.StrictMode>,
);
`;
  }

  return `// ${config.name} — tauron app entry

import { TauronClient } from '@tauron/core';

const client = TauronClient.create({
  autoConnect: true,
});

client.on('ready', () => {
  console.log('[${config.name}] ready');
});

client.on('error', (error) => {
  console.error('[${config.name}] error:', error);
});
`;
}

/**
 * 生成 main.ts
 */
function generateMain(config: AppConfig): string {
  if (config.template === 'react') {
    return `import './index';
`;
  }

  return `import './index';
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
`;
}
