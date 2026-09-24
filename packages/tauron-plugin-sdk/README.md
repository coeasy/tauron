# @tauron/plugin-sdk

插件 SDK —— registerPlugin、PluginBridge、PluginContext（iframe bridge 一代）。

> **方案 R2 提示**：本 SDK 的 `PluginContext` 是 **legacy / 过渡形状**
> （`ready` / `permissions` / `invoke` / `emit` / `onEvent` / `onInit` / `destroy`）。
> 新插件代码应使用**共享契约形状** `@tauron/plugin-context-contract` 的
> `PluginContext`（`pluginId` / `host` / `commands` / `events` / `settings` / `log`），
> 并用 `createContractContext` 把两者接起来。

## 安装

```bash
pnpm add @tauron/plugin-sdk
```

## 使用（iframe 一代 / legacy）

```javascript
import { registerPlugin } from '@tauron/plugin-sdk';

registerPlugin({
  name: 'my-plugin',
  version: '1.0.0',
  methods: {
    async format({ args, ctx }) {
      return { formatted: args.code };
    },
  },
});
```

## 共享契约适配（R2）

同一份插件定义（只消费契约成员）可以在这里跑：

```ts
import { createContractContext, createPluginContext } from '@tauron/plugin-sdk';

const legacy = createPluginContext({ handshakeToken });
const ctx = createContractContext(legacy, { pluginId: 'com.example.myplugin' });
await plugin.activate(ctx); // 与 @tauron/app-plugin-sdk 下同一份 plugin 定义
```

### 语义映射

| 契约成员 | 本 SDK 的落地 |
|---|---|
| `pluginId` | 由选项给出（legacy 上下文没有身份字段——身份在握手时由宿主授予） |
| `host.request(cmd, args)` | `legacy.invoke(cmd, args)` |
| `host.contributesRegister(entry)` | 优先用 legacy 上下文自己的同名方法，否则经 `invoke` 转发 |
| `commands.*` | 适配器内的注册表（legacy 没有命令注册面）；处理器收到**契约上下文** |
| `events.subscribe` | `legacy.onEvent`（宿主 `emitToPlugin` 推送的帧到达订阅者） |
| `events.publish` | `legacy.emit`（postMessage 单向语义，发完即返回） |
| `settings.*` | 适配器内的去重账本 + best-effort 上报宿主贡献表 |
| `log.*` | 注入的 logger（缺省 `console`，前缀 `[pluginId]`） |

### 诚实标注：未接线 / 固有差异

1. **宿主命令词表**：legacy 传输只承诺「把方法名转给宿主」，**不定义命令词表**。
   `createContractContext` 用 `hostCommands` 选项给出命令名，缺省值与
   `@tauron/host` 的 `HostClient` 一致（`host_events_publish` /
   `host_contributes_register` / `host_plugin_call`）。这是适配层与
   `@tauron/app-plugin-sdk` 之间唯一无法消除的差异。
2. **没有取件泵**：legacy 的事件投递由宿主经 `emitToPlugin` **推送**，不存在
   `host_events_drain`；因此 `events.subscribe` 只挂本地监听，不做取件。
3. **设置 Tab 的宿主可见性**：本地账本（去重 / 注销）与 app 侧完全等价；
   「喂给宿主贡献表」这条副作用走的是 legacy 的 `invoke`，宿主是否真的把它
   落进贡献表取决于宿主实现（契约里 `contributesRegister` 是**可选**成员，
   正是为了不假装所有传输都有贡献面）。
4. **legacy 专有成员不进契约**：`ready` / `permissions` / `onInit` / `invoke` /
   `emit` / `onEvent` / `destroy` 都是 postMessage 与握手细节，契约**有意**
   不包含它们——插件代码不得依赖（互操作测试有显式断言）。

## 验证

- `@tauron/plugin-sdk`：36 tests（含 legacy 上下文与 bridge 的既有回归）
- 互操作：`packages/tauron-app-plugin-sdk/src/interop.test.ts` 用**同一份插件
  定义**在两个 SDK 下激活，断言命令返回 / 事件载荷 / 设置 Tab / 日志序列相等。
