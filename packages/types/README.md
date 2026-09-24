# @tauron/types

类型定义包 —— 信封、错误码、ACL、Manifest、事件等核心类型。

## 安装

```bash
pnpm add @tauron/types
```

## 主要导出

- `PluginInvokeRequest` / `PluginInvokeResponse` — 信封类型
- `PluginErrorCode` — 错误码枚举 (14 个 SC-xxxx)
- `PluginPermissionGrant` — 权限授予
- `PluginManifest` — 插件清单
- `PluginEvent` — 事件类型
- `TauronConfig` — 配置类型

## 使用

```typescript
import type { PluginInvokeRequest, PluginInvokeResponse } from '@tauron/types';

const request: PluginInvokeRequest = {
  pluginId: 'com.example.plugin',
  method: 'format',
  callId: 'uuid-123',
  timeoutMs: 30000,
};
```
