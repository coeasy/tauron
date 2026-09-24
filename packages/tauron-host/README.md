# @tauron/host

tauron 能力编排层（开发计划 §4.9）。

宿主侧对应实现：`crates/tauron-host`（Rust）。

## 职责

- `invoke` / `event` / `channel` 封装
- 结构化错误规范化（ADR-04：宿主永不向 JS 抛裸 panic）
- 能力探测（每能力有 `available()`）
- **Backend provider 抽象**（ADR-16）
- tree-shaking 友好导出（`sideEffects: false`）

## 关键约束

1. **唯一 `@tauri-apps/api` 引用点** = `src/tauri-backend.ts`。
   其余模块只依赖 `Backend` 接口，因此可在无 Tauri 依赖的环境下
   （契约测试、SSR 冒烟、类型检查）完整运行。
   该约束由 `src/gates.test.ts` 的静态扫描门禁保证（§8-1）。
2. **二进制载荷禁止 base64 走 JSON**（架构 §4.8 R6）：走 `argsRaw` + `Channel`。
3. **Backend 的浏览器/mock 实现仅供契约测试，不作为 Web 交付**（ADR-16）：
   mock 单独从 `@tauron/host/testing` 引入。
4. `self` 档命令的 `pluginId` **不得**由调用方传入（ADR-17）——宿主只认
   webview label（`plugin-<id>`）。

## 导出

```ts
// 核心（无 Tauri 依赖）
import { HostClient, normalizeError, capabilityMatrix, CAPABILITIES } from '@tauron/host';

// 真实后端（唯一 import 点）
import { TauriBackend, pluginIdFromLabel } from '@tauron/host/tauri';

// 契约测试专用
import { MockBackend } from '@tauron/host/testing';
```

## 跨语言契约门禁

`src/gates.test.ts` 读取真实 Rust 源码并断言两侧同构：

| 门禁 | 内容 |
|---|---|
| §8-1 | 整个 `src/` 只有 `tauri-backend.ts` 导入 `@tauri-apps/api` |
| 错误码 | TS `HOST_ERROR_CODES` 与 Rust `error::ErrorCode` 枚举**逐项同序一致** |
| 可重试 | TS `RETRYABLE_HOST_ERROR_CODES` 与 Rust `ErrorCode::retryable()` 的 `matches!` 集合一致 |
| 命令面 | TS `CAPABILITIES` 与 Rust `authz::COMMANDS` / `ADMIN_COMMANDS` 逐项一致 |
| 废弃命令 | `host_grant_request`（D16）、`host_call_begin`（D2）两侧都不存在 |

因此任何一侧改了协议而忘了同步另一侧，CI 会在这里失败。

## 命令面（计划 §2.1 定稿）

9 条插件命令（8 条 `self` + 1 条 `scoped-read`）+ 1 条主窗特权命令：

| 命令 | 档位 | 消费方 |
|---|---|---|
| `host_plugin_call` | self | plugin |
| `host_call_end` | self | plugin |
| `host_cancel` | self | plugin |
| `host_lifecycle_report` | self | plugin |
| `host_contributes_register` | self | plugin |
| `host_events_publish` | self | plugin |
| `host_events_subscribe` | self | plugin |
| `host_events_unsubscribe` | self | plugin |
| `host_registry_list` | scoped-read | plugin |
| `host_registry_admin` | privileged | main-window |

> **订阅取件**：`host_events_subscribe` 只把帧排进每订阅者队列；
> 前端必须调用 `HostClient.eventsDrain(kind)` 取走帧（`host_events_publish`
> 走可靠语义，帧在 `request` 通道）。不取件等于没订阅。

## 主窗客户端（应用层）

以下客户端面向**主窗**，通过 `host_window_*` / `host_dialog_*` / `host_clipboard_*` /
`host_market_*` / `host_deep_link_*` 等主窗命令族工作（完整 45 条命令面见
`crates/tauron-adapter/src/tauri.rs` 的 `generate_handler!`）：

- `ShellClient` / `ShellController` — 标题栏动作、主题、更新事件路由（`bootstrap()` 已接线）
- `AutoUpdateClient` — 检查/下载/安装/重启状态机 + 定时自动检查
- `DialogClient` — 文件对话框 / 消息框 / 剪贴板
- `DeepLinkClient` — 协议注册 + `deep-link` 事件分发（OS 回调经 Rust
  `tauron_adapter::tauri::deliver_deep_link` 双管道投递：Tauri 原生事件 + EventBus）
- `WindowState` — 窗口几何持久化（恢复/保存，越界钳制）
- `bootstrap()` — 7 阶段启动编排（配置 → 动效 → splash → 插件拓扑注册 → …）

### 可选运行时集成 `@tauron/ui`

`bootstrap()` 的 motion 主题与 Splash 在运行时动态加载 `@tauron/ui`。
它**不是**包依赖（避免 ui↔host 循环）：应用装了 `@tauron/ui` 就生效，未装则静默跳过。

## 开发

```bash
pnpm --filter @tauron/host test         # vitest，288 tests
pnpm --filter @tauron/host typecheck    # tsc --noEmit
pnpm --filter @tauron/host build        # 产出 dist/
```
