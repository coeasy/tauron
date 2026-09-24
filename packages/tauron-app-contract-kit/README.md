# @tauron/app-contract-kit

tauron **应用契约测试套件**（§4.28）：插件注册表模拟、轻量测试运行器、断言工具与 mock 宿主 API。

用于验证「应用层实现」与「框架契约」的一致性——插件作者与应用外壳各测各的接口，
双方只要都通过本套件，集成即可预期正确。

## 安装

```bash
pnpm add -D @tauron/app-contract-kit
```

## 模块

### MockRegistry — 插件注册表模拟

```typescript
import { createMockRegistry, createPopulatedRegistry } from '@tauron/app-contract-kit';

const registry = createMockRegistry();
registry.install({
  id: 'com.example.p1',
  name: 'P1',
  version: '1.0.0',
  description: 'demo',
  author: 'tauron',
  homepage: 'https://example.com',
  type: 'js',
  main: './index.js',
  permissions: [],
  tags: [],
});

registry.enable('com.example.p1');     // → { success, error? }
registry.disable('com.example.p1');
registry.uninstall('com.example.p1');

registry.list();                       // 全部条目
registry.getState('com.example.p1');   // 'installed' | 'enabled' | 'disabled' | 'errored' | 'uninstalled'
registry.snapshot();                   // 聚合快照（统计 + 条目）
registry.getOperations();              // 操作记录（断言状态机路径用）

// 预置多插件场景
const populated = createPopulatedRegistry();
```

### PluginTestRunner — 轻量测试运行器

```typescript
import { createTestRunner, createMockHostApi, expect } from '@tauron/app-contract-kit';

const runner = createTestRunner();

runner.describe('my plugin', (suite) => {
  suite.it('invokes host command', async () => {
    const host = createMockHostApi('com.example.hello');
    const res = await host.invoke('hello.greet', { who: 'world' });
    expect(res).toBeTruthy();
  });
});

const report = await runner.run();   // RunResult：suites / totalTests / passed / failed
```

### expect — 极简断言

`toEqual` `notToEqual` `toBeTruthy` `toBeFalsy` `toBeUndefined` `toBeNull` `toMatch`
`toContain` `toThrow` `toBeType`（断言结果汇总在 `runner.getAssertions()`）。

### createMockHostApi(pluginId)

模拟宿主 API 面：`invoke` / `getSettings` / `setSettings` / `getPluginData` /
`setPluginData` / `requestPermission` / `log`——插件逻辑可在无真实后端下驱动。

## 契约覆盖

| 契约 | 内容 |
|---|---|
| 注册表状态机 | install→enable/disable→uninstall 迁移与幂等、errored 状态与错误记录 |
| 操作审计 | `getOperations()` 记录完整操作序列（含成败） |
| 宿主 API 形状 | invoke 返回信封、settings/pluginData 命名空间隔离 |

## 相关包

- `@tauron/contract-tests` — **框架层**（Rust↔TS）跨语言协议测试；本包聚焦**应用层** API 契约
- `@tauron/app-plugin-sdk` — 被测插件的开发 SDK
