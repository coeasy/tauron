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
