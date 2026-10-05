# 渐进接入指南：三档装配（只底座 / 底座 + i18n + notify / + 插件运行时）

> 适用对象：应用层宿主（`tauron-adapter` + `@tauron/host`）。
> 框架层只取信封协议（`tauron-shell`，3 条命令）见[附录 A](#附录-a框架层只取信封协议tauron-shell3-条命令)。
> **命令数均为实测值**（核对日期 **2026-09-26**，计数方法与门禁见 §0.3）。
> 交叉引用：[应用层线格式协议 · §1 注册形态](../architecture/app-layer-wire.md)、
> [架构概览](../architecture/overview.md)（「两层架构」「接线状态（诚实披露）」两节）、
> [canonical 归属](../architecture/canonical-owners.md)。
>
> **不想手工接线**：`@tauron/app-cli` 的 `tauron-app new`（新建工程）/ `tauron-app init`
> （注入现有 Tauri 2 项目）会按本文档的装配生成 `src-tauri`，并固定依赖到 Tauron `1.1.0`
> ——见 [安装与使用 · §3.2](../installation.md)。源码贡献时可显式用 `--tauron-path` 切换成本地
> `path` / `file:` 坐标。生成物已含 vite 配置、`before*Command` 钩子与
> 占位图标，`npm install && npm run tauri dev` 可一键起步；`src-tauri/icons/` 是纯色占位图，
> 发布前须替换成品牌图标。

> 官方支持边界是 Tauri 2 + React/Vue/Svelte/原生 TypeScript 前端。其他宿主需自定义
> `Backend`/`HostTransport` 并保留宿主自身的授权检查，详见[支持范围](./support-boundary.md)。

---

## 0. 三档一览（命令面实测）

| 档 | Rust 装配 | handler 宏 | **实测命令数** | 典型宿主 |
|---|---|---|---|---|
| **档 1 只底座** | 只 `manage(SubstrateState)` | `tauron_substrate_handler![]` | **61** | harness 壳、单窗口工具壳 |
| **档 2 底座 + i18n + notify** | 与档 1 **完全相同** | `tauron_substrate_handler![]` | **61**（i18n 6 + notify 3 已含在内） | 需要多语言 / 通知中心的壳 |
| **档 3 再加插件运行时** | `state_init*()` 或 `init*()`（注册两份状态） | `tauron_plugin_handler![]`（= `tauron_generate_handler![]`） | **83**（= 底座 61 + 插件运行时 22） | 多插件客户端 |

> **一处必须如实说明的粒度问题**：i18n（6 条）与 notify（3 条）的命令**已经在底座宏的
> 61 条里**（`tauron_substrate_handler!` 的命令列表），而 `tauron-adapter`
> 对 `tauron-i18n` / `tauron-notify` 是**非可选**依赖
> （`crates/tauron-adapter/Cargo.toml:16-17`）。因此今天**做不到**「只要底座、不要
> i18n/notify 的命令面」——**档 1 与档 2 的编译期命令面完全相同（都是 61 条）**，
> 两者的差别只在「宿主是否真的使用这两个域」（是否装载语言包、是否开通知中心读端）。
> 真要按域裁剪，需要新增第三个族宏（当前只有两组编译期可选集合，理由见
> [app-layer-wire.md §1](../architecture/app-layer-wire.md) 的「为什么是两组集合」）。
> 这是本指南里唯一「方案设想」与「代码现状」不一致的地方，明确标注为**规划**。

> ✅ **并发重构已收口（轮 11 更新）**：本文初稿实测于 2026-09-24 17:0x，当时
> `crates/tauron-adapter` 正被并发的 R7 工作流改写，`host_settings_migrate`
> 尚未注册为 `#[tauri::command]`。该工作流已落地：settings 走
> `tauron-settings::SettingsStore`、通知接 `DispatchSink`，且
> `host_settings_adopt_legacy` / `host_settings_migrate` **都已注册进两个 handler 宏**
> （底座 35 → **39**、全量 50 → **60**，插件域差集 15 → **21**；0.4-A1 后另加跨主体调用 3 条已含。**命令数以本节实测与
> wire-gate 为准**；本文的行号引用以符号名为主，避免重构后漂移。

### 0.1 底座 61 条按域拆分（实测）

来源：`crates/tauron-adapter/src/tauri.rs` 的 `tauron_substrate_handler!` 宏（行号会随重构漂移，以符号名为准）。

| 域 | 条数 | 命令 |
|---|---|---|
| shell | 18 | `host_window_minimize` `host_window_maximize` `host_window_restore` `host_window_close` `host_window_quit` `host_window_relaunch` `host_window_set_position` `host_window_set_size` `host_clipboard_write` `host_clipboard_read` `host_deep_link_register` `host_dialog_open` `host_dialog_save` `host_dialog_message` `host_dialog_confirm` `host_market_check` `host_market_download` `host_market_install` |
| ipc（事件总线） | 7 | `host_events_publish` `host_events_subscribe` `host_events_unsubscribe` `host_events_drain` `host_events_approve` `host_events_revoke` `host_events_approvals`（后三条为 1.1 新增的主窗特权审批面） |
| i18n | 6 | `host_i18n_t` `host_i18n_t_params` `host_i18n_set_locale` `host_i18n_load` `host_i18n_stats` `host_i18n_cleanup_plugin` |
| notify | 3 | `host_notify` `host_notifications_list` `host_notifications_read` |
| settings | 4 | `host_settings_get` `host_settings_set` `host_settings_adopt_legacy` `host_settings_migrate` |
| recovery | 2 | `host_recover_boot` `host_recover_report` |
| menu（R9） | 3 | `host_menu_set` `host_menu_popup` `host_menu_reset` |
| tray（R9） | 3 | `host_tray_create` `host_tray_set_menu` `host_tray_remove` |
| fs（R9，允许根内 I/O） | 6 | `host_fs_read` `host_fs_write` `host_fs_list` `host_fs_stat` `host_fs_mkdir` `host_fs_remove` |
| http（R9） | 1 | `host_http_request` |
| updater（R9，接 `tauron-distribute`） | 2 | `host_updater_check` `host_updater_status` |
| theme（接 `tauron-theme`） | 3 | `host_theme_list` `host_theme_get` `host_theme_set` |
| brand | 1 | `host_brand_info` |
| 能力协商 | 1 | `host_capabilities` |
| 生产就绪自检（1.1） | 1 | `host_production_doctor`（只读诊断，主窗专属） |
| **合计** | **61** | |

> **每域的真实性口径（不美化）**：`menu` / `tray` 在 `tauri` feature 下由
> `TauriMenuSink` / `TauriTraySink` 做**真实现**；点击回传走 Tauri 事件通道
> （`AppHandle::emit` + 前端 `listen(MENU_CLICK_TOPIC)`，路由表是
> `crate::MenuRouteTable`），**不是** `host_events_*` 总线；两条 lane 各存一份路由，
> `host_menu_reset` 不会清掉托盘的路由（轮 37）。仓库内的接收方是
> `examples/minimal-app` 第 9 节；`fs` 是 `std::fs` 真实现，但
> **必须以 `AdapterConfig::with_fs_roots` 配好允许根**（空 = 该域返回
> `UnsupportedBody`）；`updater` 接 `tauron-distribute`，需 `EndpointClient` 注入，
> 缺省无端点故如实降级；`http` **是诚实降级**——`reqwest` 的 TLS 后端不在离线依赖
> 闭包内，只留可注入的 `HttpSink`；`theme` 用 `tauron-theme` 的内置 light/dark 注册表；
> `brand` 由环境变量 `TAURON_BRAND_CONFIG_JSON` / `TAURON_BRAND_CONFIG` 提供。
> 这些可用性会由 `host_capabilities` 的 `families` / `unsupported` **运行期推导**并
> 如实落位（见 §4）。

> 注意：`host_recover_trial_enable` **不在**底座集合里——它要读注册表里插件的当前状态，
> 属插件运行时域；`host_window_create` 同理（要查注册表确认插件存在），而
> `host_window_relaunch` **在**底座集合里（只需底座状态）。踩这条坑的代价是
> 「底座宿主白拿插件命令面」，已由 wire-gate 挡住。

### 0.2 插件运行时 22 条（档 3 才注册）

来源：`crates/tauron-adapter/src/tauri.rs` 的 `tauron_plugin_handler!` 宏中**不属于**底座集合的那些（行号会漂移）。

| 组 | 条数 | 命令 |
|---|---|---|
| 生命周期 / 调用 | 4 | `host_lifecycle_report` `host_plugin_call` `host_call_end` `host_cancel` |
| 注册表 | 3 | `host_registry_list` `host_registry_list_all` `host_registry_admin` |
| contributes | 3 | `host_contributes_register` `host_contributes_list` `host_contributes_reconcile`（对账声明 vs 注册，0.4-W3） |
| 恢复（试验启用） | 1 | `host_recover_trial_enable` |
| 流式（**成组**） | 4 | `host_stream_open` `host_stream_write` `host_stream_grant`（有界 byte credit 补充，V4） `host_stream_close` |
| 进程插件运行时（**成对**） | 2 | `host_runtime_spawn` `host_runtime_health` |
| 窗口（R8） | 1 | `host_window_create`（必须查注册表确认 `plugin-<id>` 存在，故绑 `PluginRuntimeState`；对应的 `host_window_relaunch` 只需底座状态，因此它在**底座集合**里） |
| 资源诊断（M8） | 1 | `host_resource_stats`（主窗专属，返回全局与逐插件配额占用） |
| 跨主体调用（0.4-A1，**成组**） | 3 | `host_call_plugin` `host_call_result` `host_call_take`（发起 / 执行方回填 / 发起方取件） |
| **合计** | **22** | |

> 成组/成对不是排版：只注册流式三命令中的一两条会让流无法开或无法终结，只注册
> `host_runtime_spawn` 而没有 `host_runtime_health` 则永远发现不了 sidecar 崩溃。
> 两条约束都由 wire-gate 锁死（`packages/tauron-contract-tests/src/wire-gate.test.ts`）。

### 0.3 计数方法（可复现）

PowerShell（在仓库根执行；数的是两个宏体内 `$crate::tauri::host_*` 的条数）：

```powershell
# 按宏名解析（不写死行号，抗并发重构）：数两个宏体内 $crate::tauri::host_* 的条数
$src = Get-Content crates\tauron-adapter\src\tauri.rs -Raw
foreach ($m in 'tauron_substrate_handler','tauron_plugin_handler') {
  $i = $src.IndexOf("macro_rules! $m"); $end = $src.IndexOf("`n}`n", $i)
  $n = ([regex]::Matches($src.Substring($i, $end - $i), '\$crate::tauri::host_')).Count
  "$m = $n"
}
# 输出（2026-10-01 复核；R9 五域 + 品牌/主题 + 1.1 审批/自检命令接通后）：
# tauron_substrate_handler = 61
# tauron_plugin_handler = 85   ← 宏体内 83 + 2 条 plugin-install（cfg-gated，宏体带 #[cfg]）
# 注：`plugin-install` 是 **opt-in**（`crates/tauron-adapter/Cargo.toml` 的 `default = []`，
# V4 minimal-substrate 规则），故**默认构建实际注册 83 条**；接入方显式
# `features = ["plugin-install"]` 才是 85 条。宏体计数法数的是**源码文本**，
# 天然包含 cfg-gated 的 2 条，所以它给出的是「开启态」上限，别读成默认值。
# （1.0-W6 曾把它进默认特性，1.1 已改回 opt-in。）
```

另有**门禁**持续守住这两个数字之间的关系（不靠人眼）：
`wire-gate.test.ts:318-331` 解析两个宏的命令清单，断言
①底座集合 ⊂ 全量集合、②底座集合不含插件域命令、③全量集合 == `tauri.rs` 中全部
`#[tauri::command]` 定义；`wire-gate.test.ts:1504-1507,1586-1591` 断言两组 handler
都经同一个 `origin_gated_handler` 包裹。运行期可用性证据：
`crates/tauron-adapter/src/lib.rs` 的单测 `substrate_only_state_serves_base_commands`
（连同 `plugin_runtime_shares_one_substrate_and_injects_sink`）。

---

## 1. 档 1：只底座（61 条）

「不跑插件运行时」的宿主：harness 壳、单窗口工具、只有窗口/剪贴板/对话框/事件/
设置/恢复/i18n/通知/菜单/托盘/文件系统/HTTP/更新通道/主题的桌面客户端。

### 1.1 依赖（`src-tauri/Cargo.toml`）

```toml
[dependencies]
tauri = { version = "2", features = ["wry"] }
# 只取应用层适配器；tauron-adapter 的 default 只有 "plugin-install"（无 "tauri"），
# 必须显式开 "tauri"。档 1/2 不需要安装链路，故关掉默认特性（否则会连带
# plugin-install → tauron-acl / tauron-market 与 2 条 install 命令）
tauron-adapter = { path = "../../crates/tauron-adapter", default-features = false, features = ["tauri"] }

[build-dependencies]
tauri-build = { version = "2", features = [] }
```

> 只需 `tauron-adapter` 一个 tauron crate：`tauron-host` / `tauron-i18n` /
> `tauron-notify` / `tauron-recovery` 都是它的（传递）依赖，宿主**不必**重复声明。
> 需要直接用 `RegistryConfig` 等类型时再加 `tauron-host`（档 3 常见）。

### 1.2 Rust 装配（main.rs）

```rust
use tauri::Manager;
use tauron_adapter::{AdapterConfig, SubstrateState};

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let cfg = AdapterConfig {
                // 生产宿主必须传数据目录，否则崩溃检测只有内存态（进程一退计数器即失，
                // 安全模式永不触发）。缺省 None，仅单元测试可用。
                recovery_data_dir: app.path().app_config_dir().ok(),
                ..AdapterConfig::default()
            };
            // 底座装配：事件总线 / 设置 / i18n / 通知 / 恢复（含落盘判定）都在这一份状态里。
            app.manage(SubstrateState::with_adapter_config(&cfg));
            Ok(())
        })
        // 61 条底座命令：未注册即不可达（插件命令面不存在）
        .invoke_handler(tauron_adapter::tauron_substrate_handler![])
        .run(tauri::generate_context!())
        .expect("failed to run");
}
```

核对的 API（都存在，签名以此为准）：

- `tauron_adapter::SubstrateState` / `tauron_adapter::AdapterConfig` —— 均为 crate 根的 `pub struct`（`crates/tauron-adapter/src/lib.rs`）
- `SubstrateState::with_adapter_config(cfg: &AdapterConfig) -> Self` —— `crates/tauron-adapter/src/lib.rs`
- `tauron_adapter::tauron_substrate_handler![]` —— `#[macro_export]`，`crates/tauron-adapter/src/tauri.rs`（宏名 `tauron_substrate_handler`）
- `AdapterConfig::default()` —— 派生 `Default`。字段以 `crates/tauron-adapter/src/lib.rs`
  的结构体为准（16 个 `pub` 字段，别在这里数）；档 1 真正会用到的是
  `registry` / `recovery_data_dir` / `required_plugins` / `origin_allowlist`，
  Production 另需 `main_window_labels`（可信主窗 label，留空 = `["main"]`）
  与安全事实位（`caller_identity_policy_enabled` / `admin_audit_available` 等）

> ⚠️ **不要**在档 1 用 `state_init()` / `init()`：它们注册的是 `PluginRuntimeState`
> （含注册表），与「一分插件状态都不建」相矛盾；两者**互斥**，同时 `manage` 会
> 因重复注册而 panic（见 [app-layer-wire.md §1](../architecture/app-layer-wire.md)）。

**本节代码是有可运行证据的**（轮 12 补，验收标准 1）：仓库内
`examples/minimal-app/src-tauri` 带一个 `substrate-only` feature，其 `main()` 在
该 feature 下就是上面这段装配（只 `manage(SubstrateState)` + 底座命令族，
不注册任何插件命令），可以直接跑：

```bash
cargo check --manifest-path examples/minimal-app/src-tauri/Cargo.toml \
  --features substrate-only --all-targets      # 0 警告
```

配套的功能性证据在 `crates/tauron-adapter/src/lib.rs` 的
`substrate_only_host_is_functionally_complete`：只用 `SubstrateState`，逐域各打一次
真实调用（shell 窗口命令 → Sink、settings 读写、i18n 装载+切语言+取词、notify
写入+读回、recovery 启动载荷+上报、事件总线发布、brand 形状）——证明"底座不是空壳"，
而不只是"能编译"。

> ⚠️ **ACL 提醒**：即使只底座，宿主也必须有一份 capability 文件（Tauri v2：不匹配
> 任何 capability 的 webview 完全没有 IPC 访问）。示例里的
> `capabilities/default.json` 授予 `core:default` 并覆盖 `main` 与 `plugin-*`。

### 1.3 TS 侧（前端）

```ts
import { HostClient, ShellClient } from '@tauron/host';
import { TauriBackend } from '@tauron/host/tauri';

// root 注册 = 裸命令名 → 前缀必须清空（默认前缀是 'plugin:tauron|'）
const backend = new TauriBackend({ commandPrefix: '' });
const host = new HostClient({ backend });   // 插件调用面（档 3 才有意义）
const shell = new ShellClient({ backend }); // 底座面：settings/i18n/notify/recovery/brand
```

核对的 API：`TauriBackend` 的构造参数 `{ commandPrefix?: string }`
（`packages/tauron-host/src/tauri-backend.ts` 的 `constructor(options)`，默认
`'plugin:tauron|'`）；`HostClient({ backend })`（`packages/tauron-host/src/host.ts`
的 `export class HostClient`）；`ShellClient({ backend })`
（`packages/tauron-host/src/shell-client.ts` 的 `export class ShellClient`）。

UI 取 **`@tauron/ui-primitives`**（只依赖 `@tauron/shell-events`），
**不要**取 `@tauron/ui`——后者的 `PluginManagerStore` 走 `host_registry_list/admin`
（插件运行时域），档 1 拿到会 `command not found`
（`packages/tauron-ui/src/index.ts:7-14`、`packages/tauron-ui-primitives/README.md`）。

### 1.4 命令面

**61 条**（§0.1 的域拆分）。`tauron_plugin_handler!` 的 22 条全部不可达。

### 1.5 会失去什么能力

- **插件运行时全部 22 条**：插件注册表（列表/启停/卸载）、`host_plugin_call` 调用、
  contributes 注册、流式调用、进程 sidecar 的启动与健康探测、安全模式下的试验性启用。
  前端调用这些命令得到 `command not found`（未注册即不可达，正是期望行为）。
- **`@tauron/ui` 与插件管理 UI**：只能用原语包。
- **i18n 回退链 / 缺失键计数、通知中心的环形缓冲与未读计数**：命令在（61 条里），
  但本档的语义是「不消费」——不装载语言包、不开通知读端，等于没有这两块能力。
- **`host_recover_trial_enable`**：安全模式下无法试验性启用某个插件（它属插件域）。

---

## 2. 档 2：底座 + i18n + notify（61 条）

**Rust 侧的依赖与装配与档 1 逐字相同**（命令面也相同，原因见 §0 的粒度说明）。
本档的增量在**真的把这两个域用起来**：

### 2.1 i18n：装载语言包 + 切换语言

```ts
// 装载（pluginId 缺省 = 宿主自己的文案；传 pluginId 时自动加 `plugin:<id>.oc.` 前缀）
await shell.i18nLoad('zh-CN', { 'app.title': 'Tauri 客户端', 'app.ok': '确定' });
await shell.i18nLoad('en-US', { 'app.title': 'Tauri Client', 'app.ok': 'OK' });

await shell.i18nSetLocale('zh-CN');          // 语言状态单一来源
const text = await shell.i18nT('app.title'); // 缺失时返回 key 本身并计入缺失计数
const stats = await shell.i18nStats();       // 缺失键可观测性
```

> 命名空间：`i18nLoad(locale, entries, pluginId?)`，传 `pluginId` 时每个 key 自动加
> `plugin:<id>.oc.` 前缀（`packages/tauron-host/src/shell-client.ts:483-489`，
> Rust 侧 `crates/tauron-adapter/src/tauri.rs:902`）。条目值必须是字符串，否则整次
> 调用被拒（`E_INVALID_MANIFEST`，不做部分生效）。

### 2.2 notify：写入 + 读端配对

```ts
await shell.notify('com.example.app', '安装完成', '插件已更新到 1.2.0');
const center = await shell.notificationsList(50); // { unread, total, items }
await shell.notificationsRead('notif-id');        // 缺省 = 全部已读
```

> `host_notify` 写入 `tauron_notify::NotifyStore`（环形缓冲），
> `host_notifications_list` 是它的读端——两者必须成对使用，只写不读会让未读计数只增不减
> （`crates/tauron-adapter/src/lib.rs` 的 `cmd_notify` / `cmd_notifications_list`）。
> **系统通知派发（`DispatchSink`）已接线到事件与 OS 两层**：`cmd_notify` 按注入的
> `notify_sink` 分支调 `tauron_notify::dispatch`，真实装配在
> `command_state_with_dir_and_config` 里 `notify_sink.set(TauriDispatchSink::new(app))`。
> `send` 将通知作为 **不含正文**的信号 `emit` 到 `NOTIFICATION_TOPIC`（正文只从
> `host_notifications_list` 按调用方身份过滤后提供），轮 36 起 `ShellController` 就是那条
> 信号的接收方；系统通知气泡仍**降级**（`tauri-plugin-notification` 不在依赖闭包，
> `send` 返回 `Ok(false)`）。展开说明见 §5 表格「系统通知派发」同名行。

### 2.3 会失去什么能力

相对档 3 仍然失去插件运行时的 22 条（见 §1.5）；相对档 1 **不失去任何命令**——
本档是「把已经付过编译代价的 6 + 3 条命令真的用起来」。

---

## 3. 档 3：再加插件运行时（83 条）

多插件客户端：需要注册表、插件调用、contributes（命令面板/设置页）、流式调用、
进程插件 sidecar。

### 3.1 依赖（`src-tauri/Cargo.toml`）

与档 1 相同即可；如需直接用 `RegistryConfig` 装配注册表上限，再加一行：

```toml
tauron-host = { path = "../../crates/tauron-host" }
```

> 进程插件（sidecar）**不需要**额外声明 `tauron-proc`：它已是 `tauron-adapter`
> 的依赖（`crates/tauron-adapter/Cargo.toml:22`），且 sidecar 的启停只经
> `host_runtime_spawn` / `host_runtime_health` 两条命令。

### 3.2 Rust 装配：两种注册形态（二选一）

**形态 A：root 注册（零配置，裸命令名）** —— 与 `examples/minimal-app` 一致：

```rust
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        // 注册两份 managed 状态（底座 + 插件运行时，共享同一份底座 Arc）
        .plugin(tauron_adapter::tauri::state_init())
        // 83 条 host_* 命令（= 底座 61 + 插件运行时 22），root 注册
        .invoke_handler(tauron_adapter::tauron_generate_handler![])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                // 窗口销毁 → 回收该插件的订阅/队列与 pending 调用
                let state = window.state::<tauron_adapter::CommandState>();
                tauron_adapter::tauri::cleanup_closed_window(&state, window.label());
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run");
}
```

**形态 B：插件注册（`plugin:tauron|*` 路由，生产推荐）**：

```rust
use tauron_adapter::AdapterConfig;

let cfg = AdapterConfig {
    // origin 允许清单 / 注册表上限 / 必需插件 / 恢复数据目录都在这里；
    // 只配需要的字段，其余用 default。
    ..AdapterConfig::default()
};

tauri::Builder::default()
    // 含 capability 检查：必须为 `tauron` 插件配置权限，否则 `plugin:` 命令被运行时拒绝
    .plugin(tauron_adapter::tauri::init_with_adapter_config(cfg))
    .run(tauri::generate_context!())
    .expect("failed to run");
```

核对的 API（全部存在）：

| API | 位置（符号名为准，行号会随重构漂移） |
|---|---|
| `tauron_adapter::tauri::state_init()` / `state_init_with_adapter_config(cfg)` | `crates/tauron-adapter/src/tauri.rs`（注册两份状态，插件名为 `tauron-state`） |
| `tauron_adapter::tauri::init()` / `init_with_adapter_config(cfg)` | `crates/tauron-adapter/src/tauri.rs`（插件名为 `tauron`，含 invoke_handler = 83 条；`plugin-install` 默认开启时 85 条） |
| `tauron_adapter::tauron_generate_handler![]` / `tauron_plugin_handler![]` | `crates/tauron-adapter/src/tauri.rs`（后者为 83/85 条真身，前者是别名） |
| `tauron_adapter::CommandState` | `crates/tauron-adapter/src/lib.rs` 的类型别名（= `PluginRuntimeState`，含注册表） |
| `tauron_adapter::tauri::cleanup_closed_window(&state, label)` | `examples/minimal-app/src-tauri/src/main.rs` 使用 |
| `PluginRuntimeState::with_substrate(Arc<SubstrateState>, AdapterConfig) -> Result<Self, AssemblyError>` | `crates/tauron-adapter/src/lib.rs`（手动装配时用；测试须用 `with_spawner`。**同一底座的第二次装配返回 `Err(AlreadyAssembled)`，拿不到第二个 Runtime**——轮 22 / V7-P1-01） |

> **两种形态互斥**：同时用会在 setup 阶段以 `AssemblyError::AlreadyManaged` 结构化失败
> （[app-layer-wire.md §1](../architecture/app-layer-wire.md)；此前是重复
> `manage::<PluginRuntimeState>` 的运行时 panic）。
> `AdapterConfig` 是唯一能配 `origin_allowlist` / `main_window_labels` / `registry` /
> `required_plugins` / `recovery_data_dir` 的入口；缺省入口等价于 `AdapterConfig::default()`。
> ⚠️ **Production 下 `origin_allowlist` 不得为空**：空清单=origin 门未装弹，
> 装配阶段就被 `ORIGIN_GATE_ARMED_REQUIRED` 拒绝（轮 10）。`main_window_labels`
> 留空展开为 `["main"]`；多窗宿主里**其它**想调宿主命令的窗口必须显式声明，
> 否则在 Production 会被 `MAIN_WINDOW_LABEL_NOT_DECLARED` 拒掉。
> ⚠️ `examples/minimal-app/src-tauri/src/main.rs` 与
> `Cargo.toml` 的注释此前写的「45 条」是**过时注释**，轮 11 已就地修正为 **54 条**；此后经 R9 五域（menu/tray/fs/http/updater）与品牌/主题接通，当前命令总数为 **底座 61 / 全量 83**（= 底座 + 插件运行时 22；1.1 向底座进 Event 审批 3 条 + 生产就绪自检 1 条，向插件运行时进流式 credit 1 条）；`plugin-install` feature 另注册 2 条，且该 feature 是 **opt-in**：`crates/tauron-adapter/Cargo.toml`
> 的 `default = []`（1.1 的 V4 合并按 minimal-substrate 规则把它从 1.0-W6 的默认特性里改了回来），
> 所以只开 `tauri` 命令桥的宿主是 **83 条**，显式 `features = ["plugin-install"]` 才是 **85 条**。
> 示例应用属显式开启那一类（`examples/minimal-app/src-tauri/Cargo.toml` 的
> `default = ["plugin-install", "runtime-wasm-broker"]`），因此它的安装包注册 85 条
> （doc 里记录过的口径漂移已清零，不再只是"标注过时"）。

### 3.3 TS 侧

```ts
import { AdminClient, HostClient, ShellClient } from '@tauron/host';
import { TauriBackend } from '@tauron/host/tauri';

// 形态 A（root 注册）：前缀清空
const backend = new TauriBackend({ commandPrefix: '' });
// 形态 B（插件注册）：用默认前缀 'plugin:tauron|'

const host = new HostClient({ backend });
const registry = await host.registryList();   // scoped-read：只列可见插件
const admin = new AdminClient({ backend });   // privileged：仅主窗可用
await admin.registryAdmin({ op: 'enable', id: 'com.example.plugin' });

// 轮 43（A83）：uninstall/purge 在服务端要求「预览→确认→带令牌提交」。
// 预览（preview: true）无副作用，返回 kind: 'review' 与一次性令牌；提交时宿主重核
// 插件/操作/预览版本——预览后版本漂移即拒（须重新预览）。UI 层 PluginManagerStore
// 与 ShellController 已内置这条链（含无 DOM 失败关闭）；直连 AdminClient 的接入方
// 需自行实现：不提交令牌即自然作废，取消与失败请按接入方语义区分。
const review = await admin.registryAdmin({ op: 'uninstall', id: 'com.example.plugin', preview: true });
if (review.kind === 'review' && (await confirmUninstall(review))) {  // confirmUninstall：接入方自己的确认面
  await admin.registryAdmin({ op: 'uninstall', id: 'com.example.plugin', reviewToken: review.reviewToken });
}

const shell = new ShellClient({ backend });
// profile 是 RuntimeSpawnProfile：signature / binaryHash / abi 是**必填**验签材料
// （宿主不会替调用方造 hash——那等于把未验证的二进制伪装成已验证）。
const handle = await shell.runtimeSpawn('com.example.plugin', profile); // → { pid, lease }
const health = await shell.runtimeHealth(handle.lease);                 // → { alive, crashes, … }
```

核对的 TS API（以符号名为准，行号会漂移）：`HostClient#registryList(scope?)`；
`AdminClient({ backend })#registryAdmin(op) -> RegistryAdminOutcome`（判别联合
`kind: 'executed' | 'review'`；破坏性操作须先后带 `preview` 与 `reviewToken`，见
`packages/tauron-host/src/events.ts`）；
`ShellClient#runtimeSpawn(pluginId, profile) -> RuntimeHandle{pid, lease}` /
`#runtimeHealth(lease) -> RuntimeHealth{alive, status, pid, crashes, consecutiveFailures, reap, health}`
（V4 三态 `status`：`unknown` **不消耗**崩溃预算；`health` 是 A103 的规范化
存活/就绪报告，见 `packages/tauron-host/src/shell-client.ts`）。
1.1 起 `AdminClient` 另有 `eventsApprove` / `eventsRevoke` / `eventsApprovals` 与
`productionDoctor()`，用法见 `docs/api/plugin-development-guide.md`。

**但**：`host_runtime_spawn` 只接受 `process` 型插件，其他类型返回
`E_PLUGIN_TYPE_NO_RUNTIME`（**诚实失败**，不伪造 pid，
`crates/tauron-adapter/src/lib.rs` 的 `cmd_runtime_spawn`）；js/rust/wasm 插件目前
**没有执行器**（见 §5）。

### 3.4 命令面

**83 条**（§0.1 的 61 + §0.2 的 22）；`plugin-install` 默认开启时 **85 条**（+2 条 install）。

### 3.5 会失去什么能力

相对档 1/2 没有失去——它是全集。反过来要清楚**档 1/2 失去的 22 条**正是插件的
生命周期与调用面：没有它们，`@tauron/ui` 的插件管理器、contributes 驱动的命令面板/
设置页、流式调用、sidecar 都无从谈起。

---

## 4. 三档能力对照

| 能力 | 档 1 只底座 | 档 2 + i18n/notify | 档 3 + 插件运行时 |
|---|:--:|:--:|:--:|
| 窗口 / 剪贴板 / 对话框 / 深链接 | ✅ 18 条 shell 命令（其中 market 3 条是桩） | ✅ | ✅ |
| 事件总线（三通道 + 审批） | ✅ 7 条（1.1 含审批 3 条） | ✅ | ✅ |
| 设置读写 | ✅ 2 条（`host_settings_get/set`；底层正从扁平 KV 换成 `SettingsStore`，见 §5） | ✅ | ✅ |
| 崩溃恢复（三级降级 + 跨进程标记） | ✅ 2 条（进程插件崩溃检测为轮询式） | ✅ | ✅ |
| i18n 命令面 | 已注册（6 条），本档不消费 | ✅ 消费 | ✅ |
| 通知命令面 | 已注册（3 条），本档不消费 | ✅ 消费（系统派发未接线） | ✅ |
| 插件注册表 / 调用 / contributes / 流式 / sidecar | ❌ 22 条不可达 | ❌ | ✅ |
| `@tauron/ui`（插件管理器） | ❌（用 `ui-primitives`） | ❌ | ✅ |
| **实测命令数** | **61** | **61** | **83**（默认含 install 为 85） |

---

## 5. 诚实披露：已经接线 vs 尚未接线

**已接线**（有实现 + 有测试 + 命令体真的调它）：生命周期状态机、三档授权 + origin 门、
事件总线、i18n、通知存储（环形缓冲）、恢复引擎与落盘判定、通知读端、
进程 sidecar 的**启动/探测/终止**、per-plugin 崩溃预算、
**设置四层合并 / schema 版本迁移**（`SubstrateState.settings` 是
`tauron-settings::SettingsStore`，`Cargo.toml` 已声明该依赖，
`host_settings_migrate` 已注册成命令）。

**尚未接线**（不要按「有 crate 就等于能用」规划）：

| 能力 | 现状 | 证据 |
|---|---|---|
| sidecar 的 stdin/stdout JSON-RPC 帧回路 | **已接线，轮 22 起有运行期证据**（V7-P1-05）：`Stdio::piped()` + 每条进程一个 stdout 读线程 + `ProcessFrameSinkImpl` → `Registry::settle_call`；真 sidecar 夹具 `crates/tauron-test-sidecar`（`publish = false`，永不上架）的 E2E 真起进程验证收帧 / 回帧 / 结算 / 禁用时真杀进程 | `crates/tauron-proc/src/spawner.rs` 的 `CommandSpawner`、`crates/tauron-adapter/src/process_delivery.rs`、`crates/tauron-test-sidecar/tests/sidecar_e2e.rs` |
| 进程组 / 作业对象 / kill 树、空闲超时 kill | 前三者**已实现**（Unix/macOS 独立 process group、Windows `KILL_ON_JOB_CLOSE` Job Object、`CommandSpawner` 丢弃时逐个 tree-aware `kill`；两 provider 自报 `Partial`，无 fs/net/syscall 隔离，Production 因此拒接 `ProcSpawner`）；**空闲超时 kill 仍未实现** | `crates/tauron-proc/src/spawner.rs` 的 `UnixProcessGroupSandboxProvider` / `WindowsJobObjectSandboxProvider` / `impl Drop for CommandSpawner` |
| 进程崩溃检测 | **轮询式**（无后台监控线程；不被调用的 `host_runtime_health` 不会发现死亡） | `crates/tauron-adapter/src/lib.rs` 的 `cmd_runtime_health` 文档注释 |
| wasm 运行时 | **执行层已落地（轮 22 / V7-P0-01）**：`tauron-wasm` 依赖真引擎 `wasmi = "2.0.0"`（`src/provider.rs`），`WasmEngine::execute` 必须走「活跃代际模块 → 导出检查 → 代次键实例池 → `provider.invoke` 真执行」，模块未加载即 `ModuleNotLoaded` 硬失败，绝不伪造 `success`；**但适配层投递仍未接它**——opt-in `runtime-wasm-broker` 下的 `wasm_delivery.rs` 只做配置校验与崩溃预算，执行回执恒 `delivered:false` 如实标注，命令面对 wasm 仍回 `E_PLUGIN_TYPE_NO_RUNTIME`；host_fn ABI 目前只支持整数参数（无字符串/线性内存传递） | `crates/tauron-wasm/src/provider.rs`、`crates/tauron-adapter/src/wasm_delivery.rs`；`crates/tauron-wasm/Cargo.toml` |
| 市场监管 / 更新链 | 商城是**桩**：`host_market_check` 恒 `{available:false}`，download/install 恒 `{simulated:true}`；**更新通道已接线**（轮 13 更正）：`host_updater_check` / `host_updater_status` 真跑 `tauron_distribute::check_for_update`（灰度 4 阶段 + 签名 + 崩溃门禁），`InstallationIdentity::load_or_create` 落盘；未注入 `EndpointClient` 时 fail-closed。**轮 29 补上缺的最后一公里**：对外 SDK 的 `AutoUpdateClient.checkUpdate()` 过去读的是那张桩表（宿主接了真端点也报"无更新"），现在改读 `host_updater_check`，`currentVersion` 由接入方在配置里声明。**轮 31 补上壳层那条腿**：`<oc-updater-dialog>` 的「检查更新」经 `ShellController` 同样读桩、且把返回值整个丢掉，现在它也打 `host_updater_check`，判定与 SDK 共用导出的 `toUpdateInfo`，结果经新 option `onUpdaterCheck` 交回接入方（`currentVersion` 同样必给，缺则一个命令都不发）。**轮 32 收口这条链的状态词表**：对话框主按钮派发什么不再由组件里的 `if/else` 字面量决定，而由 `@tauron/shell-events` 的 `UPDATER_STATUSES` + `UPDATER_PRIMARY_ACTION` 推导——`ready` 是**唯一**点亮「立即重启」（`oc-restart` → `host_window_relaunch`）的状态；此前组件的重启分支等 `'done'` 而客户端的终态叫 `'ready'`，那条腿在真装配里点不出来。**轮 33 补这条链的读侧**：`host_updater_status`（灰度百分比 / 崩溃门禁 / 宿主进程内账本）此前在 TS 侧零消费者，`available: false` 的四件事实在 UI 上都是同一句"没有更新"。现在 SDK 的 `checkUpdate()` 与壳层的检查按钮共用同一个 `enrichUpdaterInfoWithChannel`，把通道事实并进 `info.reason`，示例 `describeUpdate()` 在"没有更新"分支把它打印出来。账本另加线字段 `stateSimulated`（Rust `state_simulated`）与 `state` **成对读**：`update_state` 现有写入方是 `host_market_download` / `host_market_install` 两条桩，只看 `state` 就会把"点了一下模拟安装"说成"已安装 2.0.0"。诊断是附加读数——不改 `available`/`degraded` 判定，命令缺席 / 报错 / 答空都原样返回结论，有更新时不多打这条命令；加的是返回字段不是新命令，85 条命令面不变。**轮 54 补上这条链的降级门禁**：`tauron-distribute` 此前没有任何版本序比较，一条目标版本低于已安装版本的清单会被当正常升级走完 download→extract→swap；现在 `UpgradeRunner::validate()` 与装配腿 `DistributeUpgradeInstaller::select_manifest` 共用 `ensure_not_downgrade`（版本序复用 `tauron-market` 的 `cmp_version`/`is_downgrade`，不自造第二套），拒绝发生在**任何字节落地之前**、零文件系统副作用，对外报 `E_INVALID_MANIFEST`；无版本基线（`installed_version` 缺席）时如实放行，不猜版本。 | `crates/tauron-adapter/src/lib.rs` 的 `cmd_market_check` / `cmd_updater_check` / `DistributeUpdaterSink` |
| 品牌运行期信息 | **已接线**（轮 13 更正：此前写「恒 `{}`」——判定与形状都错）：配置 env 来源后 `cmd_brand_info` 走 `tauron-brand` 校验链返回 `BrandInfo`；未配置如实返回 `UnsupportedBody{supported:false}`；构建期品牌 CI 矩阵是另一条真实链路 | `crates/tauron-adapter/src/lib.rs` 的 `cmd_brand_info` / `load_brand_config_from_env` |
| 权限授予 / 审批（安装路径） | **已接线**：`tauron-acl` 是 `plugin-install` 的可选依赖，`draft_grant_set` / `validate_grants` / `AclStore` 与 HMAC 封存都在真实安装链路里被调用 | `crates/tauron-adapter/Cargo.toml` 的 `plugin-install`；`crates/tauron-adapter/src/lib.rs` 的 `registry_install_preview_inner` / `registry_install_inner` |
| 审批结果**物化为 Tauri Capability** | **未接线**：`AclStore::to_capability` 只有库内 API 与形态单测，宿主从不调 `add_capability`——运行时权限边界实际由代码层身份判定 + Tauri capability 清单（构建期静态）承担 | `crates/tauron-acl/src/grant.rs` 的 `to_capability` |
| 双世界沙箱（QuickJS-WASM）、Shell 矩阵 | **参考实现**（模拟返回） | `packages/tauron-dual-world/src/sandbox.ts:134`、`packages/tauron-shell-matrix/src/manager.ts` 的 `simulateShellStart` / `simulated` 判据字段 |
| 系统通知派发（`DispatchSink`） | **已接线到 OS 与事件两层，OS 气泡仍降级**：`cmd_notify` 按注入的 `notify_sink` 调 `tauron_notify::dispatch`；真实 Tauri 装配里 `command_state_with_dir_and_config` 会 `notify_sink.set(TauriDispatchSink::new(app))`——**「仓库内没有任何 `DispatchSink` 实现、`notify_sink` 无人注入」这句是旧事实，本轮删除**（它一直与 `tauri.rs` 的实现和 R7-3 门禁「DispatchSink 必须有 Tauri 实现」相矛盾）。`send` 做两件真事：向 `NOTIFICATION_TOPIC` `emit` 一条**不含正文**的信号、对警示级以上调 `request_user_attention`；系统通知气泡**没有**（`tauri-plugin-notification` 不在依赖闭包），所以返回 `Ok(false)` = 降级为应用内。**轮 36 补的是接收方**：`emit` 一路 `Ok` 而全仓无人监听那个 topic，`notificationsList()` 也只有自己的单测在调——发出端绿灯、读端孤儿，通知到不了用户且不报错。现在 `ShellController` 订阅 `NOTIFICATION_TOPIC`（TS 线值常量与 Rust `pub const` 由门禁比对）→ 拉 `host_notifications_list` → 经 `onNotification` 上屏，`examples/minimal-app` 是真消费者。 | `crates/tauron-adapter/src/tauri.rs` 的 `TauriDispatchSink` / `NOTIFICATION_TOPIC` / `notification_payload`、`packages/tauron-host/src/host-topics.ts`、`packages/tauron-host/src/shell-controller.ts` 的 `_subscribeNotifications` |
| 孤儿公共 API（整类问题的登记，不是一条能力） | **已登记 + 上门禁（轮 2）**：12 条「文档宣称未接线」的公共 API（`to_capability` / `load_unverified` / `build_approval_rows` / `FaultBoundary::run` / 配置样张三件套 / `trim_to` / `is_monotonic` / TS 的 `validateClientConfig` 族 / 三个 `create*Client` 工厂 + `startAutoCheck`）（`resolve_best` 轮 49、`remove_resource_bundle` 轮 52、`validate_group_key` 轮 53、`is_downgrade` 轮 54 已接线并从台账删除，故不再列名）逐条写明 disposition，并由门禁复核：**未接线宣称必须在产品接线面查不到消费者，已接线反例必须查得到**，另加「不许新增孤儿」棘轮（当前基线 635，收口后应下调）。注意反例的价值——`draft_grant_set` / `validate_grants` / `validate_zip_constants` 看似孤儿（同名符号只在声明文件出现）其实**真在安装链路里被调用**，把「查不到=孤儿」当判据会把好链路判死 | `contracts/orphan-public-api.json`；`scripts/check-orphan-public-api.mjs`（CI 步骤 `Orphan-Public-API（V7 轮 22）`） |

同一份事实的仓库自述见
[overview.md「接线状态（诚实披露）」](../architecture/overview.md)；竞品文档的头条特性
兑现度标记见 [competitive-analysis.md](../competitive-analysis/competitive-analysis.md) §〇。

---

## 附录 A：框架层——只取信封协议（`tauron-shell`，3 条命令）

如果连底座的 61 条都不需要，只要「插件调用信封」（`plugin_invoke` /
`plugin_cancel` / `plugin_emit`），走**框架层**：

```toml
[dependencies]
tauri = { version = "2", features = ["wry"] }
tauron-shell = { path = "../../crates/tauron-shell", features = ["tauri"] }
```

```rust
tauri::Builder::default()
    .plugin(tauron_shell::commands::state_init())
    .invoke_handler(tauron_shell::tauron_generate_handler![])
    .run(tauri::generate_context!())
    .expect("failed to run");
```

- 命令数：**3 条**（`plugin_invoke` / `plugin_cancel` / `plugin_emit`），见
  [overview.md](../architecture/overview.md) 的「两层架构」表。
- > ⚠️ **这一档今天拿不到可运行的调用**，别按「装了就通用」规划：
  > ① 仓库内**没有任何宿主**注册这 3 条——`examples/minimal-app/src-tauri/src/main.rs`
  > 用的是 `tauron_adapter::tauron_generate_handler![]`（应用层），`tauron_shell::…`
  > 那条只存在于**注释**里；
  > ② `PluginDispatcher` 的**生产实现为零**（唯一的 `impl` 是测试用的 `EchoDispatcher`），
  > 未装载分发器时 `plugin_invoke` 一律回 `SC-9001`（`crates/tauron-shell/src/dispatch.rs`
  > 的「装载插件分发器。未装载时 `plugin_invoke` 返回 `SC-9001`」）；
  > ③ CI 会带 `tauri` feature 编译并测试 `tauron-shell`，但那是**库内自测**，
  > 不构成「真实宿主装配」证据。
  > 结论：框架层可用 = 你自己实现并装载 `PluginDispatcher`；在那之前，
  > 三档对照表（§4）里凡涉及框架层的行都按「需自实现」读。
- `tauron-shell` 与 `tauron-adapter` 的应用层插件名不同（`tauron-shell` vs `tauron`），
  因此**可以叠加注册**；两层的线格式由 `@tauron/contract-tests` 锁定。
- 引用出处：`examples/minimal-app/src-tauri/src/main.rs:36-40` 的注释、
  `crates/tauron-shell/src/commands.rs:154`。

---

## 附录 B：规划（尚未实现，不要当作可用）

1. **按域裁剪命令族**：今天只有「底座 61 / 全量 83」两组编译期集合，无法只取
   shell + ipc 或只取 i18n + notify（§0 的粒度问题）。
2. **底座 API 不再能触达注册表**：`SubstrateState` 已不含 registry，但
   `SubstrateState::with_adapter_config` 仍会建恢复/通知/i18n 全量状态；更细的
   按需装配（只建用到的域）仍是 R1 的后续项。
3. **进程插件帧回路、wasm 运行时、更新链**：见 §5 表格，接入时替换命令体/新增命令，
   **不需要改动线协议**（这也是三档能渐进的原因：命令名与形状已由门禁冻结）。
