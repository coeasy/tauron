# 命令面全量参考（自动生成，请勿手改）

> **这个文件是生成物**：`node scripts/generate-command-surface.mjs` 写，
> CI 用 `pnpm command-surface:check` 复算——它把「**代码有的命令，文档必须一条不漏**」
> 变成可执行断言，而不是某个人记得去补的表。
>
> 输入是五处事实，全部可复算：`crates/tauron-adapter/src/tauri.rs` 的 `#[tauri::command] pub fn host_*`
> 定义（业务形参、返回类型、`#[cfg(feature)]` 门、函数上方 `///` 注释首句），
> `crates/tauron-adapter/src/lib.rs` 的三个编译期命令集合（`SUBSTRATE_COMMANDS` /
> `PLUGIN_RUNTIME_COMMANDS` / `PLUGIN_INSTALL_COMMANDS`），
> `tauron_host::authz` 与适配器 feature 门控档位表，**沿委托链**（命令函数 →
> `cmd_*_as` → `admin_gate` / 私有 helper，深度上限 3）收集到的判定
> （`require_*` 拒绝型、`visible_notifications` 按身份过滤、`scoped_within_roots`
> 根目录限定），以及 `packages/*` 中按命令名发起调用的位置。
> 语义细节仍写在各专题文档；本表只保证**不漏**与**形参/返回/判定与代码同形**。

## 计数口径

| 集合 | 条数 | 注册形态 |
|---|---|---|
| 底座 `SUBSTRATE_COMMANDS` | 61 | 只依赖适配器底座即可注册 |
| 插件运行时 `PLUGIN_RUNTIME_COMMANDS` | 22 | 插件面命令，档位登记在 authz 表 |
| 插件安装 `PLUGIN_INSTALL_COMMANDS` | 2 | `plugin-install` feature，**opt-in**（`default = []`） |
| **合计（去重）** | **85** | 默认构建实际可见条数见 `contracts/public-surface-ledger.json` |

四条判据同时钉在这里，任何一条变数都说明链路断了：

- 定义了但未落进任何集合的命令：**0**（无——每个 `host_*` 都有集合归属）
- 后端有、**前端无人调用**的孤儿命令：**0**（无——85 条全部有 `packages/*` 消费者）
- 既不在档位表、沿委托链（含 `admin_gate` / `*_payload` 等中转函数）也找不到任何
  判定（`require_*` / 按身份过滤 / 根目录限定）的命令 —— 这类命令**任何有 IPC 访问的
  webview 都调得到，包括插件窗**，除非它只操作调用方自身：
  **9**（host_brand_info、host_i18n_t、host_i18n_t_params、host_window_close、host_window_maximize、host_window_minimize、host_window_restore、host_window_set_position、host_window_set_size）
- 上面那些命令里连 `///` 说明都没有的：**0**（无）

## 底座命令（主窗专属面）

**61 条**

| 命令 | 档位与判定 | Rust 业务形参 | 返回 | feature 门 | 前端落点（package） | 登记语义 |
|---|---|---|---|---|---|---|
| `host_brand_info` | 不在档位表，**代码层无判定** | *无业务形参* | `Result<crate::ProviderResult<crate::BrandInfo>, TauriError>` | — | `tauron-host` | 品牌信息（真实实现：接孤儿 crate `tauron-brand`；未配置来源时 返回 [`crate::ProviderResult::Unsupported`]，线形与其余 provider 型命令一致）。 **无需身份判定（轮 12 复核）**：只读、不含任何主体相关状态，未配置来源时诚实 降级——插件读到品牌信息不构成越权面。 |
| `host_capabilities` | `Self_`（`tauron_host::authz`） | *无业务形参* | `Result<crate::CapabilitiesBody, TauriError>` | — | `tauron-cli` `tauron-host` | 拉取宿主真实命令面与域可用性（能力协商入口，fail-closed 的真相源） |
| `host_clipboard_read` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_clipboard_read_as`） | *无业务形参* | `Result<crate::DegradedValue<String>, TauriError>` | — | `tauron-host` | 读取剪贴板（进程内）。 |
| `host_clipboard_write` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_clipboard_write_as`） | `text: String` | `Result<crate::UnsupportedBody, TauriError>` | — | `tauron-host` | 写入剪贴板（进程内）。 **代码层身份判定（轮 12）**：剪贴板是**一个**全局槽位（`shell_ext.clipboard`）， 不分主体——插件写就是覆盖别人的内容，插件读就是读走别人的内容。判定与理由见 [`crate::cmd_clipboard_read_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_deep_link_register` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_deep_link_register_as`） | `protocol: String` | `Result<crate::ProviderResult<()>, TauriError>` | — | `tauron-host` | 注册深链接协议。 **代码层身份判定（轮 11 第三批）**：仅主窗——注册的是**应用级**协议（写 `shell_ext` + 声明 topic，并会注销上一个协议），插件改它等于把整个应用的深链接 入口改到自己名下。R8 曾把它当 self-service，这里是修正。见 [`crate::cmd_deep_link_register_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_dialog_confirm` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_dialog_confirm_as`） | `title: String`、`message: String`、`confirm_label: Option<String>`、`cancel_label: Option<String>` | `Result<crate::ProviderResult<bool>, TauriError>` | — | `tauron-host` | 确认对话框；缺少 provider 时返回 `UnsupportedBody`。 **代码层身份判定（轮 12）**：确认框是「同意」原语——插件能借宿主名义骗取用户 同意，故仅主窗。理由同 [`crate::cmd_dialog_open_as`]。 |
| `host_dialog_message` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_dialog_message_as`） | `title: String`、`message: String`、`kind: Option<String>` | `Result<crate::ProviderResult<()>, TauriError>` | — | `tauron-host` | 消息对话框。 **代码层身份判定（轮 12）**：弹的是**应用级**模态——插件能借宿主的名义向用户 显示任意提示。理由同 [`crate::cmd_dialog_open_as`]。 |
| `host_dialog_open` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_dialog_open_as`） | `multiple: Option<bool>`、`directory: Option<bool>`、`filters: Option<Vec<HostFileFilter>>`、`default_path: Option<String>` | `Result<crate::ProviderResult<Option<String>>, TauriError>` | — | `tauron-host` | 文件选择对话框；缺少 provider 时返回 `UnsupportedBody`。 **代码层身份判定（轮 12）**：四条对话框命令统一仅主窗——现状是桩，但线形同形， 原生 provider 一接入就变成「任何插件都能弹应用级模态 / 借原生选择器读用户磁盘」， 判定必须在接线之前就位（同 `host_market_*` / `host_updater_*` 的先例）。 见 [`crate::cmd_dialog_open_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_dialog_save` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_dialog_save_as`） | `default_name: Option<String>`、`filters: Option<Vec<HostFileFilter>>`、`default_path: Option<String>` | `Result<crate::ProviderResult<Option<String>>, TauriError>` | — | `tauron-host` | 保存对话框；缺少 provider 时返回 `UnsupportedBody`。 **代码层身份判定（轮 12）**：仅主窗，理由同 [`crate::cmd_dialog_open_as`]—— 它决定的是「往哪里写」，接原生后等于把落盘目标交给任意插件。 |
| `host_events_approvals` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `cmd_events_approvals_as`） | *无业务形参* | `Result<Vec<crate::EventApproval>, TauriError>` | — | `tauron-host` | 读取当前 EventBus 私有 topic 审批事实 |
| `host_events_approve` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `admin_gate`） | `subscriber: String`、`topic: String` | `Result<(), TauriError>` | — | `tauron-host` | 批准某插件订阅一个私有 EventBus topic |
| `host_events_drain` | `Self_`（`tauron_host::authz`） | `kind: String` | `Result<Vec<Frame>, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` | 拉取本插件待投递帧（只取自己订阅的可见集） |
| `host_events_publish` | `Self_`（`tauron_host::authz`） | `evt: HostEventPublish` | `Result<PublishResult, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` `tauron-plugin-sdk` | 事件发布唯一入口（越界丢弃+计数） |
| `host_events_revoke` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `admin_gate`） | `subscriber: String`、`topic: String` | `Result<bool, TauriError>` | — | `tauron-host` | 撤销某插件订阅一个私有 EventBus topic 的审批 |
| `host_events_subscribe` | `Self_`（`tauron_host::authz`） | `sub: Vec<HostEventSelector>` | `Result<HostSubscription, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` | 事件订阅（跨插件订阅需对方 public:true） |
| `host_events_unsubscribe` | `Self_`（`tauron_host::authz`） | `token: String` | `Result<(), TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` | 事件退订（窗口销毁时由宿主 on_window_event → cleanup_closed_window 回收） |
| `host_fs_list` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_fs_list_as`） + 限定宿主允许根目录 `scoped_within_roots`（经 `cmd_fs_list`） | `path: String` | `Result<crate::ProviderResult<Vec<crate::FsEntry>>, TauriError>` | — | `tauron-host` | 列目录（仅主窗）。 |
| `host_fs_mkdir` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_fs_mkdir_as`） + 限定宿主允许根目录 `scoped_within_roots`（经 `cmd_fs_mkdir`） | `path: String`、`recursive: bool` | `Result<crate::ProviderResult<()>, TauriError>` | — | `tauron-host` | 建目录（仅主窗）。 |
| `host_fs_read` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_fs_read_as`） + 限定宿主允许根目录 `scoped_within_roots`（经 `cmd_fs_read`） | `path: String`、`max_bytes: Option<u64>` | `Result<crate::ProviderResult<crate::FsReadResult>, TauriError>` | — | `tauron-host` | 读取文本文件（仅主窗；限定宿主允许根目录内）。 |
| `host_fs_remove` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_fs_remove_as`） + 限定宿主允许根目录 `scoped_within_roots`（经 `cmd_fs_remove`） | `path: String` | `Result<crate::ProviderResult<()>, TauriError>` | — | `tauron-host` | 删除文件或空目录（仅主窗；**不递归**）。 |
| `host_fs_stat` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_fs_stat_as`） + 限定宿主允许根目录 `scoped_within_roots`（经 `cmd_fs_stat`） | `path: String` | `Result<crate::ProviderResult<crate::FsStat>, TauriError>` | — | `tauron-host` | 取元数据（仅主窗）。 |
| `host_fs_write` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_fs_write_as`） + 限定宿主允许根目录 `scoped_within_roots`（经 `cmd_fs_write`） | `path: String`、`text: String` | `Result<crate::ProviderResult<crate::FsWriteResult>, TauriError>` | — | `tauron-host` | 写入文本文件（仅主窗；覆盖）。 |
| `host_http_request` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_http_request_as`） | `spec: crate::HttpRequestSpec` | `Result<crate::ProviderResult<crate::HttpResponseSpec>, TauriError>` | — | `tauron-host` | 发起一次 HTTP 请求（仅主窗；缺省诚实降级）。 |
| `host_i18n_cleanup_plugin` | 不在档位表（主窗面）；绑定自身身份 `require_self_plugin_scope`（经 `cmd_i18n_cleanup_plugin_as`） | `plugin_id: String` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 清除一个插件的全部文案。 **代码层身份判定（轮 11 第二批）**：这条会**销毁**目标插件的全部文案，核心 [`crate::cmd_i18n_cleanup_plugin_as`] 拒绝插件清别人的。`window` 由 Tauri 注入， **线形不变**。 |
| `host_i18n_load` | 不在档位表（主窗面）；绑定自身身份 `require_self_plugin_scope`（经 `cmd_i18n_load_as`） | `locale: String`、`entries: serde_json::Map<String, serde_json::Value>`、`plugin_id: Option<String>` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 装载一个语言包（同语言按 key 合并，不整体替换）。 传 `pluginId` 时每个 key 自动加 `plugin:<id>.oc.` 前缀。 **代码层身份判定（轮 11 第二批）**：`pluginId` 就是命名空间归属，核心 [`crate::cmd_i18n_load_as`] 拒绝插件往别人的命名空间（或宿主文案）里装东西。 `window` 由 Tauri 注入，**线形不变**。 |
| `host_i18n_set_locale` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_i18n_set_locale_as`） | `locale: String` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 切换语言（§4.20：语言状态的单一来源）。 **代码层身份判定（轮 11 第三批）**：仅主窗——切的是**全局**语言（宿主 UI + 所有插件界面），插件改它 = 跨插件全局状态篡改。见 [`crate::cmd_i18n_set_locale_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_i18n_stats` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_i18n_stats_as`） | *无业务形参* | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | i18n 状态与缺失键可观测性。 **代码层身份判定（轮 12）**：全局文案普查（含别人的命名空间与缺失键）只给主窗， 见 [`crate::cmd_i18n_stats_as`]；取值用的 `host_i18n_t` / `host_i18n_t_params` 不判（插件界面本来就要取文案）。`window` 由 Tauri 注入，**线形不变**。 |
| `host_i18n_t` | 不在档位表，**代码层无判定** | `key: String` | `Result<String, TauriError>` | — | `tauron-host` | 翻译。 `host_i18n_t`：翻译。 **无需身份判定（轮 12 复核）**：只读取值、一次只答一个键，是插件渲染自己界面的 正常路径；全局普查面（谁的命名空间、缺了哪些键）是 `host_i18n_stats`，那条已判 仅主窗。 全部缺失时返回 key 本身（并计入缺失计数），不是空串——空串会让缺失文案 彻底隐形。缺失可观测性见 `host_i18n_stats`。 |
| `host_i18n_t_params` | 不在档位表，**代码层无判定** | `key: String`、`params: serde_json::Map<String, serde_json::Value>` | `Result<String, TauriError>` | — | `tauron-host` | 带 `{{param}}` 占位替换的翻译。 **无需身份判定（轮 12 复核）**：与 `host_i18n_t` 同——只读取值，插件渲染自己界面 的正常路径。 |
| `host_market_check` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_market_check_as`） | `endpoints: Option<Vec<String>>`、`pubkey: Option<String>` | `Result<crate::MarketCheckResult, TauriError>` | — | `tauron-host` | 检查更新（**模拟**：不做任何真实可用性探测）。 **返回**（R8 §4）：`{ available: boolean, simulated: boolean, version: string \| null, reason: string \| null }`——`simulated: true` 与 `reason` 让前端**据字段**判断 "这是本地桩结果"，而不是靠读注释。 **代码层身份判定（轮 11 第二批）**：仅主窗——商城操作的是宿主级产物（更新源 / 安装包），见 [`crate::cmd_market_check_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_market_download` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_market_download_as`） | `version: Option<String>`、`endpoints: Option<Vec<String>>`、`pubkey: Option<String>` | `Result<crate::MarketUpdateResult, TauriError>` | — | `tauron-host` | 下载更新（**模拟**，待接 `tauri-plugin-updater`）。 `endpoints`/`pubkey` 为协议保留参数（TS 侧已发送，真实 updater 接线后启用）。 **返回**（R8 §4：`simulated` 是**线字段**，不再只写在注释里）： `{ ok: boolean, simulated: boolean, version: string \| null, reason: string \| null }`。 `ok: true` 只表示"命令执行成功"，**不是**"更新已下载"——判断后者看 `simulated`。 **代码层身份判定（轮 11 第二批）**：仅主窗（同 [`host_market_check`] 的理由： 会推进宿主的 `updateState`）。`window` 由 Tauri 注入，**线形不变**。 |
| `host_market_install` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_market_install_as`） | `version: Option<String>` | `Result<crate::MarketUpdateResult, TauriError>` | — | `tauron-host` | 安装更新（**模拟**，待接 `tauri-plugin-updater`）。 返回线与 [`host_market_download`] 同形（`MarketUpdateResult`）。 **代码层身份判定（轮 11 第二批）**：仅主窗（接线后就是"替换应用自身二进制"）。 |
| `host_menu_popup` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_menu_popup_as`） | `spec: crate::MenuSpec` | `Result<crate::ProviderResult<crate::MenuOutcome>, TauriError>` | — | `tauron-host` | 弹出上下文菜单（仅主窗）。 |
| `host_menu_reset` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_menu_reset_as`） | *无业务形参* | `Result<crate::ProviderResult<crate::MenuOutcome>, TauriError>` | — | `tauron-host` | 移除应用菜单（仅主窗）。 |
| `host_menu_set` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_menu_set_as`） | `spec: crate::MenuSpec` | `Result<crate::ProviderResult<crate::MenuOutcome>, TauriError>` | — | `tauron-host` | 设置应用菜单（仅主窗）。 |
| `host_notifications_list` | 不在档位表（主窗面）；按身份过滤可见集合 `visible_notifications`（经 `notifications_list_payload`） | `limit: Option<usize>` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 通知中心读取端（`host_notify` 的配对）。 **代码层身份过滤（轮 11 第三批）**：插件只看得见**署名是自己**的条目， `total` / `unread` / `dispatchLog` 与 `items` 同源（都在可见集合上重新数）。 见 [`crate::cmd_notifications_list_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_notifications_read` | 不在档位表（主窗面）；绑定自身身份 `require_self_plugin_scope`（经 `cmd_notifications_read_as`） | `id: Option<String>` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 标记通知已读（`id` 缺省 = 全部）。 **代码层身份判定（轮 11 第三批）**：插件只能标记**自己**的通知，`None`（全部 已读 = 全局状态）一律拒绝，未知 id 也拒绝（否则 `marked` 变成存在性预言机）。 见 [`crate::cmd_notifications_read_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_notify` | 不在档位表（主窗面）；绑定自身身份 `require_self_plugin_scope`（经 `cmd_notify_as`） | `plugin_id: String`、`title: String`、`body: String` | `Result<(), TauriError>` | — | `tauron-host` | 发送通知。 **代码层身份判定（轮 11 第二批）**：入参 `pluginId` 是通知的**署名**，核心 [`crate::cmd_notify_as`] 会拒绝"以别人的名义发通知"（插件只能发自己的；主窗可发 任意署名）。`window` 由 Tauri 注入，**线形不变**（前端参数没变）。 |
| `host_production_doctor` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `cmd_production_doctor_as`） | *无业务形参* | `Result<tauron_host::ProductionDoctorReport, TauriError>` | — | `tauron-host` | 读取机器可读的 production readiness 自检报告 |
| `host_recover_boot` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_recover_boot_as`） | *无业务形参* | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 启动恢复检查。 **代码层身份判定（轮 12）**：返回的是**应用级**恢复态势，插件侧无合法读取场景， 见 [`crate::cmd_recover_boot_as`]。`window` 由 Tauri 注入，**线形不变**。 |
| `host_recover_report` | 不在档位表（主窗面）；绑定自身身份 `require_self_plugin_scope`（经 `cmd_recover_report_as`） | `outcome: String`、`plugin_id: Option<String>` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 应用上报启动结果（恢复引擎的**驱动信号**）。 `outcome` 是闭集（`success` / `failure`），表外值由核心层拒绝——不静默当成 failure 处理，否则一个拼写错误会悄悄把应用推进安全模式。 `pluginId` 仅在该插件处于 `TrialEnable` 时改变计数路径（按试验失败记， 不累入全局计数）；否则只做诊断提示。 **代码层身份判定（轮 11 第二批）**：正因为上面这条"改变计数路径"，`pluginId` 是**能伤到别人的参数**——核心 [`crate::cmd_recover_report_as`] 拒绝插件替别的 插件（或应用级 `None`）上报。`window` 由 Tauri 注入，**线形不变**。 |
| `host_settings_adopt_legacy` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_settings_adopt_legacy_as`） | `doc: serde_json::Value` | `Result<(), TauriError>` | — | `tauron-host` | 接手一份旧版（v1）宿主设置文档。 **谁能调（R7 收口后）**：**仅主窗**。这里此前写的是「与 `host_settings_get` / `host_settings_set` 完全同一口径，没有额外身份判定」——那是缺口的原文： 「特权 = 仅主窗」当时只在部署配置（origin 白名单 / Tauri ACL）里成立，代码里 没有判定。现在主体由 `window.label()` 解析并经 [`crate::require_main_window`] 硬判：插件主体一律拒绝 （[`tauron_host::ErrorCode::E_AUTH_DENIED`]）。 为什么不做成「按插件键空间限定」：本命令接手的是**整份**文档（跨所有插件与 宿主键），还会把用户层整体改写、把数据版本重标为 v1。按键的命名空间规则在这 里没有对应的键可以施加，所以只能整体拒绝。 **`doc` 的期望形状**：对象（键 = 设置键，值 = 设置值），即 R7 之前裸 `HashMap` 落盘的那份文档；数组 / 标量 / null 返回 `E_INVALID_MANIFEST`， 不静默退化成空文档。文档的键按 **v1 裸键**语义解释（可以含 `.`），写入后 数据版本标为 v1，随后由 `host_settings_migrate` 转成当前版本。 **返回**：`()`（与 `host_settings_set` 的返回口径一致）。**线形未变**—— `window` 由 Tauri 注入，前端入参仍是 `{ doc }`。 **写失败不影响启动**：本命令只改内存里的用户层，不做任何与启动/恢复链路的 交互；失败只是返回错误，标记文件、恢复引擎、启动流程都不受影响。 |
| `host_settings_get` | 不在档位表（主窗面）；绑定自身键空间 `require_settings_key_scope`（经 `cmd_settings_get_as`） | `key: String` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 读取设置。 **线形不变**（前端契约）：入参 `key: string`，返回任意 JSON；未写过的键 返回 `Null`（不是报错）。 **代码层身份判定（R7 收口）**：主窗可读任意键；插件只能读自己命名空间 （`plugin:<自己 id>` 或 `plugin:<自己 id>.…`）内的键，越界拒绝 [`tauron_host::ErrorCode::E_AUTH_DENIED`]。`window` 由 Tauri 注入，故**入参 形状一个字节都没变**。 |
| `host_settings_migrate` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_settings_migrate_as`） | *无业务形参* | `Result<usize, TauriError>` | — | `tauron-host` | 把设置迁到当前 schema 版本。 **入参**：无。**返回**：迁移**步数**（数字）。`0` = 已是最新（或本就无 数据）；重复调用恒为 `0`（幂等）。 **谁能调（R7 收口后）**：**仅主窗**——理由同 `host_settings_adopt_legacy`： 迁移会整体改写用户层（键编码契约换版），按键的命名空间规则无法表达 「只准动自己的键」。插件主体一律拒绝。 **原子性**：全有或全无——迁移链缺失、或迁移结果过不了新 schema 校验时， 用户层一个字节都不改，返回 `E_INVALID_MANIFEST`。**无数据版本标注却有数据** 时同样拒绝（宁可报错，也不猜起点）。 **写失败的影响**：同 `host_settings_adopt_legacy`（不触碰启动/恢复链路）。 |
| `host_settings_set` | 不在档位表（主窗面）；绑定自身键空间 `require_settings_key_scope`（经 `cmd_settings_set_as`） | `key: String`、`value: serde_json::Value` | `Result<(), TauriError>` | — | `tauron-host` | 写入设置。 **线形不变**（前端契约）：入参 `key: string, value: any`，返回 `()`。 写入**经 [`SettingsStore`]**（不再是裸 `HashMap`）：键会被编码成合法点路径， 值要过命名空间 schema。 **代码层身份判定（R7 收口）**：主窗可写任意键；插件只能写自己命名空间内的键。 越界在**写之前**拒绝，拒绝路径不产生任何写入副作用。 |
| `host_theme_get` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_theme_get_as`） | `id: String` | `Result<crate::ProviderResult<serde_json::Value>, TauriError>` | — | `tauron-host` | 读取单个主题（仅主窗）。 |
| `host_theme_list` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_theme_list_as`） | *无业务形参* | `Result<Vec<tauron_theme::ThemeContribute>, TauriError>` | — | `tauron-host` | 列出可用主题（仅主窗；接孤儿 crate `tauron-theme`）。 |
| `host_theme_set` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_theme_set_as`） | `id: String` | `Result<crate::ProviderResult<serde_json::Value>, TauriError>` | — | `tauron-host` | 切换激活主题（仅主窗）。 |
| `host_tray_create` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_tray_create_as`） | `spec: crate::TraySpec` | `Result<crate::ProviderResult<crate::TrayOutcome>, TauriError>` | — | `tauron-host` | 创建/更新系统托盘（仅主窗）。 |
| `host_tray_remove` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_tray_remove_as`） | *无业务形参* | `Result<crate::ProviderResult<crate::TrayOutcome>, TauriError>` | — | `tauron-host` | 移除系统托盘（仅主窗）。 |
| `host_tray_set_menu` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_tray_set_menu_as`） | `spec: crate::MenuSpec` | `Result<crate::ProviderResult<crate::TrayOutcome>, TauriError>` | — | `tauron-host` | 设置托盘菜单（仅主窗）。 |
| `host_updater_check` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_updater_check_as`） | `current_version: String` | `Result<crate::ProviderResult<crate::UpdaterCheckOutcome>, TauriError>` | — | `tauron-host` | 检查更新（仅主窗；真跑 `tauron-distribute`）。 |
| `host_updater_status` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_updater_status_as`） | *无业务形参* | `Result<crate::UpdaterStatus, TauriError>` | — | `tauron-host` | 更新通道状态（仅主窗）。 |
| `host_window_close` | 不在档位表，**代码层无判定** | *无业务形参* | `Result<(), TauriError>` | — | `tauron-host` | 关闭调用方的窗口。 **无需身份判定（轮 12 复核）**：目标同样是注入的调用方窗口 label——插件关不掉 邻居或主窗。销毁后的资源回收见 [`crate::cmd_window_close`] 的注释。 |
| `host_window_maximize` | 不在档位表，**代码层无判定** | *无业务形参* | `Result<(), TauriError>` | — | `tauron-host` | 最大化调用方的窗口。 **无需身份判定（轮 12 复核）**：目标同样是注入的调用方窗口 label。 |
| `host_window_minimize` | 不在档位表，**代码层无判定** | *无业务形参* | `Result<(), TauriError>` | — | `tauron-host` | 最小化**调用方自己**的窗口。 **无需身份判定（轮 12 复核）**：目标窗口是 Tauri 注入的调用方 label（不是入参）， 插件只能动自己的窗口；同族的应用级动作（quit / relaunch / create）已判仅主窗。 |
| `host_window_quit` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_window_quit_as`） | *无业务形参* | `Result<(), TauriError>` | — | `tauron-host` | 退出应用（`app.exit(0)` 在 [`TauriWindowSink`] 里）。 **代码层身份判定（轮 12）**：quit 是**应用级**动作（一次调用关掉整个宿主）， 与轮 11 判为仅主窗的 `host_window_relaunch` 同类且更彻底；同族窗口命令因为传 `window.label()`（调用方自己的窗口）不需要判定。见 [`crate::cmd_window_quit_as`]。 `window` 由 Tauri 注入，**线形不变**。 |
| `host_window_relaunch` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_window_relaunch_as`） | *无业务形参* | `Result<crate::WindowRelaunchOutcome, TauriError>` | — | `tauron-host` | 重启应用（**仅主窗**；先对账恢复阶段、再重启）。 **线形**：入参无（`window` 由 Tauri 注入），返回 `{ reconcile: { scanned, entered, exited, ignored }, relaunchRequested: bool, reason: string \| null }`。 **顺序**（R8 §3 的核心不变量）在 `crate::cmd_window_relaunch_as` 里是两行按序 代码：先 `reconcile_recovery_phase`，再 `window_sink.relaunch()`。反序会把 「注册表标志未与引擎判定对齐」带进下一次启动（阶段计数跨进程持久化）→ 重启循环。 对账结果随返回值给出去，所以这个顺序在运行时可观测、有单测钉住。 |
| `host_window_restore` | 不在档位表，**代码层无判定** | *无业务形参* | `Result<(), TauriError>` | — | `tauron-host` | 还原调用方的窗口（Tauri 侧为 `unmaximize`）。 **无需身份判定（轮 12 复核）**：目标同样是注入的调用方窗口 label。 |
| `host_window_set_position` | 不在档位表，**代码层无判定** | `x: i32`、`y: i32` | `Result<(), TauriError>` | — | `tauron-host` | 移动窗口（真实 Tauri 操作在 sink 里）。 **无需身份判定（轮 12 复核）**：`x` / `y` 之外没有目标窗口入参，改的是注入的 调用方窗口——插件移动不了宿主或邻居的窗口。 |
| `host_window_set_size` | 不在档位表，**代码层无判定** | `width: u32`、`height: u32` | `Result<(), TauriError>` | — | `tauron-host` | 调整窗口大小（真实 Tauri 操作在 sink 里）。 **无需身份判定（轮 12 复核）**：同 `host_window_set_position`——改的是注入的 调用方窗口，入参里没有目标 label。 |

## 插件运行时命令

**22 条**

| 命令 | 档位与判定 | Rust 业务形参 | 返回 | feature 门 | 前端落点（package） | 登记语义 |
|---|---|---|---|---|---|---|
| `host_call_end` | `Self_`（`tauron_host::authz`） | `req: HostCallEndReq` | `Result<PendingCall, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` | 流式调用终帧确认 |
| `host_call_plugin` | `Self_`（`tauron_host::authz`） | `req: HostCallPluginReq` | `Result<ProviderResult<PendingCall>, TauriError>` | — | `tauron-host` | 跨主体调用：宿主调插件或插件调插件（caller/target 显式） |
| `host_call_result` | `Self_`（`tauron_host::authz`） | `claimed_id: Option<String>`、`req: HostCallResultReq` | `Result<PendingCall, TauriError>` | — | `tauron-host` | 执行方回填一次调用的结果（仅 target 可回填） |
| `host_call_take` | `Self_`（`tauron_host::authz`） | `claimed_id: Option<String>`、`req: HostCallTakeReq` | `Result<PendingCall, TauriError>` | — | `tauron-host` | 发起方取走一次已结算的结果（仅 caller 可取） |
| `host_cancel` | `Self_`（`tauron_host::authz`） | `call_id: String` | `Result<(), TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` | 取消调用，取消传播到 sidecar/supervisor |
| `host_contributes_list` | `ScopedRead`（`tauron_host::authz`） | `kind: Option<String>` | `Result<Vec<ContributeEntry>, TauriError>` | — | `tauron-host` | 列出贡献表（commands/menus/panels/settings，纯只读） |
| `host_contributes_reconcile` | `Self_`（`tauron_host::authz`） | *无业务形参* | `Result<crate::ContributesReconcileReport, TauriError>` | — | `tauron-host` | 对账 manifest 声明的贡献与 activate 期实际注册（分叉报 E_CONTRIBUTES_DRIFT） |
| `host_contributes_register` | `Self_`（`tauron_host::authz`） | `entry: ContributeEntryInput` | `Result<(), TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` `tauron-plugin-sdk` | 注册 contributes（commands/menus/panels/…） |
| `host_lifecycle_report` | `Self_`（`tauron_host::authz`） | `claimed_id: Option<String>`、`evt: HostLifecycleEvt` | `Result<TransitionOutcome, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` | 上报生命周期事件（state/reason） |
| `host_plugin_call` | `Self_`（`tauron_host::authz`） | `claimed_id: Option<String>`、`req: HostPluginCallReq`、`channel: Option<String>` | `Result<PendingCall, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` `tauron-plugin-sdk` | C/D 类插件的 JS↔宿主调用往返（取代信封式 host_call_begin） |
| `host_recover_trial_enable` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_recover_trial_enable_as`） | `plugin_id: String` | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 安全模式内试验性启用一个插件。 **代码层身份判定（轮 11 第二批）**：仅主窗——见 [`crate::cmd_recover_trial_enable_as`] 的危害说明（改别人的状态、烧别人的试验 预算）。`window` 由 Tauri 注入，**线形不变**。 |
| `host_registry_admin` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `admin_gate`） | `op: HostAdminOp` | `Result<TransitionOutcome, TauriError>` | — | `tauron-host` `tauron-ui` | 管理操作：disable/enable/uninstall/purge |
| `host_registry_list` | `ScopedRead`（`tauron_host::authz`） | `scope: Option<String>` | `Result<Vec<PluginSummary>, TauriError>` | — | `tauron-app-plugin-sdk` `tauron-host` `tauron-ui` | 列出可见插件（结果按可见性过滤） |
| `host_registry_list_all` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_registry_list_all_as`） | *无业务形参* | `Result<Vec<PluginSummary>, TauriError>` | — | `tauron-host` | 全量列表（仅主窗）。 **代码层判定**：主体从 `window.label()` 解析后交给 [`crate::cmd_registry_list_all_as`]，插件主体被硬拒——「仅主窗」不再只是 部署配置（origin 白名单 / Tauri ACL）里的一句话。`window` 由 Tauri 注入， **线形不变**（入参仍然没有，返回仍然是 `PluginSummary[]`）。 |
| `host_resource_stats` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `cmd_resource_stats_as`） | *无业务形参* | `Result<serde_json::Value, TauriError>` | — | `tauron-host` | 读取全局与逐插件资源配额占用 |
| `host_runtime_health` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `cmd_runtime_health_as`） | `lease: String`、`generation: Option<u64>` | `Result<RuntimeHealth, TauriError>` | — | `tauron-host` | 按租约查询 sidecar 健康（pid/崩溃窗口计数；暴露 PID 故同属特权） |
| `host_runtime_spawn` | `Privileged`（`tauron_host::authz`）；仅主窗 `require_main_window`（经 `admin_gate`） | `plugin_id: String`、`profile: RuntimeSpawnProfile` | `Result<RuntimeHandle, TauriError>` | — | `tauron-host` | 启动进程插件 sidecar（幂等：已有租约则返回既有 pid/lease） |
| `host_stream_close` | `Self_`（`tauron_host::authz`） | `req: HostStreamCloseReq` | `Result<StreamFrame, TauriError>` | — | `tauron-host` | 发终帧并使句柄失效（self 档） |
| `host_stream_grant` | `Self_`（`tauron_host::authz`） | `req: HostStreamGrantReq` | `Result<crate::StreamCredit, TauriError>` | — | `tauron-host` | 补充有界 byte credit（self 档） |
| `host_stream_open` | `Self_`（`tauron_host::authz`） | `req: HostStreamOpenReq`、`channel: Option<String>` | `Result<StreamOpened, TauriError>` | — | `tauron-host` | 为一次已挂帧载体的调用开流（self 档） |
| `host_stream_write` | `Self_`（`tauron_host::authz`） | `req: HostStreamWriteReq` | `Result<StreamFrame, TauriError>` | — | `tauron-host` | 写一帧（self 档，seq 由宿主铸） |
| `host_window_create` | 不在档位表（主窗面）；仅主窗 `require_main_window`（经 `cmd_window_create_as`） | `plugin_id: String`、`title: Option<String>`、`width: Option<u32>`、`height: Option<u32>` | `Result<crate::WindowCreateOutcome, TauriError>` | — | `tauron-host` | 为**已注册**插件创建主面板窗口（**仅主窗**）。 **线形**：`{ pluginId: string, title?: string, width?: number, height?: number }` → `{ label: string, pluginId: string, created: boolean, reason: string \| null }`。 **没有 URL 参数**（刻意的）：URL 只能来自 manifest 的 `entry.ui`——让调用方指定 URL 等于让主窗把"带插件身份的 webview"指向任意地址，而 label 决定身份。 身份判定与注册表检查都在核心 （[`crate::cmd_window_create_as`]），本层只解析主体并转调。 |

## 插件安装命令（`plugin-install` feature，opt-in）

**2 条**

| 命令 | 档位与判定 | Rust 业务形参 | 返回 | feature 门 | 前端落点（package） | 登记语义 |
|---|---|---|---|---|---|---|
| `host_registry_install` | `Privileged`（`tauron_adapter（feature 门控）`）；仅主窗 `require_main_window`（经 `admin_gate`） | `package_path: String`、`approved_permissions: Vec<String>`、`review_token: crate::InstallReviewToken` | `Result<crate::PluginInstallResult, TauriError>` | `plugin-install` | `tauron-host` | 安装经过签名验证的本地插件包并写入精确授权集 |
| `host_registry_install_preview` | `Privileged`（`tauron_adapter（feature 门控）`）；仅主窗 `require_main_window`（经 `admin_gate`） | `package_path: String` | `Result<crate::PluginInstallPreview, TauriError>` | `plugin-install` | `tauron-host` | 验证签名插件包并返回安装前权限审批摘要 |

## 读表须知

1. **「档位」与「判定」是两回事**。档位（`Self_` / `ScopedRead` / `Privileged`）是
   authz 表里的登记，决定它在能力面（`CAPABILITIES`）里叫什么；判定是命令函数
   **或其委托链上的函数**（`cmd_*_as`、`admin_gate`、`notifications_list_payload` 等，
   表里注明经由哪个函数）实际执行的 `require_*` / 按身份过滤 / 根目录限定。
   **「不在档位表」不等于「主窗专属」**：本仓 `host_*` 走应用层 root 注册，能力文件
   （`examples/minimal-app/src-tauri/capabilities/default.json`）的 `permissions` 只有
   `core:default`、不按命令名授 ACL，而 `windows` 同时覆盖 `main` 与 `plugin-*`——
   所以 capability **管不到** `host_*` 的可达性。「只有主窗能碰」这句口径的唯一真凭据是
   代码层判定；本表最后一列如实写出有没有，**没判定就写没判定**。
2. **「代码层无判定」的两种合法情形**（逐条在「登记语义」里给出理由，理由必须在代码注释里）：
   ① 动作目标就是注入的**调用方自身**（如 `host_window_close` / `set_size` 用的是
   `window.label()`，不是入参 label，插件碰不到别人的窗口）；② 只读取值且不暴露跨主体
   拓扑（`host_i18n_t` / `host_i18n_t_params` 一次答一个键；`host_brand_info` 只读且
   诚实降级）。除此之外的新增命令必须带判定，否则 `wire-gate` 的清单断言会红。
3. **「前端落点」是链路证明**：列出 `packages/*` 里按该命令名发起调用的包。出现
   **缺（孤儿命令）** 即后端注册了却没有任何前端消费者，按仓库口径属孤儿逻辑——
   要么接线，要么删除并在 V4 台账登记。
4. **「Rust 业务形参」已剔除注入参数**（`state` / `app` / `window` /
   `TauriCallerSource` / `State<..>`），剩下的就是 IPC 请求体里该出现的键
   （Tauri 按 snake_case 参数名收；`@tauron/host` 侧的 camelCase → snake_case 映射与
   参数包规则见 `docs/architecture/app-layer-wire.md` 第 2 节）。
5. **「feature 门」列只登记命令函数自身的 `#[cfg(feature)]`**（写在
   `#[tauri::command]` 上方或下方都算）。集合归属才是 opt-in 的完整口径：`plugin-install`
   那 2 条既在编译期集合里也带自身 `cfg`，所以两处都会出现。
6. 返回类型里的 `ProviderResult<..>` / `DegradedValue<..>` / `UnsupportedBody`
   是**带降级语义的结果形状**（能力未就绪时返回 degraded 而不是报错）。这个形状
   **不等于**桩实现，也不等于已就绪——某个域到底是不是桩，看专题文档的「诚实边界」段。
7. 本表**不解释语义边界**。关键命令的线格式与错误契约见
   `docs/architecture/app-layer-wire.md`，面向插件作者的用法见
   `docs/api/plugin-development-guide.md`。
