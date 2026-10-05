// ──────────────────────────────────────────────────────────────────────────
// 宿主 → 前端的 Tauri **事件主题**线值（轮 36）。
//
// 这里只放「宿主主动向 webview `emit` 的那一类通道名」，它与 `@tauron/shell-events`
// 里的 `SHELL_EVENTS` 是两套东西，不能混：
// - `SHELL_EVENTS.*` = DOM `CustomEvent` 名，发生在**同一个 webview 内**
//   （组件 → 控制器）；
// - 本文件的常量 = Tauri 事件 topic，**跨进程**（Rust 宿主 → webview）。
//
// 为什么必须单点定义：这两个字符串没有任何编译期联系——Rust 侧改一个字，TS 侧的
// 监听就静默变成零命中，而**发出端仍然一路 `Ok`**，看起来"通知发出去了"。
// 本仓已经有过同型事故（`SHELL_EVENTS` 两侧各写字面量，三个事件长期零监听）。
// 因此线值的唯一事实源是 Rust 侧的 `pub const`，本文件是它的镜像，
// 二者一致性由 `wire-gate` 的「轮 36」门禁逐条比对（解析 Rust 字面量，不靠注释）。
//
// 轮 36 / 37 各钉了一条，本轮（轮 38）把**整张表**钉住：本文件必须镜像 A 层全部
// 五条 webview 事件 topic，而 Rust 侧每一个 `pub const *TOPIC*` 都要归入「轮 38」
// 门禁的三层表之一——新增一条 topic 而不登记，门禁直接红，不必等下一轮再补。
// ──────────────────────────────────────────────────────────────────────────

/**
 * 系统通知的**投递信号**主题（Rust `tauron_adapter::NOTIFICATION_TOPIC` 的镜像）。
 *
 * 载荷只有 `{ id, pluginId, kind, ts }`，**没有标题与正文**——`Manager::emit` 在
 * Tauri 2.x 是广播给所有 webview 的，正文进广播等于跨插件内容泄露（轮 11 的修正）。
 * 所以这条事件的语义是「有新通知了，去取」，正文的唯一权威出口是
 * `host_notifications_list`（按调用方身份过滤）。监听端因此**必须**是
 * "收到信号 → 拉取快照"，不得试图从事件载荷渲染内容。
 */
export const NOTIFICATION_TOPIC = 'tauron://notification';

/**
 * 菜单 / 托盘点击的回传主题（Rust `tauron_adapter::MENU_CLICK_TOPIC` 的镜像）。
 *
 * 这是**约定** topic（不是宿主补的默认值）：`MenuItemSpec.event` 没填就一次都不发，
 * 填了自定 topic 就用自定的；仓库内的接线统一用本常量，两处字面量必然漂移（Rust 改
 * 一个字，前端监听静默零命中），所以线值只有 Rust 侧一份事实源、本文件是它的镜像，
 * 由 `wire-gate` 的「轮 37」门禁逐字比对。
 *
 * 载荷是 `MenuClickFrame`（`{ id, source: 'menu' | 'tray', native: true }`），
 * 它经 Tauri 的**事件通道**（`AppHandle::emit`）送达，**不在** `host_events_*`
 * 那套底座总线里——去 `host_events_drain` 取件永远取不到一次菜单点击。
 */
export const MENU_CLICK_TOPIC = 'tauron://menu-click';

/**
 * 深链接**投递**主题（Rust `tauron_adapter::DEEP_LINK_TOPIC` 的镜像）。
 *
 * 宿主在收到 OS 交给 app 的 URL 时 `emit` 一帧 `DeepLinkEvent`，`DeepLinkClient` 是
 * 仓库内的监听方。轮 38 之前这条腿在 TS 侧根本没有常量，监听点写着裸字面量（就写在
 * `deep-link-client.ts` 的 `listen` 调用里）：宿主改一个字，深链接打开就静默失效，而
 * 宿主侧的 `emit` 照旧成功——与轮 36/37 同一个形状。
 */
export const DEEP_LINK_TOPIC = 'deep-link';

/**
 * 原生对话框**降级信令**主题（Rust `tauron_adapter::DIALOG_DEGRADED_TOPIC` 的镜像）。
 *
 * 仓库内**零监听方**（轮 36 的逐 topic 清单已如实写明）：权威结论在命令返回值本身
 * （`simulated` / `reason` / `native: false`），这条帧只是给接入方的额外可观测性。
 * 这里仍然导出常量，是为了让"想接的人"不必手打一个和 Rust 没有编译期联系的字符串。
 */
export const DIALOG_DEGRADED_TOPIC = 'tauron://dialog-degraded';

/**
 * 深链接**注册结果**信令主题（Rust `tauron_adapter::DEEP_LINK_NATIVE_TOPIC` 的镜像）。
 *
 * 与上面的 `DEEP_LINK_TOPIC` 是两条不同的腿：那条是 URL 投递，这条是"协议有没有注册上"
 * 的诊断。同样**仓库内零监听方**，权威结论在 `host_deep_link_register` 的返回值。
 */
export const DEEP_LINK_NATIVE_TOPIC = 'tauron://deep-link-registration';
