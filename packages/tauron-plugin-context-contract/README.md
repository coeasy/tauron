# @tauron/plugin-context-contract

插件上下文共享契约：`PluginContext` 及其窄接口。**仅类型、零运行时依赖**。

## 为什么存在（方案 R2 / §3 的 B2 发现）

仓库里并存两个插件 SDK，各自定义了一个**同名不同构**的 `PluginContext`：

| SDK | 构造 | 成员 |
|---|---|---|
| `@tauron/app-plugin-sdk` | `createPluginContext(pluginId, host)` | `pluginId` / `host` / `commands` / `events` / `settings` / `log` |
| `@tauron/plugin-sdk` | `createPluginContext({ handshakeToken })`（iframe bridge） | `ready` / `permissions` / `invoke` / `emit` / `onEvent` / `onInit` / `destroy` |

两份类型同名、都叫「插件上下文」，却没有一条成员同名同义。后果：为某一个 SDK
写的插件在另一个 SDK 下**根本无法激活**——而且是类型层面就无法赋值，没有任何
测试观测过「同一份插件定义能否在两个 SDK 下跑出同样行为」。

## 收敛方式

- `@tauron/app-plugin-sdk` 的 `PluginContext` **`extends`** 本契约，并用编译期
  断言（`AppPluginContextIsContract`）把签名漂移变成**编译失败**；
- `@tauron/plugin-sdk` 提供 `createContractContext(...)`，从它既有的
  bridge / `invoke` / 本地注册表构造契约形状。其 legacy `PluginContext` 保留
  （另一代 API，有既有测试），属**过渡**形状；
- 互操作测试（`@tauron/app-plugin-sdk/src/interop.test.ts`）用**同一份插件定义**
  在两代 SDK 下激活，断言可观察量完全相等。

## 边界（为什么是「仅类型」）

本包**零 import、零运行时依赖**，特别是**不引用 `@tauron/host`**：把
`HostClient` 写进契约就等于把插件契约绑死在 Tauri 传输上。`PluginHost` 只声明
结构性最小面 `request<T>(cmd, args?)`（+ 可选的 `contributesRegister`），
`HostClient`、iframe 适配器、测试 mock 都结构满足它——**换传输只换实现**。

## 导出

| 导出 | 说明 |
|---|---|
| `PluginContext` | 共享上下文：`pluginId` / `host` / `commands` / `events` / `settings` / `log` |
| `PluginHost` | 传输无关宿主面：必需 `request`，可选 `contributesRegister` |
| `CommandRegistry` / `PluginCommandHandler` / `AnyPluginCommandHandler` / `CommandRegisterResult` | 命令注册与派发 |
| `EventSink` / `PluginEventListener` / `PluginEventEnvelope` | 事件发布、订阅与信封（topic + 署名 pluginId） |
| `SettingsTabRegistry` / `SettingsTabConfig` | 设置 Tab 注册与注销 |
| `PluginLog` | 带插件署名的三级日志 |

## 约束

- **零依赖**：`dependencies` 必须为空（含 `@tauron/host`）。
- 契约成员只增不减；删改成员会让两代 SDK 的编译期断言同时失败，这是有意设计。
