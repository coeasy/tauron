# @tauron/core

核心层 —— invokePlugin、EventBus、ACL、Registry、ConfigManager。

## 安装

```bash
pnpm add @tauron/core
```

## 主要导出

- `invokePlugin` — 调用插件方法
- `cancelPlugin` — 取消调用
- `listenEvent` / `emitEvent` — 事件订阅/发布
- `PluginRegistry` — 插件注册表
- `EventBus` — 事件总线
- `ConfigManager` — 配置管理
- `checkPluginPermission` — 权限检查
- `createTauriBackend` — Tauri 后端

## 使用

```typescript
import { createTauriBackend, invokePlugin } from '@tauron/core';

const backend = createTauriBackend();

const result = await invokePlugin(
  backend,
  'com.example.plugin',
  'format',
  { code: '...' }
);
```

## 线上传输（`createTauriBackend`）

后端只发三条宿主命令：`plugin_invoke` / `plugin_cancel` / `plugin_emit`。取消载荷是
`@tauron/types` 的 `PluginCancelRequest`（`{ callId }`，外层再包一个 `request` 键）——
宿主侧对应的 Rust struct 带 `deny_unknown_fields`，**字段名写错不是「多一个字段」，而是整次
调用反序列化失败**，所以本包的两处取消调用点都按该类型标注，字段集由 `@tauron/contract-tests`
逐字段核对 Rust 定义。

## 已知边界（不要按 README 的导出名推断「仓库里跑通过」）

- `PluginRegistry` / `ConfigManager` 是本包对外 SDK 面的历史项：仓内**没有生产装配点**，
  插件注册的线上事实源是 Rust `tauron-host::Registry`，设置的读写事实源是 Rust
  `cmd_settings_*`。二者登记在 `contracts/orphan-public-api.json`。
- 内层权限检查（`checkPluginPermission` / `getMissingPermissions`）是**扩展点**：默认分发主链
  不调用它，宿主按字符串比对授予表；`@tauron/types` 的 `PERMISSION_GRANULARITY` 只是声明词表，
  没有任何执行者（轮 58 / 轮 61 实测）。
