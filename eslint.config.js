// ESLint 9 flat config —— 单一根配置，20 个包的 `lint` 脚本都复用它
// （ESLint 9 会从执行目录向上查找配置，因此在包目录里跑也能命中这一份）。
//
// ⚠️ 诚实边界：本仓库的 TypeScript 代码从未跑过 ESLint，首次 `pnpm lint`
// 会报出一批问题。CI 里的 lint 任务是 advisory（不阻塞合并）。清理干净后
// 把 CI 里的 `continue-on-error` 删掉即可转正。详见 CHANGELOG.md「已知债务」。
//
// 刻意**不**启用 type-aware（`recommendedTypeChecked`）规则集：那需要为每个
// 包生成 TS program，在 20 包 monorepo 上会把 lint 从秒级拖到分钟级；而类型
// 正确性已由 `tsc --noEmit`（`pnpm typecheck`）独立把关，不需要 ESLint 再算一遍。
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: [
      '**/dist/**',
      '**/node_modules/**',
      '**/*.tsbuildinfo',
      // 示例的 Tauri 工程是独立依赖树，不进 lint 范围
      'examples/minimal-app/src-tauri/**',
      'examples/minimal-app/dist/**',
      '.workbuddy-ai/**',
    ],
  },
  ...tseslint.configs.recommended,
  {
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      // 同一份代码同时跑在 Node（CLI / 工具链）与浏览器（UI 适配层）里，
      // 两套全局都开，免得把 `process` / `document` 误报成未定义。
      globals: { ...globals.node, ...globals.browser },
    },
    rules: {
      // 契约层与适配层里大量「有意不用」的形参/变量以 `_` 前缀标记，
      // 这是本仓库的既有约定（如 `_ctx`、`_event`）。
      '@typescript-eslint/no-unused-vars': [
        'warn',
        {
          argsIgnorePattern: '^_',
          varsIgnorePattern: '^_',
          caughtErrorsIgnorePattern: '^_',
        },
      ],
      // 线格式里存在刻意的 `any`（Rust 侧 JSON 的落点），降级为提醒。
      '@typescript-eslint/no-explicit-any': 'warn',
      // 空接口在契约声明里用于「占位扩展」，是有意为之。
      '@typescript-eslint/no-empty-object-type': 'off',
    },
  },
  {
    // 测试文件：断言里常出现「刻意构造的非法值」，放开几条噪音规则。
    files: ['**/*.test.ts', '**/*.spec.ts'],
    rules: {
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-non-null-assertion': 'off',
    },
  },
);
