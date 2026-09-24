# @tauron/app-plugin-sdk

tauron **应用插件开发 SDK**：类型安全的插件定义、生命周期管理、命令/设置/事件/贡献注册。

适用于在 tauron 客户端内运行的 JS 插件（业务层插件），宿主通过 `@tauron/host` 与其交互。

## 安装

```bash
pnpm add @tauron/app-plugin-sdk
```

`@tauron/host` 为对等依赖（宿主客户端类型与运行时）。

## 快速上手

```typescript
import { createPlugin } from '@tauron/app-plugin-sdk';

export default createPlugin({
  id: 'com.example.hello',
  name: 'Hello Plugin',
  version: '1.0.0',

  // 声明式命令
  commands: {
    'hello.greet': (args: { who: string }) => `hello ${args.who}`,
  },

  // 贡献注册（与 Manifest contributes.* 扩展点一致）
  contributes: {
    commands: [{ id: 'hello.greet', title: '打招呼' }],
    menus: [{ id: 'hello.menu', command: 'hello.greet' }],
    settingsTabs: [{ id: 'hello.settings', title: 'Hello 设置' }],
    shortcuts: [{ accelerator: 'Ctrl+Alt+H', command: 'hello.greet' }],
  },

  // 事件订阅声明（仅声明过的 topic 会触发 onEvent）
  events: {
    publish: ['hello.changed'],
    subscribe: ['theme-changed'],
  },

  async activate(ctx) {
    await ctx.events.publish('hello.changed', { at: Date.now() });
  },
  async deactivate(ctx) {
    // 释放资源
  },
  onEvent(topic, payload, ctx) {
    // 已声明 topic 的回调
  },
});
```

## API

| 导出 | 说明 |
|---|---|
| `createPlugin(definition)` | 由声明式 `PluginDefinition` 创建 `PluginInstance` |
| `createPluginContext(pluginId, host)` | 手动创建生命周期上下文（高级用法） |
| 类型 | `PluginDefinition` `PluginInstance` `PluginContext` `PluginHooks` `CommandHandler` `SettingsTabConfig` `EventListener` |

### PluginContext

- `ctx.host` — `HostClient`（`@tauron/host`）：宿主命令底层通道
- `ctx.commands.register/unregister/has/list/execute` — 命令表
- `ctx.settings.registerTab/unregisterTab` — 设置 Tab 贡献
- `ctx.events.publish/subscribe` — 事件发布/订阅（topic 需经 `events` 声明）
- `ctx.log.info/warn/error` — 结构化日志

## 测试子入口

```typescript
import { createPluginTestContext, createMockContext } from '@tauron/app-plugin-sdk/testing';

const ctx = createMockContext('com.example.hello');
// 用 mock 宿主上下文直接驱动插件逻辑，无需真实后端
```

## 与 @tauron/plugin-sdk 的区别

- **本包**（`app-plugin-sdk`）：面向 tauron 应用层 JS 插件（业务插件、贡献注册）
- `@tauron/plugin-sdk`：面向 iframe 沙箱插件的 postMessage 桥协议（`registerPlugin` / `PluginBridge`）
