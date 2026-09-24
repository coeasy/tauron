// ──────────────────────────────────────────────────────────────────────────
// @tauron/ui — 便利包（R3 重构后）。
//
// 职责：把两类东西聚合成一个入口——
// 1. **零宿主依赖的 UI 原语**：转出 `@tauron/ui-primitives`（设计令牌、哑组件、
//    动效、退出/组件动画）；
// 2. **需要宿主命令面的便利 Store**：`PluginManagerStore`（走 `host_registry_list`
//    / `host_registry_admin`，因此依赖 `@tauron/host`）。
//
// 依赖方向（R3 关键，单向无环）：
//   ui-primitives（零依赖）← host（客户端层）← ui（便利包）
//
// **底座项目**（任意桌面宿主，不需要插件管理）应直接依赖
// `@tauron/ui-primitives`，从而完全不拖入 `@tauron/host`。
// ──────────────────────────────────────────────────────────────────────────

// 原语全量转出（令牌 / 哑组件 / 动效 / 退出与组件动画）。
export * from '@tauron/ui-primitives';

// 需要宿主命令面的便利 Store。
export {
  PluginManagerStore,
} from './plugin-manager.js';
export type {
  AdminOp,
  PluginActionResult,
  PluginListState,
  PluginManagerConfig,
  PluginSummary,
  StoreSnapshot,
} from './plugin-manager.js';
