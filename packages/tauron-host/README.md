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

**27 条已登记命令**：`crates/tauron-host/src/authz.rs` 的 `COMMANDS` **19 条**
（17 `self` + 2 `scoped-read`）+ `ADMIN_COMMANDS` **8 条特权**（仅主窗）；
另有 **2 条 feature-gated**（`host_registry_install_preview` / `host_registry_install`，
挂 `plugin-install`——**该特性现已进 `tauron-adapter` 的默认特性**，
见 `crates/tauron-adapter/Cargo.toml:21`；只想取底座用 `default-features = false`）。

> 「27 条」是**需登记档位**的命令；Tauri 实际注册的命令面是另一个口径：**83 条**
> （`tauron_plugin_handler!` = 底座 61 + 插件运行时 22），默认特性下 85 条。
> 两侧一致性由 `src/gates.test.ts` 逐名比对，不靠本文维护。

| 命令 | 档位 | 消费方 |
|---|---|---|
| `host_plugin_call` | self | plugin-sdk `createPlugin()` |
| `host_call_end` | self | plugin-sdk（流式终帧确认） |
| `host_cancel` | self | plugin-sdk |
| `host_lifecycle_report` | self | plugin-sdk 生命周期钩子 |
| `host_contributes_register` | self | plugin-sdk（attach 期） |
| `host_events_publish` | self | plugin-sdk |
| `host_events_subscribe` | self | plugin-sdk |
| `host_events_unsubscribe` | self | plugin-sdk（dispose 期） |
| `host_events_drain` | self | plugin-sdk（事件取件泵） |
| `host_stream_open` | self | plugin-sdk（流式开流） |
| `host_stream_write` | self | plugin-sdk（流式写帧） |
| `host_stream_grant` | self | plugin-sdk（流式 credit 补充，V4） |
| `host_stream_close` | self | plugin-sdk（流式收尾） |
| `host_contributes_reconcile` | self | plugin-sdk（激活后自检贡献声明） |
| `host_call_plugin` | self | 宿主主窗 / plugin-sdk（插件→插件） |
| `host_call_result` | self | plugin-sdk（执行方回填） |
| `host_call_take` | self | 宿主主窗 / plugin-sdk（发起方取件） |
| `host_registry_list` | scoped-read | plugin-sdk / `<oc-plugin-manager>` |
| `host_contributes_list` | scoped-read | `ShellClient.contributesList` / 应用设置中心 |
| `host_registry_admin` | privileged | 宿主 UI 主窗（`<oc-plugin-manager>`） |
| `host_runtime_spawn` | privileged | 宿主 UI 主窗（插件生命周期监管） |
| `host_runtime_health` | privileged | 宿主 UI 主窗（插件生命周期监管） |
| `host_resource_stats` | privileged | 宿主 UI 主窗（资源配额诊断） |
| `host_events_approve` | privileged | 宿主 UI 主窗（Event 审批，主题须已声明） |
| `host_events_revoke` | privileged | 宿主 UI 主窗（撤销 Event 审批） |
| `host_events_approvals` | privileged | 宿主 UI 主窗（只读审批事实清单） |
| `host_production_doctor` | privileged | 宿主 UI 主窗（生产就绪自检 A109，只读） |

> **订阅取件**：`host_events_subscribe` 只把帧排进每订阅者队列；
> 前端必须调用 `HostClient.eventsDrain(kind)` 取走帧（`host_events_publish`
> 走可靠语义，帧在 `request` 通道）。不取件等于没订阅。

## 主窗客户端（应用层）

以下客户端面向**主窗**，通过 `host_window_*` / `host_dialog_*` / `host_clipboard_*` /
`host_market_*` / `host_deep_link_*` / `host_capabilities` 等主窗命令族工作（完整命令面见
`crates/tauron-adapter/src/tauri.rs` 的 `tauron_substrate_handler!` / `tauron_plugin_handler!`
宏：底座 **57** + 插件运行时 **21** = **78** 条；`plugin-install` 2 条**已进默认特性**，
默认装配共 **80** 条）：

- `ShellClient` / `ShellController` — 标题栏动作、主题、更新事件路由（已接线）
- `AutoUpdateClient` — 检查/下载/安装/重启状态机 + 定时自动检查
- `DialogClient` — 文件对话框 / 消息框 / 剪贴板
- `DeepLinkClient` — 协议注册 + `deep-link` 事件分发（OS 回调经 Rust
  `tauron_adapter::tauri::deliver_deep_link` 双管道投递：Tauri 原生事件 + EventBus）
- `WindowState` — 窗口几何持久化（恢复/保存，越界钳制）

> **启动编排不在本包**：曾经有一个 `bootstrap()`（7 阶段启动编排：配置 → 动效 →
> splash → 插件拓扑注册 → …），但它**只被自己的测试引用**，且是 `@tauron/host`
> 里唯一一处运行时 import `@tauron/core` 的地方——本包声明 `sideEffects: false`，
> 于是它在示例产物里根本不存在。它已在 1.0-W1 删除。启动编排属**应用装配层**：
> 接入方按自己的顺序组合上面的客户端（见 `examples/minimal-app/src/main.ts`）。

## 开发

```bash
pnpm --filter @tauron/host test         # vitest（全量用例，数量随门禁增长）
pnpm --filter @tauron/host typecheck    # tsc --noEmit
pnpm --filter @tauron/host build        # 产出 dist/
```
