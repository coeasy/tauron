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
  onSettingsChanged(settings, ctx) {
    // 本插件命名空间内的设置变更（键为线形键，如 `plugin:<id>.width`）
    // 前提：主窗已批准本插件订阅 `host:settings:changed`；未批准时不触发
  },
});
```

## API

| 导出 | 说明 |
|---|---|
| `createPlugin(definition)` | 由声明式 `PluginDefinition` 创建 `PluginInstance` |
| `createPluginContext(pluginId, host)` | 手动创建生命周期上下文（高级用法） |
| `HOST_SETTINGS_CHANGED_TOPIC` | 宿主设置变更镜像 topic 的线值（`'host:settings:changed'`）；主窗批准订阅时要用它 |
| 类型 | `PluginDefinition` `PluginInstance` `PluginContext` `PluginHooks` `CommandHandler` `SettingsTabConfig` `EventListener` |

### 设置变更观察（`onSettingsChanged`）

声明该钩子即在激活时订阅 `HOST_SETTINGS_CHANGED_TOPIC`，收到的是
**「本轮变化的线形键 → 新值」**（可直接拿去 `host_settings_get/set` 回查回写）。

- **要主窗批准**：该 topic 宿主所有、默认私有；`AdminClient.eventsApprove(pluginId, HOST_SETTINGS_CHANGED_TOPIC)`
  之前钩子永不触发（订阅拿不到 token → 取件泵不起跑），**激活仍会成功**——静默降级不是报错。
- **只看自己的键**：等于 `plugin:<自己 id>` 或以 `plugin:<自己 id>.` 开头才投递；
  别的插件的键、`host.*` 这类宿主级键不投给本钩子。
- **可丢，不可回放**：走 `event` 通道，慢消费者按 topic 丢最旧（条数与字节双预算）。
  要权威值用 `host_settings_get`，要变更流水请自建。
- **等值写入不产帧**：与当前值相同的写入按无操作回落。

完整链路与边界见 `docs/api/plugin-development-guide.md`「设置变更观察」。

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
