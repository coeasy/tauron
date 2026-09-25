// Tauri 命令适配层（§4.1 适配器）。
//
// 关键约束（ADR-04）：每个入口 `catch_unwind` → `E_HOST_PANIC`，
// 结构化错误，从不让 panic 抵达 JS。
//
// 本模块**不**依赖 `tauri` crate：命令逻辑保持平台无关、可离线测试。
// `#[tauri::command]` 注册在 feature-gated 的 `tauri.rs` 中（`tauri` feature）。
//
// 集成方式（第三方 Tauri 应用）：
// ```toml
// [dependencies]
// tauron-adapter = { path = "...", features = ["tauri"] }
// ```
// ```rust,ignore
// tauri::Builder::default()
//     .plugin(tauron_adapter::tauri::init())
//     .run(tauri::generate_context!())
// ```

#[cfg(feature = "tauri")]
pub mod tauri;

/// 启动恢复的持久化与崩溃检测（平台无关，纯 std）。
mod recovery;

pub use recovery::{BootRecord, LoadSource, RecoveryStore};

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use tauron_host::{
    authz::RegistryAdminOp,
    eventbus::{ChannelKind, EventBus, Frame, PublishResult, SubscribeOutcome},
    lifecycle::{Event, State as LifecycleStateName, TransitionOutcome},
    manifest::{EventDecl, PluginId, PluginType},
    registry::{PluginSummary, Registry, RegistryConfig},
    runtime::{LeaseReaper, ReapOutcome, ReapStats, RuntimeHandle},
    stream::{StreamFrame, StreamKind},
    guard, PendingCall,
};
// R8 §2：装配器宏的**消费者**要在自己的函数签名里命名这些类型（返回 `HostResult`、
// 匹配 `ErrorCode`），而 `tauron_plugin_as_host_command!` 展开时引用的
// `$crate::tauri::HostResult` 只在 feature `tauri` 下存在。因此在 crate 根重新导出：
// 「写一条宿主形态命令」不必先自己依赖 `tauron-host`，也不必打开某个 feature。
pub use tauron_host::{ErrorCode, HostError, HostResult};
use tauron_i18n::{I18nEngine, ResourceBundle};
use tauron_notify::{dispatch, DispatchSink, NotifyEntry, NotifyKind, NotifyStore};
use tauron_proc::{
    AbiFingerprint as ProcAbiFingerprint, BinarySignature, CrashLimit, CrashTracker, ProcError,
    ProcSpawner, SpawnConfig, current_abi_contract, validate_abi, validate_spawn_config,
};
use tauron_recovery::{BootContextEntry, BootPhase, PluginState as RecoveryPluginState, RecoveryEngine};
use tauron_settings::{Migration, SettingsError, SettingsStore};

/// 贡献注册条目（命令/菜单/面板/设置Tab）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributeEntry {
    pub plugin_id: String,
    pub kind: String,
    pub id: String,
    pub label: String,
}

/// 贡献注册表：管理所有已注册贡献。
#[derive(Debug, Default)]
pub struct ContributesRegistry {
    entries: Vec<ContributeEntry>,
}

impl ContributesRegistry {
    /// 注册贡献。
    pub fn register(&mut self, entry: ContributeEntry) -> HostResult<()> {
        // 检查重复
        if self.entries.iter().any(|e| e.plugin_id == entry.plugin_id && e.id == entry.id) {
            return Err(tauron_host::HostError::new(
                ErrorCode::E_PLUGIN_EXISTS,
                format!("贡献 {} 已存在", entry.id),
            ));
        }
        self.entries.push(entry);
        Ok(())
    }

    /// 获取所有贡献。
    pub fn list_all(&self) -> &[ContributeEntry] {
        &self.entries
    }

    /// 按插件获取贡献。
    pub fn list_by_plugin(&self, plugin_id: &str) -> Vec<&ContributeEntry> {
        self.entries.iter().filter(|e| e.plugin_id == plugin_id).collect()
    }

    /// 按类型获取贡献。
    pub fn list_by_kind(&self, kind: &str) -> Vec<&ContributeEntry> {
        self.entries.iter().filter(|e| e.kind == kind).collect()
    }

    /// 清除插件的所有贡献。
    pub fn clear_plugin(&mut self, plugin_id: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.plugin_id != plugin_id);
        before - self.entries.len()
    }

    /// 贡献数量。
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

/// 适配器装配配置（宿主启动时传入一次）。
///
/// 全部字段都有默认值：`AdapterConfig::default()` = 恢复持久化关闭 + 空必需
/// 集合，即 `CommandState::new` 的既有行为（单元测试不需要磁盘）。
#[derive(Debug, Clone, Default)]
pub struct AdapterConfig {
    /// 注册表配置（上限、TTL、加载过滤器）。`None` = [`RegistryConfig::default()`]。
    pub registry: Option<RegistryConfig>,
    /// 宿主数据目录：恢复标记（崩溃检测）落盘位置。
    ///
    /// 生产宿主应传 Tauri 的 `app.path().app_config_dir()`。为 `None` 时恢复
    /// 引擎只有内存态：进程一退计数器即失，安全模式永不触发。
    pub recovery_data_dir: Option<PathBuf>,
    /// 安全模式/修复模式下仍必须加载的插件 id（引擎的必需集合）。
    ///
    /// 这里是必需性的**权威来源**，每次启动都会用它整体替换引擎里的集合。
    pub required_plugins: HashSet<String>,
    /// **origin 允许清单（R4-D2）**：允许调用 `host_*` 命令的 webview origin。
    ///
    /// 语义（见 [`tauron_host::authz::origin_allowed`]）：
    /// - **空清单 = 不启用**（默认）→ 一律放行，兼容既有装配；
    /// - 非空 = **fail-closed**：宿主侧从 `Invoke` 取真实 webview origin，逐字命中
    ///   清单项才放行；取不到 origin 的调用方一律拒绝。
    ///
    /// 条目应为规范化形式 `scheme://host[:port]`（如 `http://127.0.0.1:63896`）；
    /// 比较前会去掉首尾空白与尾随 `/`。这是多宿主/混淆代理场景的防线：把非官方
    /// origin 的窗口挡在特权命令之外。
    pub origin_allowlist: Vec<String>,
}

/// 命令状态：宿主进程生命周期内的共享状态。
///
/// Tauri 通过 `State<CommandState>` 注入到每个命令处理器。
/// 恢复阶段对账的**插件侧写回口**（底座只依赖本 trait，不依赖注册表）。
///
/// 为什么需要这层：`reconcile_recovery_phase` 做的事横跨两个域——读引擎判定
/// （底座）、把判定补发成注册表事件（插件运行时）。若让它直接拿注册表，底座命令
/// 就必须触达 `registry`，R1 的编译期隔离当场失效；若把它整个划归插件域，
/// `host_recover_report`（宿主上报**自身**启动结果，底座语义）就无法触发对账。
///
/// 于是底座只依赖本 trait：`SubstrateState::plugin_flags` 由**多插件宿主**在装配时
/// 注入，底座-only 宿主保持空——没有插件就没有对账对象，这是语义正确，不是静默失败。
pub trait PluginFlagSink: Send + Sync {
    /// 当前插件快照：`(plugin_id, 是否被安全模式禁用)`。
    fn flag_snapshots(&self) -> Vec<(String, bool)>;
    /// 补发安全模式事件；`Some(illegal)` = 注册表已处理（非法迁移只计数不报错），
    /// `None` = 注册表拒绝该次迁移。
    fn report_flag_event(&self, plugin_id: &str, enter_safemode: bool) -> Option<bool>;
}

// ──────────────────────────────────────────────────────────────────────────
// R8：宿主能力 Sink 抽象（窗口 / 对话框 / 深链接）
//
// **为什么需要这一层**：这三类能力的**平台部分**（Tauri 窗口 API、原生对话框
// 插件、深链接插件）此前直接写在 `tauri.rs` 的 `#[tauri::command]` 包装器里。
// 后果有两条，且都是实测出来的：
//  1. 非 Tauri 宿主（harness 壳 / 底座-only 宿主 / 单测）**无法替换实现**——
//     `lib.rs` 里只能留 `Ok(())` / `Ok(None)` 桩，而「桩」与「真实现」的差别
//     没有任何类型或测试能表达；
//  2. 「命令真的调了平台能力」在单测里**不可判定**：`WebviewWindow` /
//     `AppHandle` 构造不出来，只能做类型证据（见 `tauri.rs` 里既有的
//     `tauri_dispatch_sink_implements_the_trait`）。
//
// 抽象存在的**唯一理由**就是这两条：让宿主与单测注入假实现，并让行为可断言
// （`lib.rs` 的 `sink_tests` 模块里「换 sink 即换命令结果」就是验收标准）。
//
// **注入方式**：三个字段都是 [`SubstrateState`] 上的 `pub Arc<dyn …>`，装配时
// 直接替换（生产注入点是 `tauri.rs` 的 `command_state_with_dir_and_config`）。
// 缺省值是**进程内降级实现**（[`MemoryWindowSink`] / [`NoopDialogSink`] /
// [`NoopDeepLinkSink`]）——它们如实降级，不伪造任何平台行为。
//
// **边界（本轮如实登记）**：Tauri 实现里**只有窗口操作是真实的**；
// 原生对话框与 OS 级深链接注册都是**降级路径**（`tauri-plugin-dialog` /
// `tauri-plugin-deep-link` 不在本仓依赖闭包内，硬约束不许新增依赖）。
// 各自的降级语义写在对应实现的文档注释里，一个字都不美化。
// ──────────────────────────────────────────────────────────────────────────

/// 窗口操作的一次留痕（[`MemoryWindowSink`] 的记录单元）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowOpRecord {
    /// 操作线名：`minimize` / `maximize` / `restore` / `close` / `set_position`
    /// / `set_size` / `quit` / `relaunch` / `create`。
    pub op: &'static str,
    /// 目标 webview label；应用级操作（`quit` / `relaunch`）为 `None`。
    pub label: Option<String>,
    /// 诊断细节（几何数值 / 新窗口 url）；无则空串。
    pub detail: String,
}

/// **窗口能力**（平台部分）。与既有命令面逐一对齐。
///
/// 七条既有操作（`host_window_minimize` / `maximize` / `restore` / `close` /
/// `set_position` / `set_size` / `quit`）加 R8 §3 的两条新操作
/// （`relaunch` / `create`，对应 `host_window_relaunch` / `host_window_create`）。
///
/// `label` 是**目标 webview 的 label**，由 wire 层从真实 `WebviewWindow` 取
/// （不是前端入参）：`minimize` / `close` 等操作的就是**调用方自己**那个窗口，
/// 与 R8 之前包装器里 `window.close()` 的语义逐字一致。sink 是**状态级**对象、
/// 不知道"当前调用来自哪个窗口"，所以目标必须显式传进来。
///
/// **错误码（18 码封闭词表内复用）**：目标窗口不存在、或平台拒绝该操作 →
/// `E_STATE_INVALID_TRANSITION`（词表里没有"平台操作失败"这一类，最贴近的语义是
/// 「该操作在当前状态下不成立」；`deep_link_delivered` 已有同样用法）。宁可如实
/// 报"没做成"，也不新增错误码。
pub trait WindowSink: Send + Sync {
    /// 最小化 `label` 窗口。
    fn minimize(&self, label: &str) -> HostResult<()>;
    /// 最大化 `label` 窗口。
    fn maximize(&self, label: &str) -> HostResult<()>;
    /// 还原 `label` 窗口（从最大化/最小化）。
    fn restore(&self, label: &str) -> HostResult<()>;
    /// 关闭 `label` 窗口。
    fn close(&self, label: &str) -> HostResult<()>;
    /// 把 `label` 窗口移到 `(x, y)`（逻辑像素 / DIP）。
    fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()>;
    /// 把 `label` 窗口缩放到 `width × height`（逻辑像素 / DIP）。
    fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()>;
    /// 退出整个应用（应用级，无目标窗口）。
    fn quit(&self) -> HostResult<()>;
    /// 请求宿主**重启**（应用级，R8 §3）。
    ///
    /// # 返回（诚实口径）
    /// - `Ok(true)` = 已向宿主请求重启（Tauri 实现走 `AppHandle::restart()`）；
    /// - `Ok(false)` = **未实现**：宿主没有重启原语（非 Tauri 宿主 = 降级），
    ///   调用方据此如实上报，**不得**把 `false` 当成"已重启"。
    fn relaunch(&self) -> HostResult<bool>;
    /// 创建 webview 窗口（R8 §3）。
    ///
    /// # 返回（诚实口径）
    /// - `Ok(true)` = 真的创建了窗口；
    /// - `Ok(false)` = **未实现**（非 Tauri 宿主 = 降级），命令据此回
    ///   `created: false`，而不是假装创建成功。
    ///
    /// 目标窗口已存在、或平台拒绝创建 → `Err`（不返回"半态成功"）。
    fn create(&self, spec: &WindowCreateSpec) -> HostResult<bool>;
}

/// 进程内窗口 sink（**缺省实现**）：只留痕，**不操作任何窗口**。
///
/// 用途：非 Tauri 宿主（底座-only / harness 壳）的缺省装配；以及单测里断言
/// 「命令真的调了 sink」（抽象存在的验收标准）。
///
/// ⚠️ **它不是窗口实现的替代品**：不创建、不移动、不关闭、不退出任何东西。
/// 要真实窗口行为必须注入平台实现（`tauri.rs` 的 `TauriWindowSink`）。
#[derive(Debug, Default)]
pub struct MemoryWindowSink {
    ops: Mutex<Vec<WindowOpRecord>>,
}

impl MemoryWindowSink {
    /// 空记录器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次操作。
    fn record(&self, op: &'static str, label: Option<&str>, detail: String) {
        self.ops.lock().push(WindowOpRecord {
            op,
            label: label.map(str::to_string),
            detail,
        });
    }

    /// 已记录的操作（按发生顺序）。
    pub fn ops(&self) -> Vec<WindowOpRecord> {
        self.ops.lock().clone()
    }

    /// 某操作是否发生过。
    pub fn recorded(&self, op: &str) -> bool {
        self.ops.lock().iter().any(|r| r.op == op)
    }
}

impl WindowSink for MemoryWindowSink {
    fn minimize(&self, label: &str) -> HostResult<()> {
        self.record("minimize", Some(label), String::new());
        Ok(())
    }

    fn maximize(&self, label: &str) -> HostResult<()> {
        self.record("maximize", Some(label), String::new());
        Ok(())
    }

    fn restore(&self, label: &str) -> HostResult<()> {
        self.record("restore", Some(label), String::new());
        Ok(())
    }

    fn close(&self, label: &str) -> HostResult<()> {
        self.record("close", Some(label), String::new());
        Ok(())
    }

    fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()> {
        self.record("set_position", Some(label), format!("x={x},y={y}"));
        Ok(())
    }

    fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()> {
        self.record("set_size", Some(label), format!("width={width},height={height}"));
        Ok(())
    }

    fn quit(&self) -> HostResult<()> {
        self.record("quit", None, String::new());
        Ok(())
    }

    /// 降级：记录请求，但**不重启任何东西**（返回 `false`）。
    fn relaunch(&self) -> HostResult<bool> {
        self.record("relaunch", None, String::new());
        Ok(false)
    }

    /// 降级：记录请求，但**不创建窗口**（返回 `false`）。
    fn create(&self, spec: &WindowCreateSpec) -> HostResult<bool> {
        self.record("create", Some(&spec.label), spec.url.clone());
        Ok(false)
    }
}

/// **对话框能力**（平台部分）。
///
/// 四个方法与既有命令面对齐（`host_dialog_open` / `save` / `message` /
/// `confirm`）。返回 `Ok(None)` / `Ok(false)` 一律表示**用户取消**（安全降级），
/// 而**不是**"对话框弹过了、用户取消了"。真正的原生对话框需要
/// `tauri-plugin-dialog`——**不在依赖闭包内**，因此本仓的 Tauri 实现
/// （[`crate::tauri::TauriDialogSink`]）也是降级路径（见其文档注释）。
pub trait DialogSink: Send + Sync {
    /// 打开文件（`directory = true` 时选目录）。`Ok(None)` = 取消。
    ///
    /// `multiple` 的**多选结果如何回传**当前无法表达：线形（`host_dialog_open`）
    /// 是单个 `string | null`，多选需要数组；本轮不改线形，故平台实现即使支持
    /// 多选也只能回第一个。如实登记为未实现，不假装支持。
    fn open_file(&self, multiple: bool, directory: bool) -> HostResult<Option<String>>;
    /// 保存文件。`Ok(None)` = 取消。
    fn save_file(&self, default_name: Option<&str>) -> HostResult<Option<String>>;
    /// 消息框（`kind` ∈ `info` / `error` / `warning`，缺省 `info`）。
    fn message(&self, kind: &str, title: &str, body: &str) -> HostResult<()>;
    /// 确认框。`Ok(false)` 的语义以实现文档为准（降级实现 = 取消，**不是**
    /// "用户点了否"）。
    fn confirm(&self, title: &str, body: &str) -> HostResult<bool>;
}

/// 对话框 sink 的**降级**实现：没有任何原生 UI（缺省实现）。
///
/// 语义与 R8 之前的桩逐字一致：
/// - `open_file` / `save_file` → `Ok(None)`（= 用户取消）；
/// - `confirm` → `Ok(false)`（= 取消；**既不是"用户点了否"，也不是"用户点了是"**）；
/// - `message` → 什么都不弹。
///
/// ⚠️ **这不是"实现了对话框"**：依赖闭包内没有 `tauri-plugin-dialog`，也没有
/// 任何原生对话框 API。"返回取消"是**安全**的降级（调用方按取消处理不会有破坏性
/// 后果），而谎报"用户选了某个文件 / 点了确定"会让上层按伪造的用户意图行事——
/// 那是安全缺陷，不是功能缺失。
#[derive(Debug, Default)]
pub struct NoopDialogSink;

impl DialogSink for NoopDialogSink {
    fn open_file(&self, _multiple: bool, _directory: bool) -> HostResult<Option<String>> {
        Ok(None)
    }

    fn save_file(&self, _default_name: Option<&str>) -> HostResult<Option<String>> {
        Ok(None)
    }

    fn message(&self, _kind: &str, _title: &str, _body: &str) -> HostResult<()> {
        Ok(())
    }

    fn confirm(&self, _title: &str, _body: &str) -> HostResult<bool> {
        Ok(false)
    }
}

/// **深链接能力**（平台部分）：注册 / 注销协议目标是**OS 级**动作。
///
/// 应用层那一半（记录协议 + 声明 `deep-link` 公共 topic）**不在本 trait 里**——
/// 它是平台无关的状态与总线操作，由 [`cmd_deep_link_register`] 自己完成。
/// 本 trait 只负责"让 OS 把这个协议交给本应用"这一件平台事。
pub trait DeepLinkSink: Send + Sync {
    /// 向 OS 注册协议（如 `tauron`）。
    fn register(&self, protocol: &str) -> HostResult<()>;
    /// 向 OS 注销协议。
    ///
    /// 真实消费者是 [`cmd_deep_link_register`] 自己：协议**换值**时必须先注销旧值，
    /// 否则旧协议的 OS 关联会留在系统里（换名后旧链接仍然会拉起本应用，而应用层
    /// 已不认识它）。这不是预留接口，是被调用的路径。
    fn unregister(&self, protocol: &str) -> HostResult<()>;
    /// 本实现是否真的做了 **OS 级**注册（诚实标注，供诊断与测试断言）。
    ///
    /// 全仓当前实现都返回 `false`——见 [`NoopDeepLinkSink`] 与
    /// `tauri.rs` 的 `TauriDeepLinkSink` 的文档注释。
    fn native_supported(&self) -> bool;
}

/// 深链接 sink 的**降级**实现：只记账，不碰 OS（缺省实现）。
///
/// ⚠️ 本仓**没有任何 OS 级深链接注册**：`tauri-plugin-deep-link` 不在依赖闭包内。
/// 应用层的记录与 topic 声明由 [`cmd_deep_link_register`] 完成（那是真实的，
/// 前端能收到 `deep-link` 帧的前提），但"OS 把 `tauron://` 交给本应用"这件事
/// **没有做**——`native_supported()` 恒 `false` 就是这句话的机器可判定形式。
#[derive(Debug, Default)]
pub struct NoopDeepLinkSink;

impl DeepLinkSink for NoopDeepLinkSink {
    fn register(&self, _protocol: &str) -> HostResult<()> {
        Ok(())
    }

    fn unregister(&self, _protocol: &str) -> HostResult<()> {
        Ok(())
    }

    fn native_supported(&self) -> bool {
        false
    }
}

/// `host_window_create` 的**核心规格**（平台无关；由核心按注册表解析后交给 sink）。
///
/// 字段来源**全部是宿主侧**：`plugin_id` 来自入参但必须先在注册表里存在，
/// `label` 由核心按身份约定铸造（`plugin-<id>`，**不是**入参），
/// `url` 来自 manifest 的 `entry.ui`，`title` / 尺寸来自入参或核心缺省。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCreateSpec {
    /// 窗口 label，恒为 `plugin-<插件 id>`（身份模型的载体）。
    pub label: String,
    /// 目标插件 id（已在注册表中存在）。
    pub plugin_id: String,
    /// 插件主面板 UI 入口（`manifest.entry.ui`）。
    pub url: String,
    /// 窗口标题（入参缺省 = manifest 的 `name`）。
    pub title: String,
    /// 内尺寸宽（逻辑像素）。
    pub width: u32,
    /// 内尺寸高（逻辑像素）。
    pub height: u32,
}

/// `host_window_create` 的线形入参（camelCase）。
///
/// 最小形态刻意**不含 url**：让调用方指定新窗口的 URL 等于把一个"带插件身份的
/// webview"指向任意地址——那个窗口会被宿主当作 `plugin-<id>` 身份单元（label 决定
/// 身份），于是主窗就成了提权跳板。URL 只能来自 manifest 的 `entry.ui`。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowCreateRequest {
    /// 目标插件 id（必须已在注册表中）。
    pub plugin_id: String,
    /// 窗口标题；缺省 = manifest 的 `name`。
    #[serde(default)]
    pub title: Option<String>,
    /// 内尺寸宽（逻辑像素）；缺省 [`DEFAULT_WINDOW_WIDTH`]。
    #[serde(default)]
    pub width: Option<u32>,
    /// 内尺寸高（逻辑像素）；缺省 [`DEFAULT_WINDOW_HEIGHT`]。
    #[serde(default)]
    pub height: Option<u32>,
}

/// 新窗口的缺省内尺寸（与示例应用 `tauri.conf.json` 的主窗尺寸同源：
/// 1024×768；这里取 720 高以避开小屏任务栏遮挡）。
pub const DEFAULT_WINDOW_WIDTH: u32 = 1024;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 720;

/// `host_window_create` 的返回（camelCase 线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowCreateOutcome {
    /// 铸造出的窗口 label，恒为 `plugin-<插件 id>`。
    ///
    /// 这个 label 就是 [`crate::tauri::cleanup_closed_window`] 的回收键
    /// （该函数用 `plugin-` 前缀推出插件 id），因此**创建后的清理不需要新钩子**：
    /// 宿主在 App Builder 的 `on_window_event` 里已有的那一行就够了。
    pub label: String,
    /// 目标插件 id。
    pub plugin_id: String,
    /// 是否**真的**创建了窗口。`false` = 宿主未实现（降级路径）。
    pub created: bool,
    /// `created = false` 时的原因；成功时为 `null`。
    pub reason: Option<String>,
}

/// `host_window_relaunch` 的返回（camelCase 线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRelaunchOutcome {
    /// 重启**之前**完成的阶段对账结果。
    ///
    /// 它出现在返回里不是装饰：这是"先对账、后重启"这个顺序在运行时可观测的
    /// 证据（顺序反了会把"上一次未干净结束"的标记带进下一次启动）。
    pub reconcile: PhaseReconcileOutcome,
    /// 是否**真的**向宿主请求了重启。`false` = 宿主没有重启原语（降级路径）。
    pub relaunch_requested: bool,
    /// `relaunchRequested = false` 时的原因；成功时为 `null`。
    pub reason: Option<String>,
}

/// **底座状态**（substrate）：任意宿主都需要的那部分，**不含插件注册表**。
///
/// R1b 的核心：插件运行时状态与底座状态分开持有，于是底座命令的实现体在**类型上**
/// 就触达不到 `registry` / `contributes`——不是靠约定、也不靠门禁正则，而是编译期。
/// 底座-only 宿主（harness 壳这类不跑插件运行时的客户端）只装配本结构，
/// **一份插件状态都不建**。
#[derive(Clone)]
pub struct SubstrateState {
    pub bus: Arc<Mutex<EventBus>>,
    /// 宿主设置文档（R7-2：`tauron-settings` 的 [`SettingsStore`]，激活孤儿 crate）。
    ///
    /// **不再是裸 `HashMap`**：键的合法性、值的类型、以及跨 schema 版本的迁移
    /// 都由 Store 负责（见 [`cmd_settings_set`] 与 [`host_settings_migrate`]）。
    /// 命名空间是单个伪插件 id [`HOST_SETTINGS_NAMESPACE`]——宿主设置的键是
    /// 开放集合，不需要 per-plugin 隔离。
    pub settings: Arc<Mutex<SettingsStore>>,
    pub notifications: Arc<Mutex<Vec<NotificationRecord>>>,
    /// 通知存储（P0-5：对接 tauron-notify crate）。
    pub notify_store: Arc<Mutex<NotifyStore>>,
    /// 系统通知分发通道（R7-3：`DispatchSink` 的 Tauri 实现）。
    ///
    /// 与 [`SubstrateState::plugin_flags`] 同一注入模式：底座本身**不依赖 tauri**，
    /// 装配时由 feature-gated 的 `tauri` 模块 `OnceLock::set` 一次。
    /// 未注入（底座-only 宿主、单测）= 没有系统通知通道：`host_notify` 只入
    /// 应用内环形缓冲，**不**产生 dispatch 记录（没有尝试就没有日志）。
    pub notify_sink: Arc<std::sync::OnceLock<Arc<dyn DispatchSink>>>,
    /// 恢复引擎（§4.14：启动阶段判定）。
    pub recovery: Arc<Mutex<RecoveryEngine>>,
    /// 恢复持久化载体（崩溃检测标记 + 落盘状态）。`dir = None` 时不落盘。
    pub recovery_store: Arc<Mutex<RecoveryStore>>,
    /// 本轮启动恢复状态的加载来源（诊断用，见 [`LoadSource::as_str`]）。
    pub recovery_source: LoadSource,
    /// i18n 引擎（§4.20：语言状态单一来源 + 资源包 + 缺失键计数）。
    pub i18n: Arc<Mutex<I18nEngine>>,
    /// 壳扩展状态（窗口几何/剪贴板/深链接/更新状态，P2）。
    pub shell_ext: Arc<Mutex<ShellExtState>>,
    /// 应用层订阅分组：多选择器订阅（`host_events_subscribe` 的 `sub` 数组）
    /// 返回的分组 token → [`GroupSubscription`]，供一次 `host_events_unsubscribe`
    /// 整体退订。单选择器订阅不建组（直接透传核心 token，与核心行为一致）。
    /// 窗口关闭 / 插件卸载由 [`prune_subscription_groups`] 按订阅者整组回收。
    ///
    /// 归属**底座**而不是插件运行时：它是事件总线（ipc 域）的订阅登记，任何 webview
    /// 都能订阅，与注册表无关。
    pub subscription_groups: Arc<Mutex<std::collections::HashMap<String, GroupSubscription>>>,
    /// 恢复阶段对账的插件侧写回口（见 [`PluginFlagSink`]）。
    ///
    /// 用 `OnceLock` 而不是普通字段：装配时由 [`PluginRuntimeState::with_substrate`]
    /// 注入一次。宿主把**同一份** `Arc<SubstrateState>` 同时 `manage` 给底座命令、
    /// 交给插件运行时——普通字段无法在事后注入，会让底座命令读到「没注入写回口」的
    /// 副本，对账静默失效。
    pub plugin_flags: Arc<std::sync::OnceLock<Arc<dyn PluginFlagSink>>>,
    /// **窗口能力**（R8 §1）：平台部分的可替换实现。
    ///
    /// 与 [`Self::plugin_flags`] 的 `OnceLock` **不同**，这里是普通 `pub` 字段：
    /// 装配方在 `Arc` 化**之前**直接替换即可（`tauri.rs` 的
    /// `command_state_with_dir_and_config` 就是这么做的），单测也能在同一个位置
    /// 注入假实现——`OnceLock` 的"只设一次"在这里没有必要（没有第二份状态会读到
    /// 旧值），反而会让单测里的替换变成不可达。
    pub window_sink: Arc<dyn WindowSink>,
    /// **对话框能力**（R8 §1）：缺省 = [`NoopDialogSink`]（降级：恒返回取消）。
    pub dialog_sink: Arc<dyn DialogSink>,
    /// **深链接能力**（R8 §1）：缺省 = [`NoopDeepLinkSink`]（无 OS 级注册）。
    pub deep_link_sink: Arc<dyn DeepLinkSink>,
}

// ──────────────────────────────────────────────────────────────────────────
// 进程插件执行器（P0-2：激活 tauron-proc，让 `PluginType::Process` 有真实入口）
//
// 分工：
// - **启动面**（`ProcSpawner`）是可注入 trait：生产 = `std::process::Command`
//   （`tauron_proc::CommandSpawner`），测试 = fake。测试里**绝不真起 sidecar**
//   （CI 上没有 sidecar 二进制，真起进程还会带进时序与残留进程的 flaky）。
// - **租约**（`plugin_id ↔ lease ↔ pid`）在 `tauron_host::registry::Registry`
//   的 `runtime` 表里（同域锁，见该文件锁序文档）——它是**插件身份**的句柄。
// - **崩溃窗口计数**用 `tauron_proc::CrashTracker`（缺省 3 次 / 5min），不新造计数。
// ──────────────────────────────────────────────────────────────────────────

/// `host_runtime_spawn` 的 `profile` 载荷（camelCase 线形）。
///
/// 为什么启动参数要由调用方给：宿主**不能凭空造出验签材料**。§4.7 要求
/// 「spawn 前校验二进制签名与哈希」，而 manifest 只声明 `entry.sidecar`（名字），
/// 不含可信 hash——于是这里的 `signature` / `binaryHash` / `abi` 是必填项，
/// 缺失即拒（见 [`RuntimeSpawnProfile::to_spawn_config`] 的调用点）。将来由安装器
/// 维护可信指纹库时，这个载荷可以由宿主侧补齐，但**今天不假装它存在**。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSpawnProfile {
    /// sidecar 二进制路径。缺省 = manifest 的 `entry.sidecar`。
    #[serde(default)]
    pub binary_path: Option<String>,
    /// 传给 sidecar 的参数。
    #[serde(default)]
    pub args: Vec<String>,
    /// 追加的环境变量。
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// 二进制签名（算法 / 签名 / 签名者），三项都不得为空。
    pub signature: RuntimeBinarySignature,
    /// 二进制 sha256（64 位 hex）。
    pub binary_hash: String,
    /// ABI 指纹。
    pub abi: RuntimeAbiFingerprint,
}

/// 线形签名（与 `tauron_proc::BinarySignature` 同构，但走 camelCase）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBinarySignature {
    pub algorithm: String,
    pub signature: String,
    pub signer_id: String,
}

/// 线形 ABI 指纹。
///
/// **不含 `generatedAt`**：校验时刻由宿主时钟决定（`tauron_proc::AbiFingerprint::now`），
/// 让前端自报"何时校验过"等于让诊断依据失去意义。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAbiFingerprint {
    /// crate 版本指纹。
    pub rust_version: String,
    /// 接口哈希。
    pub interface_hash: String,
}

impl RuntimeSpawnProfile {
    /// 落到 `tauron_proc::SpawnConfig`。
    ///
    /// 纯映射：校验（签名 / hash / abi 是否为空、hash 是否 64 位 hex）交给
    /// `tauron_proc::validate_spawn_config`——**同一份规则**不能有两份实现，
    /// 否则"经适配层"与"直接调 tauron-proc"两条路的严格程度会悄悄分叉。
    fn to_spawn_config(&self, binary_path: &str) -> SpawnConfig {
        SpawnConfig {
            binary_path: binary_path.to_string(),
            args: self.args.clone(),
            env: self.env.clone(),
            signature: BinarySignature {
                algorithm: self.signature.algorithm.clone(),
                signature: self.signature.signature.clone(),
                signer_id: self.signature.signer_id.clone(),
            },
            binary_hash: self.binary_hash.clone(),
            abi: ProcAbiFingerprint::now(
                self.abi.rust_version.clone(),
                self.abi.interface_hash.clone(),
            ),
        }
    }
}

/// `host_runtime_health` 的返回。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHealth {
    /// 进程是否仍存活（由注入的 [`ProcSpawner::is_alive`] 判定）。
    pub alive: bool,
    /// 该租约绑定的进程号。
    pub pid: u32,
    /// 崩溃窗口内的崩溃次数（`tauron_proc::CrashTracker`，**唯一**计数来源）。
    pub crashes: u32,
    /// 恢复引擎的连续失败计数（`BootCounter.consecutive_failures`）。
    ///
    /// 这是**全局**计数（恢复引擎按"启动连续失败"定义，用于决定 normal /
    /// safemode / repairmode），不是"该插件的崩溃次数"——后者看 `crashes`。
    /// 两个数一起给出，正是为了让调用方不必猜（历史断链的典型症状就是拿错一个
    /// 当另一个用）。崩溃对它的贡献路径见 [`deliver_runtime_crash`]。
    pub consecutive_failures: u32,
    /// 租约回收的留痕（`tauron_host::runtime::ReapStats`）。
    ///
    /// **全局计数**（宿主进程内所有插件的累计：`attempts` / `terminated` /
    /// `alreadyGone` / `failures` / `lastError`），**不是本条租约的统计**——
    /// 挂在 health 上是因为终止失败的留痕必须能被宿主 UI 看到，而为此新开一条
    /// 命令会再牵动能力表/授权档位/TS 镜像一整套面。要按插件看回收情况目前做不到
    /// （表里只留全局计数），如实标注。
    pub reap: ReapStats,
}

/// 进程插件运行时：可注入启动面 + 崩溃窗口计数。
pub struct ProcRuntime {
    spawner: Arc<dyn ProcSpawner>,
    /// 崩溃预算（**唯一**定义处：窗口与上限都取自这里，不另造常量）。
    limit: CrashLimit,
    crashes: Mutex<CrashTracker>,
}

impl ProcRuntime {
    /// 用给定启动面装配（缺省崩溃窗口：3 次 / 5min，见 `tauron_proc::CrashLimit`）。
    ///
    /// 崩溃预算**不做成可配置项**：窗口与上限的唯一来源是 `CrashLimit::default()`
    /// （§4.7 的定稿数值），多一个旋钮就多一处可以配置成"永不超限"的地方——那正是
    /// "崩溃预算形同虚设"的成因（缺省参数写错一次，门就永远不会关上）。
    pub fn new(spawner: Arc<dyn ProcSpawner>) -> Self {
        let limit = CrashLimit::default();
        Self {
            spawner,
            crashes: Mutex::new(CrashTracker::new(limit.clone())),
            limit,
        }
    }

    /// 启动面（生产 = `std::process::Command`）。
    pub fn spawner(&self) -> &Arc<dyn ProcSpawner> {
        &self.spawner
    }

    /// 崩溃预算（窗口秒数 + 窗口内允许的崩溃次数上限）。
    pub fn crash_limit(&self) -> &CrashLimit {
        &self.limit
    }

    /// 记一次崩溃（窗口内计数 + 是否已超限）。返回值 = 未超限。
    pub fn record_crash(&self, plugin_id: &str) -> bool {
        self.crashes.lock().record_crash(plugin_id)
    }

    /// 窗口内崩溃次数。
    pub fn crash_count(&self, plugin_id: &str) -> u32 {
        self.crashes.lock().crash_count(plugin_id)
    }

    /// 窗口内崩溃是否已超限（`CrashLimit`：缺省 3 次 / 5min）。
    ///
    /// 判定边界**完全**由 `tauron_proc::CrashTracker` 定义：窗口内崩溃数 **>**
    /// `max_crashes` 才算超限（即上限是"允许的崩溃次数"，不是"允许的重启次数"）。
    /// 这里不另造一套阈值，免得两处判定各自为政。
    pub fn is_crash_exceeded(&self, plugin_id: &str) -> bool {
        self.crashes.lock().is_exceeded(plugin_id)
    }

    /// 清空该插件的崩溃窗口记录。
    ///
    /// **唯一调用者是"用户显式确认"**（`host_registry_admin` 的 Enable）：状态机的
    /// `ResetCounters` 只重置状态机自己的预算计数，管不到进程侧窗口。不一起清，
    /// 就会出现"用户已确认、状态机说 Enabled、spawn 仍被崩溃窗口拒绝"的死结。
    pub fn reset_crashes(&self, plugin_id: &str) {
        self.crashes.lock().clear(plugin_id);
    }
}

impl Default for ProcRuntime {
    /// 缺省 = 生产启动面（真进程）。测试请用 [`ProcRuntime::new`] 注入 fake。
    fn default() -> Self {
        Self::new(Arc::new(tauron_proc::CommandSpawner::new()))
    }
}

/// 把注册表的租约回收接到进程执行器上（[`LeaseReaper`] 的进程实现）。
///
/// **为什么需要这一层**：宿主核心（`tauron-host`）不认识进程，它只知道"要终止某
/// 个 pid"；进程知识（`Child::kill` + `wait`）住在 `tauron-proc`。装配层是唯一
/// 同时认识两者的地方，因此桥接代码放在这里，而不是让核心反向依赖进程 host。
///
/// 两种终止结果的语义差别在核心侧被保留（`Terminated` / `AlreadyGone`），
/// 失败原因转成 `String` 供核心留痕（核心不依赖 `ProcError`）。
struct SpawnerReaper(Arc<dyn ProcSpawner>);

impl LeaseReaper for SpawnerReaper {
    fn kill(&self, pid: u32) -> Result<ReapOutcome, String> {
        match self.0.kill(pid) {
            Ok(tauron_proc::KillOutcome::Terminated) => Ok(ReapOutcome::Terminated),
            Ok(tauron_proc::KillOutcome::AlreadyGone) => Ok(ReapOutcome::AlreadyGone),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// **插件运行时状态**：多插件宿主才装配的部分（注册表 + 贡献登记）。
///
/// 持 [`SubstrateState`] 的**同一份** Arc（不是拷贝）：插件命令既要注册表，也要底座
/// 能力（通知 / i18n / 恢复），因此这里显式给出唯一底座入口。装配时两个 managed 值
/// 共享同一份底座——不存在「第二份状态」这类「改了没生效」的温床。
#[derive(Clone)]
pub struct PluginRuntimeState {
    pub substrate: Arc<SubstrateState>,
    pub registry: Arc<Registry>,
    pub contributes: Arc<Mutex<ContributesRegistry>>,
    /// 进程插件运行时（P0-2）：可注入的启动面 + 崩溃追踪。
    ///
    /// 进程细节（`std::process::Command`、存活探测）与崩溃窗口计数都在这一侧，
    /// 不进 `tauron-host`；核心只持有**租约**（`Registry` 的 `runtime` 表）。
    pub proc_runtime: Arc<ProcRuntime>,
}

impl core::ops::Deref for PluginRuntimeState {
    type Target = SubstrateState;

    fn deref(&self) -> &SubstrateState {
        &self.substrate
    }
}

/// 兼容名：`CommandState` ≡ **插件运行时状态**。
///
/// R1b 之前它是 12 字段的 god object；拆分后只是 [`PluginRuntimeState`] 的别名，
/// 并通过 `Deref` 暴露底座字段。保留别名是为了让既有调用点（约 70 处测试构造 +
/// 全部命令包装器）零改动，**不是**因为还存在第二份状态。
///
/// 底座-only 宿主改用 [`SubstrateState`]（`SubstrateState::with_adapter_config`）
/// 并注册 `tauron_substrate_handler!`——类型上就拿不到插件状态。
pub type CommandState = PluginRuntimeState;

/// `host_notify` 的兼容日志上限（见 `cmd_notify` 的说明）。
///
/// 这份日志**当前没有任何读取方**（没有命令返回它，生产代码只在测试里读），
/// 但每次 `host_notify` 都会写入：没有上限时它是一个纯写入的无限增长点。
pub const MAX_NOTIFICATION_LOG: usize = 256;

/// 兼容用通知记录。
///
/// ⚠️ **不在线上**：没有任何命令返回它（`host_notify` 返回 `()`）。保留是为了
/// 兼容既有调用面。`rename_all = "camelCase"` 是为了让潜在的线上形态与
/// `@tauron/host` 的 TS `NotificationRecord`（`pluginId` 等）一致——两侧字段
/// 名此前不一致（Rust 蛇形 / TS 驼峰），一旦有人把它接上线就会静默读到
/// `undefined`。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub plugin_id: String,
    pub title: String,
    pub body: String,
    pub timestamp: u64,
}

/// 多选择器订阅的分组登记项。
///
/// `subscriber` 必须随行：分组 token（`grp:<首token>`）本身不含归属信息，
/// 而核心订阅会被 `dispose_subscriber`（窗口关闭 / 插件卸载）直接清掉——
/// 登记项若不按订阅者回收，「开窗 → 多选择器订阅 → 关窗」循环会让这张表
/// 无限累积（§8-3 悬挂状态：token 指向已消亡的订阅）。
#[derive(Debug, Clone)]
pub struct GroupSubscription {
    /// 建组的订阅者（wire 层为 `plugin_id_of(webview.label())`，与
    /// [`prune_subscription_groups`] 的回收键同源）。
    pub subscriber: String,
    /// 组内的核心订阅 token（按选择器顺序）。
    pub tokens: Vec<String>,
}

impl SubstrateState {
    /// 底座装配：恢复持久化 + i18n + origin 清单 + 通知存储。
    ///
    /// 崩溃检测（上一次未干净结束 → 计一次失败）和「本轮进行中」的落盘都由
    /// [`RecoveryStore::load`] 一次完成，调用方无法跳过这一步（§4.14：判定只
    /// 依赖标记文件与计数器，禁止解析日志字符串）。
    ///
    /// 必需插件集合以 [`AdapterConfig::required_plugins`] 为准整体替换，替换后
    /// 再落盘一次——否则宿主改了必需集合也不跨进程。落盘失败不会静默：原因留在
    /// `last_error`，由 `host_recover_boot` 的 `persistence.lastError` 暴露。
    ///
    /// **不消费 `cfg.registry`**：注册表属插件运行时（[`PluginRuntimeState`]），
    /// 底座装配不该顺带建一份——那正是 R1 要拆掉的东西。
    pub fn with_adapter_config(cfg: &AdapterConfig) -> Self {
        let notify_store = NotifyStore::new(256).expect("NotifyStore::new(256) should succeed");

        let mut store = match cfg.recovery_data_dir.clone() {
            Some(dir) => RecoveryStore::new(dir),
            None => RecoveryStore::disabled(),
        };
        let required = cfg.required_plugins.clone();

        let record = store.load(required.clone());
        let mut engine = record.engine;
        // 必需性是宿主配置说了算（引擎自身的 `set_plugin` 只增不减）。
        engine.set_required_plugins(required);
        store.save(&engine);

        // 宿主设置命名空间：注册 schema + v1→v2 迁移（R7-2）。
        // 装配期一次，之后 `host_settings_get`/`set` 才有 schema 可依。
        let mut settings = SettingsStore::new();
        install_host_settings_schema(&mut settings);

        Self {
            bus: Arc::new(Mutex::new(EventBus::default())),
            settings: Arc::new(Mutex::new(settings)),
            notifications: Arc::new(Mutex::new(Vec::new())),
            notify_store: Arc::new(Mutex::new(notify_store)),
            notify_sink: Arc::new(std::sync::OnceLock::new()),
            recovery: Arc::new(Mutex::new(engine)),
            recovery_store: Arc::new(Mutex::new(store)),
            recovery_source: record.source,
            i18n: Arc::new(Mutex::new(I18nEngine::default())),
            shell_ext: Arc::new(Mutex::new(ShellExtState {
                origin_allowlist: cfg.origin_allowlist.clone(),
                ..ShellExtState::default()
            })),
            subscription_groups: Arc::new(Mutex::new(std::collections::HashMap::new())),
            plugin_flags: Arc::new(std::sync::OnceLock::new()),
            // R8：三类平台能力的**降级缺省**。宿主（`tauri.rs`）在装配时替换为
            // Tauri 实现；不替换 = 进程内留痕 / 取消 / 无 OS 注册（如实降级）。
            window_sink: Arc::new(MemoryWindowSink::new()),
            dialog_sink: Arc::new(NoopDialogSink),
            deep_link_sink: Arc::new(NoopDeepLinkSink),
        }
    }
}

impl PluginRuntimeState {
    /// 创建默认状态（测试用）。
    ///
    /// **恢复持久化关闭**：不碰磁盘，因此测试无副作用、无残留文件。生产宿主
    /// 请用 [`Self::with_adapter_config`] 并传入宿主数据目录。
    pub fn new() -> Self {
        Self::with_adapter_config(AdapterConfig::default())
    }

    /// 创建指定注册表配置的状态（恢复持久化仍关闭）。
    pub fn with_config(config: RegistryConfig) -> Self {
        Self::with_adapter_config(AdapterConfig {
            registry: Some(config),
            ..AdapterConfig::default()
        })
    }

    /// 自建底座并装配插件运行时（测试 + 「自己全都要」的宿主）。
    pub fn with_adapter_config(cfg: AdapterConfig) -> Self {
        let substrate = Arc::new(SubstrateState::with_adapter_config(&cfg));
        Self::with_substrate(substrate, cfg)
    }

    /// 在**既有**底座状态上装配插件运行时。
    ///
    /// 宿主装配走这里（而不是 [`Self::with_adapter_config`]）：底座 Arc 由宿主创建
    /// 并同时 `manage`，插件运行时复用**同一份**——两个 managed 值不可能指向两份
    /// 底座状态。
    ///
    /// 进程执行器用生产启动面（`std::process::Command`）。**测试请用**
    /// [`Self::with_spawner`]，否则会在 CI 上真起进程。
    pub fn with_substrate(substrate: Arc<SubstrateState>, cfg: AdapterConfig) -> Self {
        Self::with_substrate_and_spawner(substrate, cfg, Arc::new(tauron_proc::CommandSpawner::new()))
    }

    /// [`Self::with_substrate`] 的可注入变体：进程启动面由调用方给出。
    ///
    /// 这是「测试里绝不真起 sidecar」的落地点：fake 启动面既能证明**真**调用了
    /// 启动面（记录调用），也能凭空制造崩溃（把 pid 标记为已死），而执行器语义
    /// （租约、`E_LEASE_EXPIRED`、崩溃计数、`RuntimeCrash` 投递）全部照原样走。
    pub fn with_substrate_and_spawner(
        substrate: Arc<SubstrateState>,
        cfg: AdapterConfig,
        spawner: Arc<dyn ProcSpawner>,
    ) -> Self {
        let registry = Arc::new(Registry::new(cfg.registry.unwrap_or_default()));
        // 把租约回收接到进程执行器（P0-2 缺口一）：卸载/清除/崩溃换新都必须**真的**
        // 终止 sidecar，否则留下没人认领的孤儿进程。未注入时核心仍会摘表项，
        // 但每次终止都按失败留痕（`Registry::runtime_reap_stats`），不会静默。
        registry.set_lease_reaper(Arc::new(SpawnerReaper(spawner.clone())));
        // 注入恢复对账的插件侧写回口。`OnceLock::set` 只接受第一次注入：重复装配
        // 不会换掉已注入的写回口（否则两套插件运行时会让对账写到错误的注册表）。
        let _ = substrate
            .plugin_flags
            .set(Arc::new(RegistryFlagSink {
                registry: registry.clone(),
            }));
        Self {
            substrate,
            registry,
            contributes: Arc::new(Mutex::new(ContributesRegistry::default())),
            proc_runtime: Arc::new(ProcRuntime::new(spawner)),
        }
    }

    /// 自建底座 + 注入进程启动面（测试用；恢复持久化关闭）。
    pub fn with_spawner(spawner: Arc<dyn ProcSpawner>) -> Self {
        Self::with_substrate_and_spawner(
            Arc::new(SubstrateState::with_adapter_config(&AdapterConfig::default())),
            AdapterConfig::default(),
            spawner,
        )
    }
}

/// [`PluginFlagSink`] 的注册表实现（插件运行时装配时注入底座）。
struct RegistryFlagSink {
    registry: Arc<Registry>,
}

impl PluginFlagSink for RegistryFlagSink {
    fn flag_snapshots(&self) -> Vec<(String, bool)> {
        self.registry
            .list_all()
            .into_iter()
            .map(|p| (p.id, p.disabled_by_safemode))
            .collect()
    }

    fn report_flag_event(&self, plugin_id: &str, enter_safemode: bool) -> Option<bool> {
        // 快照阶段已成功解析过同一个 id（确定性），这里再解析一次不会失败；
        // 真失败就当作注册表拒绝（计数 ignored），不 panic。
        let id = PluginId::new(plugin_id).ok()?;
        let event = if enter_safemode {
            Event::SafemodeEnter
        } else {
            Event::SafemodeExit
        };
        self.registry.report_event(&id, event).ok().map(|o| o.illegal)
    }
}

impl Default for PluginRuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod substrate_only_tests {
    use super::*;

    /// 方案 R1「验证」项的**运行期**证据：只有底座状态时，底座命令可用。
    ///
    /// 编译期证据同样在这个函数里——若底座命令的签名还能看见 `registry` /
    /// `contributes`，本函数就**编译不过**（`state.registry` 在 `&SubstrateState`
    /// 上不存在）。这不是约定，是类型系统。
    #[test]
    fn substrate_only_state_serves_base_commands() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());

        // 底座命令：品牌桩 + 只读恢复查询，都不需要插件运行时。
        assert!(cmd_brand_info(&state).is_ok());
        let boot = cmd_recover_boot(&state).expect("recover_boot 应成功");
        assert!(boot.get("phase").is_some(), "恢复阶段必须可读");

        // i18n / 设置 / 通知等底座域命令同样在只有底座时可用。
        assert!(cmd_settings_get(&state, "k").is_ok());
        assert!(cmd_i18n_stats(&state).is_ok());

        // 插件侧写回口未注入 → 对账无事可做（空结果，而不是 panic 或静默失败）。
        assert!(
            state.plugin_flags.get().is_none(),
            "底座-only 装配不得注入插件侧写回口"
        );
    }

    /// 插件运行时装配会注入写回口，且**共享同一份底座**（不是两份状态）。
    #[test]
    fn plugin_runtime_shares_one_substrate_and_injects_sink() {
        let plugin = PluginRuntimeState::new();
        assert!(
            plugin.substrate.plugin_flags.get().is_some(),
            "多插件装配必须注入恢复对账写回口"
        );
        // Deref：从底座侧写入，插件运行时侧立刻可见——同一份状态，不是拷贝。
        // 设置走 `SettingsStore`（R7-2），故这里用 Store 的 API 而不是裸 map。
        plugin.substrate.settings.lock().set_layer(
            HOST_SETTINGS_NAMESPACE,
            tauron_settings::LayerKind::Builtin,
            serde_json::json!({"probe": 1}),
        );
        assert_eq!(
            plugin
                .settings
                .lock()
                .get_key(HOST_SETTINGS_NAMESPACE, "probe")
                .unwrap(),
            Some(serde_json::json!(1)),
            "插件运行时与底座必须是同一份状态（同一 Arc，不是拷贝）"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 命令调用主体与身份判定（R7 收口）
// ──────────────────────────────────────────────────────────────────────────
//
// **背景（缺口原文）**：R4 的身份模型是「`self` 档命令的 `pluginId` 一律不由
// 调用方传入，由宿主从 webview label 解析」。它已经落在插件面命令上
// （`authz::resolve_self_identity` / `resolve_principal` 用于
// `host_lifecycle_report` / `host_plugin_call` / events / streams），**但没有
// 落在设置族与特权命令上**。后果是「特权 = 仅主窗」这句话此前只在**部署配置**
// （origin 白名单 / Tauri ACL）里成立，代码里没有任何判定——`host_registry_admin`
// / `host_registry_list_all` / `host_runtime_spawn` / `host_runtime_health` 的包装器
// 直接转调，不解析身份；`host_settings_*` 更是任何窗口都能读写任何键。
//
// 本节把那条判定变成**代码**：主体一律从 label 解析（复用
// [`tauron_host::authz::resolve_principal`]，不另造一套解析），畸形 label 一律
// 拒绝（绝不降级成主窗），特权命令仅主窗，设置族按身份绑定键空间。
//
// 判定入口都放在无 tauri 依赖的这一层：`tauron-adapter` 的底座必须能在没有
// Tauri 运行时的环境里被测试（单测构造不出 `WebviewWindow`），所以「解析 label →
// 判定 → 转调」里只有**第一跳**（`window.label()`）留在 wire 层。

/// 命令调用主体。
///
/// 由 wire 层从 `WebviewWindow` 的 label 解析得到，再交给 `*_as` 系列命令核心判定。
/// **没有 `Invalid` 变体**：畸形 label 在 [`Caller::from_label`] 就被拒绝，
/// 构造不出主体——这是刻意的，`authz::Principal::Invalid` 的注释要求调用方拒绝它，
/// 让它成为一个可继续传递的枚举值迟早会有人忘了判。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// 主窗 / 应用自身的非插件 webview（特权主体）。
    MainWindow,
    /// 插件 webview（label = `plugin-<合法 id>`），携带经校验的插件 id。
    Plugin(String),
}

impl Caller {
    /// 从 webview label 解析主体。
    ///
    /// 安全要点：[`tauron_host::authz::Principal::Invalid`]（`plugin-` 前缀但 id
    /// 非法）一律**拒绝**，绝不降级成 [`Caller::MainWindow`]——否则伪造一个畸形
    /// label 就能拿到主窗档权力（提权）。错误码复用既有的身份不符/越权码
    /// [`ErrorCode::E_AUTH_DENIED`]，**不新增错误码**。
    pub fn from_label(label: &str) -> HostResult<Self> {
        match tauron_host::authz::resolve_principal(label) {
            tauron_host::authz::Principal::MainWindow => Ok(Caller::MainWindow),
            tauron_host::authz::Principal::Plugin(id) => {
                Ok(Caller::Plugin(id.as_str().to_string()))
            }
            tauron_host::authz::Principal::Invalid(raw) => Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "webview label `{raw}` 不是合法身份（`plugin-` 前缀但 id 非法）；\
                     畸形 label 绝不降级成主窗"
                ),
            )),
        }
    }

    /// 是否为主窗主体。
    pub fn is_main_window(&self) -> bool {
        matches!(self, Caller::MainWindow)
    }

    /// 插件 id；主窗返回 `None`。
    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            Caller::Plugin(id) => Some(id.as_str()),
            Caller::MainWindow => None,
        }
    }

    /// 诊断用的主体描述（错误消息里用）。
    fn describe(&self) -> String {
        match self {
            Caller::MainWindow => "主窗".to_string(),
            Caller::Plugin(id) => format!("插件 `{id}`"),
        }
    }
}

/// **主窗专属命令的代码层判定**（特权 / 管理面 / 迁移入口）。
///
/// 与 [`tauron_host::authz::ADMIN_COMMANDS`] 同源：表里登记为
/// [`tauron_host::authz::AuthTier::Privileged`] 的命令必须全部被本函数拒绝插件主体
/// （有测试逐条遍历该表守住这一点，防止表与代码漂移）。表里**未登记**的主窗管理面
/// 命令（`host_registry_list_all`）与新增的迁移入口用同一条判定，语义同为「仅主窗」。
pub fn require_main_window(caller: &Caller, command: &str) -> HostResult<()> {
    match caller {
        Caller::MainWindow => Ok(()),
        Caller::Plugin(_) => Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "宿主命令 `{command}` 是主窗专属（privileged 档），{} 无权调用；\
                 该判定在代码层执行（不只是 origin ACL / Tauri ACL）",
                caller.describe()
            ),
        )),
    }
}

/// 插件设置的命名空间前缀（约定：`plugin:<插件 id>`）。
///
/// 与 TS 侧既有的键约定一致（例如 `plugin:p.theme`）。
pub fn plugin_settings_namespace(plugin_id: &str) -> String {
    format!("plugin:{plugin_id}")
}

/// **设置族的键空间判定**。
///
/// - 主窗：任意键（现状不变）；
/// - 插件：键必须落在**自己的**命名空间内——等于 `plugin:<自己 id>`，或以
///   `plugin:<自己 id>.` 开头；其余（别的插件的键、`host.*` 这类宿主级键）一律拒绝。
///
/// 边界是**显式比较**而不是 `starts_with` 前缀包含：后者会让 `plugin:p.a` 的插件
/// 写穿 `plugin:p.ab.x`（`plugin:p.a` 是 `plugin:p.ab` 的前缀）。判定的落点写成
/// 「剥掉自己的命名空间后，剩下的是空串或以 `.` 开头」。
pub fn require_settings_key_scope(caller: &Caller, key: &str) -> HostResult<()> {
    let Caller::Plugin(id) = caller else {
        // 主窗：任意键（含宿主级键）。
        return Ok(());
    };
    let ns = plugin_settings_namespace(id);
    let in_scope = match key.strip_prefix(ns.as_str()) {
        Some(rest) => rest.is_empty() || rest.starts_with('.'),
        None => false,
    };
    if in_scope {
        return Ok(());
    }
    Err(HostError::new(
        ErrorCode::E_AUTH_DENIED,
        format!(
            "设置键 `{key}` 不在{}自己的命名空间 `{ns}`（或 `{ns}.…`）内；\
             越界读写别的插件键 / 宿主级键一律拒绝",
            caller.describe()
        ),
    ))
}

/// **身份绑定参数的判定**（轮 11 第二批：`host_notify` / `host_i18n_load` /
/// `host_i18n_cleanup_plugin` / `host_recover_report`）。
///
/// # 为什么是"绑参数"而不是"整条命令主窗专属"
///
/// 这四条命令都**收一个 `pluginId` 参数**，而那个参数决定"这次操作算在谁头上"：
/// 通知的署名、文案的命名空间、失败预算与故障归因的归属。插件本来就需要以
/// **自己的名义**做这些事（发自己的通知 / 装自己的文案 / 报自己的状态），一刀切成
/// 主窗专属会把插件的正常功能一起关掉。所以判定落在**参数**上，与
/// [`require_settings_key_scope`] 同一形状：
///
/// - **主窗**：任意 `pluginId`，含 `None`（宿主自己就是这些操作的另一个合法主体，
///   现状不变）；
/// - **插件**：`Some(id)` 必须**等于自己**；`None` 一律拒绝——`None` 在这四条命令里
///   是**宿主命名空间 / 应用级上报**的语义，不是"省略"，插件代为主张就是冒名。
///
/// 与 [`tauron_host::authz::resolve_self_identity`] 同一条规则（self 档只认身份、
/// 不认入参），区别只在主体模型：那边以 label 为输入且**不认主窗**，这里以
/// [`Caller`] 为输入并按上面两档判定。
///
/// # 拒绝码与副作用
///
/// 复用既有的身份/越权码 [`ErrorCode::E_AUTH_DENIED`]（**不新增错误码**）。
/// 判定必须在**任何副作用之前**执行：越界时通知不入环形缓冲、别人的 bundle 不被清、
/// 恢复计数器与阶段一动不动（每条命令都有"零副作用"断言）。
pub fn require_self_plugin_scope(
    caller: &Caller,
    command: &str,
    claimed: Option<&str>,
) -> HostResult<()> {
    let Caller::Plugin(me) = caller else {
        // 主窗：任意 pluginId（含 None），与 R7 之前的现状一致。
        return Ok(());
    };
    match claimed {
        Some(id) if id == me => Ok(()),
        Some(id) => Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "`{command}` 的 pluginId `{id}` 不是调用方自己（`{me}`）：以别的插件或宿主的\
                 名义执行该操作一律拒绝（判定在任何副作用之前）"
            ),
        )),
        None => Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "`{command}` 未指明归属主体（`pluginId`（署名 / 命名空间）或 `id`（通知）\
                 缺省 = 宿主级命名空间 / 应用级上报），{} 不得代为主张；请传自己的 id（`{me}`）",
                caller.describe()
            ),
        )),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 命令处理函数（平台无关，可直接测试）
// ──────────────────────────────────────────────────────────────────────────

/// `host_registry_list`：列出可见插件（scoped-read 档）。
pub fn cmd_registry_list(
    state: &PluginRuntimeState,
    caller: Option<&str>,
    subscribed_topics: &[String],
) -> HostResult<Vec<PluginSummary>> {
    let caller_id = caller.map(PluginId::new).transpose()?;
    Ok(guard("registry_list", || {
        state.registry.list_visible(caller_id.as_ref(), subscribed_topics)
    })?)
}

/// `host_registry_list_all`：全量列表（privileged 档 / 主窗 UI）。
pub fn cmd_registry_list_all(
    state: &PluginRuntimeState,
) -> HostResult<Vec<PluginSummary>> {
    Ok(guard("registry_list_all", || {
        state.registry.list_all()
    })?)
}

/// `host_registry_list_all` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 判定在转调**之前**：全量列表暴露所有插件（含未安装/已禁用），插件主体看到
/// 别人存在本身就是 scoped-read 想遮住的信息。
pub fn cmd_registry_list_all_as(
    caller: &Caller,
    state: &PluginRuntimeState,
) -> HostResult<Vec<PluginSummary>> {
    require_main_window(caller, "host_registry_list_all")?;
    cmd_registry_list_all(state)
}

/// `host_registry_admin`：主窗特权命令（启用/禁用/卸载/清除）。
pub fn cmd_registry_admin(
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
) -> HostResult<TransitionOutcome> {
    let id = PluginId::new(plugin_id)?;
    // `guard` 只负责把 panic 转成 `E_HOST_PANIC`，返回 `HostResult<T>`；
    // 这里 T 是注册表操作自身的 `HostResult<TransitionOutcome>`，故需双重 `?`：
    // 外层解 guard（panic 层），内层解操作结果。
    let out = guard("registry_admin", || state.registry.admin_op(&id, op))??;

    // 卸载/清除成功后，**所有按插件 id 为键的旁路状态**必须一起回收——它们不随
    // 注册表条目一起消失，不回收就是缓慢泄漏（卸载→重装循环会持续累积）：
    // - 事件总线的发布/订阅（§8-3 零悬挂订阅），反向索引与队列以插件 id 为键；
    // - 多选择器订阅的分组登记（`subscription_groups`，以订阅者为键——不回收
    //   就是指向已消亡 token 的死条目）；
    // - 贡献注册表（菜单/命令/面板入口，残留会让卸载的插件入口还显示）；
    // - i18n 文案（`plugin:<id>.oc.*`，跨所有语言包）；
    // - 恢复引擎的插件登记（状态表是**持久化**的，残留会让已卸载插件永远
    //   出现在 `host_recover_boot` 的 `disabledPlugins` 里）。
    // 仅在迁移真的发生（非非法且状态确实改变）时回收——非法迁移的条目仍在
    // 注册表，回收它的旁路状态反而会造成不一致。
    if !out.illegal
        && out.from != out.to
        && matches!(op, RegistryAdminOp::Uninstall | RegistryAdminOp::Purge)
    {
        let bus = state.bus.lock();
        bus.dispose_publisher(plugin_id);
        bus.dispose_subscriber(plugin_id);
        prune_subscription_groups(state, plugin_id);
        state.contributes.lock().clear_plugin(plugin_id);
        state.i18n.lock().cleanup_plugin(plugin_id);
        // - 通知存储（`host_notify` 落进环形缓冲的通知，残留会让已卸载插件的
        //   未读计数永远膨胀、通知中心显示死条目）；
        state.notify_store.lock().cleanup_plugin(plugin_id);
        state.recovery.lock().remove_plugin(plugin_id);
    }

    // **人工确认的唯一出口**（P0-2 崩溃预算）：Enable 是宿主 UI 的显式动作，
    // 因此它必须同时清零**两道**预算——状态机侧的 `ResetCounters`（迁移表里
    // `ERRORED_USER_CONFIRM + ENABLE → ENABLED` 已带）与进程侧的崩溃窗口
    // （`ProcRuntime::reset_crashes`）。只清前者会出现死结：用户已确认、状态机说
    // ENABLED、`host_runtime_spawn` 仍被崩溃窗口拒绝，而窗口是 5 分钟——
    // 表现为"点了启用但插件永远起不来"。
    //
    // 条件只排除**非法迁移**（状态机明确拒绝了这个事件，说明用户想做的事与当前
    // 状态无关）；`from == to`（本来就 ENABLED）也算用户确认，一并清零。
    if !out.illegal && matches!(op, RegistryAdminOp::Enable) {
        state.proc_runtime.reset_crashes(plugin_id);
    }

    // 管理操作会改变插件状态，对账一次保持恢复判定与注册表一致。
    // 注意：在安全模式下手动 Enable 一个非必需插件会被 `SafemodeEnter` 重新
    // 禁用——返回的 `TransitionOutcome` 描述的是本次迁移，不是最终状态，
    // 最终状态以 `host_registry_list` 为准。这是刻意设计：允许手动绕过会
    // 让安全模式失去意义。
    reconcile_recovery_phase(state);

    Ok(out)
}

/// `host_registry_admin` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 判定必须在所有副作用之前：本命令会改注册表状态、回收旁路状态、清崩溃预算。
/// 拒绝路径**一个副作用都不产生**（有测试断言拒绝后注册表与插件状态不变）。
pub fn cmd_registry_admin_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
) -> HostResult<TransitionOutcome> {
    require_main_window(caller, "host_registry_admin")?;
    cmd_registry_admin(state, plugin_id, op)
}

/// `host_lifecycle_report`：上报生命周期事件（self 档）。
///
/// 身份从 webview label 解析，忽略入参 pluginId（§2.1）。
///
/// 上报后做一次恢复阶段对账：插件进入注册表（`InstallOk`）或自报启用后，若
/// 当前处于安全模式且该插件非必需，`SafemodeEnter` 会立刻把它标记为禁用——
/// 恢复判定优先于插件自己的上报。
pub fn cmd_lifecycle_report(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    event: Event,
) -> HostResult<TransitionOutcome> {
    let out = guard("lifecycle_report", || {
        state.registry.lifecycle_report(webview_label, claimed_id, event)
    })??;

    // D28 反向同步边：试启期间插件自报错误 → 注册表按 D28 回落
    // （`IncrementTrialFailure` + 打回 DISABLED + 置安全模式标志）。引擎若
    // 不知情、仍记它 `TrialEnable`，紧接着的对账会把「引擎判启用、注册表却
    // 禁用」当成不一致，补发 `SafemodeExit` 把刚打上的标志又清掉——形成
    // 「插件报错 → 标志被清 → 插件重入」的往返抖动，且两套试验预算各自为
    // 政。这里把试验失败同步进引擎，对账立即静默、预算保持同向。
    // 只补这一条动作：其余动作（重试预算等）引擎不建模，注册表自己闭环。
    if out
        .actions
        .contains(&tauron_host::lifecycle::Action::IncrementTrialFailure)
    {
        if let Ok(id) = tauron_host::authz::resolve_self_identity(webview_label, claimed_id) {
            state.recovery.lock().record_trial_failure(id.as_str());
        }
    }

    reconcile_recovery_phase(state);
    Ok(out)
}

/// `host_plugin_call`：插件调用自己的 C/D 后端（self 档）。
pub fn cmd_plugin_call(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    cmd: &str,
    args: serde_json::Value,
) -> HostResult<PendingCall> {
    guard("plugin_call", || {
        let id = tauron_host::authz::resolve_self_identity(webview_label, claimed_id)?;
        state.registry.call_begin(&id, cmd, args)
    })?
}

/// `host_call_end`：stream 终帧确认（self 档）。
pub fn cmd_call_end(
    state: &PluginRuntimeState,
    call_id: &str,
) -> HostResult<PendingCall> {
    guard("call_end", || {
        state.registry.call_end(call_id)
    })?
}

/// `host_cancel`：取消 pending call（self 档）。
pub fn cmd_cancel(
    state: &PluginRuntimeState,
    call_id: &str,
) -> HostResult<()> {
    guard("cancel", || {
        state.registry.call_cancel(call_id)
    })?
}

// ────────────────────────────────────────────────────────────────
// 流式帧（R5 / P0-1）
//
// 这三条命令是「流式」从**形状**变成**通路**的地方。在此之前：
// `plugin_call` 的 `kind: 'stream'` 会被校验、`call_end` 的注释也叫「终帧确认」，
// 但没有任何路径能把帧送出去——`channel` 只当不透明 JSON 收下，写完就丢，
// 前端表现为「回调永不触发、也不报错」。三命令 + `Registry::stream_*` 补上这条链。
// ────────────────────────────────────────────────────────────────

/// `host_stream_open` 的返回：新句柄 + 归属调用。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOpened {
    pub stream_id: String,
    pub call_id: String,
}

/// `host_stream_open`：为一次**已挂帧载体**的调用开流（self 档）。
///
/// 调用侧必须带 `channel` 调 `host_plugin_call`：没有载体的调用在这里以
/// `E_CALL_NOT_FOUND` 显式失败，而不是开一条「帧进虚空」的流（那是最难查的静默断链）。
pub fn cmd_stream_open(
    state: &PluginRuntimeState,
    subscriber: &str,
    call_id: &str,
) -> HostResult<StreamOpened> {
    guard("stream_open", || {
        let stream_id = state.registry.stream_open(call_id, subscriber)?;
        Ok(StreamOpened {
            stream_id,
            call_id: call_id.to_string(),
        })
    })?
}

/// `host_stream_write`：写一帧 `data`（self 档）。
///
/// `seq` 由宿主铸（不接受前端传入，与 pending call 的 `seq` 同规矩）：前端自报
/// 序号就能伪造乱序/重复，接收方的去重假设随即失效。
pub fn cmd_stream_write(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    args_json: Option<serde_json::Value>,
    args_raw: Option<Vec<u8>>,
) -> HostResult<StreamFrame> {
    guard("stream_write", || {
        state
            .registry
            .stream_write(stream_id, subscriber, args_json, args_raw)
    })?
}

/// `host_stream_close`：发终帧并使句柄失效（self 档）。
///
/// `kind` 只接受 `end` / `error`（闭集）：`data` 不是终帧，用它关流会让接收方
/// 以为流还在继续。
pub fn cmd_stream_close(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    kind: &str,
) -> HostResult<StreamFrame> {
    let parsed = StreamKind::parse(kind).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("未知帧种类 `{kind}`（合法值：data | end | error）"),
        )
    })?;
    guard("stream_close", || {
        state.registry.stream_close(stream_id, subscriber, parsed)
    })?
}

// ────────────────────────────────────────────────────────────────
// 进程插件运行时（P0-2）
//
// 这两条命令是 `PluginType::Process` 从「四类枚举里的一个值」变成**可执行**的
// 地方。在此之前 `tauron-proc`（1780 行、配置/校验/心跳/崩溃追踪齐备）是孤儿
// crate：没有任何 crate 依赖它，于是 `PluginType::Process` 在宿主侧没有任何入口，
// 插件只能"声明自己是 C 类"而永远跑不起来。
//
// 域归属：**插件运行时域**（依赖注册表与插件状态），只能出现在
// `tauron_plugin_handler!`，绝不进 `tauron_substrate_handler!`。
// ────────────────────────────────────────────────────────────────

/// `host_runtime_spawn`：为 **process 型**插件启动 sidecar 并铸租约。
///
/// 语义（依次）：
/// 1. 插件必须存在（否则 `E_UNKNOWN_PLUGIN`）；
/// 2. 插件类型必须是 `Process`，否则 `E_PLUGIN_TYPE_NO_RUNTIME`——**诚实失败**：
///    不为 js/rust/wasm 插件伪造 pid，也不静默返回一个"看起来成功"的句柄；
/// 3. 插件必须**可用**（`ENABLED` / `RUNNING`），否则 `E_PLUGIN_DISABLED`
///    （见下方"可用性门"）；
/// 4. 缺少可信验签材料（签名 / sha256 / ABI）时拒绝启动（§4.7 硬约束）；
/// 5. 同一插件**已有活租约 → 原样返回既有 lease**（幂等，理由见
///    `Registry::runtime_ensure_lease`）：只有一种行为，不出现"有时返回旧的、
///    有时报错"的二义；
/// 6. 需要（重新）启动且崩溃窗口超限 → `E_PLUGIN_DISABLED` 并落到
///    `ERRORED_USER_CONFIRM`（见下方"崩溃预算"）；
/// 7. **真的起了新进程**时投递既有事件 `ATTACH`，把状态推到 `RUNNING`
///    （进程插件没有 webview 可上报，只能宿主自己投；幂等返回既有租约时不投）。
///    见 [`attach_after_spawn`]。
///
/// # 可用性门（P0-2）
///
/// **一条规则**：注册表状态不是 `ENABLED` / `RUNNING` 就不许起进程。判定直接用
/// 既有的 `PluginEntry::is_active()`（`call_begin` 的即时拒绝用的是同一个谓词——
/// 不新造状态、不新造谓词，两处语义因此不可能漂移）。
///
/// 为什么把"崩溃预算的人工闸门"并进这一条：`ERRORED_USER_CONFIRM` 之所以被拒，
/// 正是因为它不 active；**人工确认闸门由本门覆盖，不再是独立分支**——两条部分
/// 重叠的规则迟早会有人只改一处。
///
/// 由此得到的**调用顺序**（宿主 UI / 恢复流程必须遵守）：
/// **先让状态可用（`host_registry_admin` 的 `enable`，或安全模式下的
/// `host_recover_trial_enable`），再 spawn。** 各场景：
/// - `INSTALLED`（装完没启用）→ 被拒：先 enable；
/// - safemode 期间（`DISABLED` + `disabledBySafemode`）→ 被拒：先
///   `host_recover_trial_enable`（试验性启用）或用户确认后的 enable；
/// - 崩溃 → `ERRORED_RETRYABLE` / `ERRORED_USER_CONFIRM` → 被拒：先 enable
///   （状态机的重试预算决定 enable 会落到 `ENABLED` 还是再次 `ERRORED_USER_CONFIRM`）。
///
/// 反向保证（在 `tauron-host` 侧，不在本命令里）：状态一旦离开 `ENABLED`/`RUNNING`
/// （禁用 / 安全模式对账 / 卸载 / 错误迁移），**仍在跑的 sidecar 会被终止**——
/// 所以本门不是"只管住新的 spawn、老的进程照跑"的半截门。
///
/// # 崩溃预算
///
/// 可用性门之外还有一道**预算**门（二者不重叠：一个管状态、一个管次数）：
/// `ProcRuntime::is_crash_exceeded`（`CrashLimit` 定稿值 3 次 / 5min）为真即拒绝，
/// 并投 `Event::ErrorFatal` 把插件落到 `ERRORED_USER_CONFIRM`（见
/// [`refuse_exhausted_crash_budget`]）。只在**需要启动新进程**时判定：已有活租约的
/// 调用是纯查询语义（把句柄交回去），不该被预算拦下。
pub fn cmd_runtime_spawn(
    state: &PluginRuntimeState,
    plugin_id: &str,
    profile: &RuntimeSpawnProfile,
) -> HostResult<RuntimeHandle> {
    guard("runtime_spawn", || {
        let id = PluginId::new(plugin_id)?;
        // 条目快照在 `runtime` 锁**之外**取（锁序 `pending → streams → runtime`；
        // `entries` 不参与该链，先取完再进 runtime 域，绝不反向）。
        let entry = state.registry.require(&id)?;

        if entry.manifest.plugin_type != PluginType::Process {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME,
                format!(
                    "插件 `{id}` 的类型是 `{}`，没有进程执行器（只有 `process` 型插件可 spawn）。\
                     这不是配置问题：该类型需要自己的执行器（js → webview 运行时 / rust → 宿主内 / wasm → 待接线）",
                    entry.manifest.plugin_type
                ),
            ));
        }

        // 可用性门：状态必须 ENABLED / RUNNING（既有谓词，不另造）。
        // 复用 `PluginEntry::is_active()`——它的实现就是 `Enabled | Running`，且
        // `call_begin` 的即时拒绝用的是同一个谓词（两处语义不可能漂移）。
        // 注意耦合：该谓词的文档讲的是"占用活跃身份槽位"，若哪天它的定义被改动
        // （例如把 INSTALLED 也算进去），这道门必须跟着重新审视——这就是不新造一个
        // 平行谓词换来的代价。
        // 状态**重新读一次**再判定：上面那份快照只用于类型与 sidecar，中间的
        // 校验/锁等待期间插件可能已被禁用——按陈旧快照放行就会绕过这道门。
        let live = state.registry.require(&id)?;
        if !live.is_active() {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_DISABLED,
                format!(
                    "插件 `{id}` 当前状态 `{}` 不可用（只有 `ENABLED` / `RUNNING` 才允许有 sidecar）：\
                     拒绝启动进程；请先让状态可用（`host_registry_admin` 的 `enable`，\
                     安全模式下用 `host_recover_trial_enable`）再 spawn",
                    live.state.state
                ),
            ));
        }

        // 缺省二进制 = manifest 声明的 sidecar（§4.2：C 类必须声明 `entry.sidecar`，
        // install 期已强校验，因此这条兜底分支**在实践中不可达**——保留它是为了
        // 「不 panic、不猜路径」：manifest 规则若放宽，这里会变成真路径而不是 panic 点。
        let binary_path = profile
            .binary_path
            .clone()
            .filter(|p| !p.trim().is_empty())
            .or_else(|| entry.manifest.entry.sidecar.clone())
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!(
                        "插件 `{id}` 未声明 `entry.sidecar`，`profile.binaryPath` 也为空：\
                         拒绝启动一个来源不明的二进制"
                    ),
                )
            })?;

        let cfg = profile.to_spawn_config(&binary_path);
        // spawn 前校验：签名 / sha256 / ABI。校验放在判定租约**之前**——
        // 不合格的载荷连"是否已有租约"都不该被它探测到（也就不会伪造出幂等假象）。
        validate_spawn_config(&cfg).map_err(proc_error_to_host)?;

        // ABI 契约比对（`tauron_proc::validate_abi` 的生产调用点）：
        // expected = 宿主当前支持的 sidecar ABI 契约（框架常量；前端可从
        // `@tauron/host` 的 `SIDECAR_ABI_CONTRACT` 取到同一份值）；
        // actual   = 调用方在 `profile.abi` 里声明的指纹。
        // 不符 → `E_ABI_MISMATCH`（**不是** `E_INSTALL_FAILED`：ABI 不匹配是版本
        // 兼容问题，调用方该升级插件/宿主，而不是重装）。
        //
        // 诚实边界：`profile.abi` 是调用方自报的，本校验挡的是"配置错配 / 前端用了
        // 旧模板"，不是"恶意调用方伪造 ABI"——后者需要 sidecar 在 RPC 握手时自报
        // 指纹（尚未实现）。与 `validate_spawn_config` 同属"配置一致性"层。
        validate_abi(&current_abi_contract(), &cfg.abi).map_err(proc_error_to_host)?;

        // 预算门（只拦"需要启动新进程"的调用；已有活租约是纯查询语义）。
        // 状态侧已由上面的可用性门覆盖（`ERRORED_USER_CONFIRM` 也不 active）。
        if state.registry.runtime_needs_restart(&id) && state.proc_runtime.is_crash_exceeded(plugin_id)
        {
            return Err(refuse_exhausted_crash_budget(state, &id, plugin_id));
        }

        // `started` 是**锁内**判定出来的"本次真的起了新进程"（幂等返回既有活租约
        // 时为 false），据此决定要不要把状态推到 RUNNING——见 `attach_after_spawn`。
        let (handle, started) = state.registry.runtime_ensure_lease(&id, || {
            state
                .proc_runtime
                .spawner()
                .spawn(&cfg)
                .map(|p| p.pid)
                .map_err(proc_error_to_host)
        })?;
        if started {
            attach_after_spawn(state, &id)?;
        }
        Ok(handle)
    })?
}

/// `host_runtime_spawn` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 判定必须在启动面**之前**：本命令会按调用方给的 `plugin_id` 启动一个可执行
/// 文件。若插件 webview 可调用，任何插件都能启动别的插件的 sidecar（越权执行
/// 原语）——拒绝路径**一次都不许到达启动面**（有测试用 fake spawner 断言
/// `call_count == 0`）。
pub fn cmd_runtime_spawn_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
    profile: &RuntimeSpawnProfile,
) -> HostResult<RuntimeHandle> {
    require_main_window(caller, "host_runtime_spawn")?;
    cmd_runtime_spawn(state, plugin_id, profile)
}

/// 真起了新进程之后，把插件从 `ENABLED` 推到 `RUNNING`（既有迁移 `ATTACH`）。
///
/// **为什么必须由宿主投**：js/rust 插件的 `ATTACH` 由 webview 上报，进程插件
/// **没有 webview**——宿主要是也不投，注册表就会停在 `ENABLED` 而进程已经在跑
/// （UI 显示"已启用"、事实是"运行中"），状态机与事实不一致。这条与可用性门合起来
/// 才是闭环：可用状态才能起进程（`ENABLED`/`RUNNING`），起了进程状态就变 `RUNNING`。
///
/// **只在真起进程时投**（幂等返回既有活租约那条路径不投）：那种情况状态本来就是
/// `RUNNING`，再投一次是无意义的重复事件。`started` 由
/// [`tauron_host::registry::Registry::runtime_ensure_lease`] 在锁内判定，避免并发
/// spawn 时两边都以为自己起了进程而各投一次。
///
/// **非法迁移不伪造事件**：唯一现实入口是"状态已经是 `RUNNING`"（`RUNNING + ATTACH`
/// 在迁移表里没有规则，例如插件自报恢复并先自己 `ATTACH` 过，随后宿主才发现旧租约已崩
/// 并换了新进程）。此时**目标态已达成**，保持原状即正确；状态机把这次非法迁移计进
/// `PluginState::illegal_transitions`（`host_registry_list` 可查），因此不静默——
/// 但这里**不会**为了"让状态好看"去伪造事件或直接改状态。
///
/// 唯一的错误路径是**条目已消失**（启动期间被并发卸载/清除）。那不是可忽略的噪声：
/// 刚铸的租约指向一个已经不存在的插件，进程再没人管——于是**回收刚铸的租约**（终止
/// 进程）后如实返回错误，而不是把一个孤儿句柄交给调用方。
fn attach_after_spawn(state: &PluginRuntimeState, id: &PluginId) -> HostResult<()> {
    match state.registry.report_event(id, Event::Attach) {
        Ok(_) => Ok(()),
        Err(e) => {
            let reclaimed = state.registry.runtime_remove(id);
            Err(HostError::new(
                e.code,
                format!(
                    "进程已启动，但插件条目在启动期间消失（并发卸载/清除），无法登记为 RUNNING：{}；\
                     已回收刚铸的租约（{}）",
                    e.message,
                    match reclaimed {
                        Some(h) => format!("pid {}", h.pid),
                        None => "租约已不在表中".to_string(),
                    }
                ),
            ))
        }
    }
}

/// 崩溃窗口超限 → 拒绝启动，并把插件落到 `ERRORED_USER_CONFIRM`（"需用户确认"）。
///
/// **为什么复用 `E_PLUGIN_DISABLED`**（不新增错误码）：这个变体的既有语义就是
/// "插件当前不可用，且只有宿主显式放行才能改变"（ADR-05：`enabled` 标志做即时
/// 拒绝）。崩溃预算耗尽正是同一件事的另一条成因，调用方的下一个动作也一样
/// （去主窗确认/启用，而不是立刻重试）——因此不该造第二个含义相同的码。同时它
/// **不可重试**（不在 `ErrorCode::retryable()` 里），正好保证框架层不会自动重试
/// 一个已经明确要求人工介入的失败。`proc_error_to_host` 早就把
/// `ProcError::CrashLimitExceeded` 映射到这个码，这里与它保持同一选择。
///
/// **落点复用 `Event::ErrorFatal`**（不新增事件）：既有迁移表里
/// `ENABLED/RUNNING + ERROR_FATAL → ERRORED_USER_CONFIRM`，语义就是"这个插件坏到
/// 不能再自动重试了，交给人"——正是崩溃预算耗尽该有的下场。选它而不是
/// `RetryExhausted`，是因为后者只有 `ERRORED_RETRYABLE` 一个来源，而本函数只在
/// 插件**仍可用**（`ENABLED`/`RUNNING`，可用性门已放行）时才会被调用，投
/// `RetryExhausted` 会成为非法迁移、落点只剩注释里的说法。副作用也正确：
/// `ErrorFatal` 带 `IncrementFailure`（"又失败一次"该记的账）。
/// 唯一的另一条分支是**试验期**（`TrialActive` 守卫）：此时落到 `DISABLED` +
/// 安全模式标志（D28 回落）而不是 `ERRORED_USER_CONFIRM`——这里如实报出实际落点，
/// 不假装落到了 `ERRORED_USER_CONFIRM`。
fn refuse_exhausted_crash_budget(
    state: &PluginRuntimeState,
    id: &PluginId,
    plugin_id: &str,
) -> HostError {
    let crashes = state.proc_runtime.crash_count(plugin_id);
    let limit = state.proc_runtime.crash_limit();
    let landing = match state.registry.report_event(id, Event::ErrorFatal) {
        Ok(outcome) if !outcome.illegal => format!("已落到 `{}`", outcome.to),
        // 非法迁移：状态机已计数（`illegal_transitions`）。两种子情况分开说明。
        Ok(_) => match state.registry.find(id).map(|e| e.state.state) {
            Some(landed) if landed == LifecycleStateName::ErroredUserConfirm => {
                format!("已处于 `{landed}`（无需迁移）")
            }
            Some(current) => format!(
                "未能落到 `ERRORED_USER_CONFIRM`：当前状态 `{current}` 对 `ERROR_FATAL` \
                 没有规则（非法迁移已计数），需人工处理"
            ),
            None => "未能落到 `ERRORED_USER_CONFIRM`：插件条目已消失".to_string(),
        },
        Err(e) => format!("未能投递 `ERROR_FATAL`（{}）：需人工处理", e.code),
    };
    HostError::new(
        ErrorCode::E_PLUGIN_DISABLED,
        format!(
            "插件 `{id}` 在 {}s 崩溃窗口内已崩溃 {crashes} 次（上限 {}），拒绝再次自动启动：{landing}；\
             用户在主窗确认后（`host_registry_admin` 的 `enable`）可再次 spawn",
            limit.window_secs, limit.max_crashes
        ),
    )
}

/// `host_runtime_health`：按 lease 查询进程健康。
///
/// - 未知 / 已失效的 lease → `E_LEASE_EXPIRED`（**不是** `E_CALL_NOT_FOUND`：
///   这是租约语义的边界）；
/// - 进程不存活时，**首次**观测到才记一次崩溃：投递 `RuntimeCrash` 生命周期事件
///   并让恢复引擎的 `consecutiveFailures` +1（复用既有引擎与状态机，不新造计数）；
/// - 崩溃检测是**轮询式**的（没有后台监控线程）：不被调用的 health 不会发现死亡。
///
/// 返回里带**全局**的租约回收留痕（`reap`）：终止失败的痕迹必须能被宿主 UI 看到，
/// 否则"不静默吞"只是写在注释里。挂在 health 上而不是新开命令，是为了不再牵动
/// 能力表/授权档位/TS 镜像（`RuntimeHealth` 本就是这条命令的既有返回）。
pub fn cmd_runtime_health(state: &PluginRuntimeState, lease: &str) -> HostResult<RuntimeHealth> {
    guard("runtime_health", || {
        let entry = state.registry.runtime_lease(lease)?;
        let alive = state.proc_runtime.spawner().is_alive(entry.pid);

        if !alive && state.registry.runtime_mark_crashed(lease)? {
            state.proc_runtime.record_crash(&entry.plugin_id);
            deliver_runtime_crash(state, &entry.plugin_id);
        }

        Ok(RuntimeHealth {
            alive,
            pid: entry.pid,
            crashes: state.proc_runtime.crash_count(&entry.plugin_id),
            consecutive_failures: state.recovery.lock().counter().consecutive_failures,
            reap: state.registry.runtime_reap_stats(),
        })
    })?
}

/// `host_runtime_health` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 为什么同属特权：返回里有 `pid`（暴露宿主侧进程标识）且会驱动崩溃检测
/// （`record_crash` + 崩溃投递），插件主体不该能借此查询/扰动别的插件的进程。
pub fn cmd_runtime_health_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    lease: &str,
) -> HostResult<RuntimeHealth> {
    require_main_window(caller, "host_runtime_health")?;
    cmd_runtime_health(state, lease)
}

/// 崩溃的**投递**（两条既有通道，都不新造计数）：
///
/// 1. `Event::RuntimeCrash` → 注册表状态机（`plugin.state` 的唯一写入者）。
///    崩溃进程的插件据此离开 RUNNING/ENABLED 进入 ERRORED（或按 D28 回落
///    `disabled-by-safemode`），副作用是 `retry_count` / `trial_failures` 等
///    既有预算计数——**不新增计数器**；
/// 2. `RecoveryEngine::record_boot_failure(Some(plugin_id))` → 与
///    `host_recover_report` 的 failure 分支**同一条**引擎路径，因此
///    `consecutiveFailures` 与安全模式判定天然同源（不会出现两套判定各自为政）。
///
/// 之后做一次阶段对账 + 落盘：判定必须落到插件侧（`reconcile_recovery_phase`
/// 是唯一途径），计数必须跨进程可见（重启后安全模式才可能触发）。
///
/// 事件的非法迁移（例如插件停在 INSTALLED）由状态机计数而不报错——**不静默**：
/// `illegal_transitions` 是可读的，且 `consecutiveFailures` 照样 +1。
///
/// 唯一可能的投递失败是**插件条目已消失**（`report_event` 返回 `E_UNKNOWN_PLUGIN`，
/// 只可能发生在并发卸载把条目摘掉、租约尚未回收的窗口里）。此时状态机上已经没有
/// 可迁移的对象，因此这里**不**把 health 升格成错误：崩溃计数与租约事实已经确定，
/// 返回错误只会让调用方丢掉这些事实（失败面由"插件条目已消失"本身可见）。
fn deliver_runtime_crash(state: &PluginRuntimeState, plugin_id: &str) {
    if let Ok(id) = PluginId::new(plugin_id) {
        let _ = state.registry.report_event(&id, Event::RuntimeCrash);
    }
    state.recovery.lock().record_boot_failure(Some(plugin_id));
    reconcile_recovery_phase(state);
    persist_recovery_engine(state);
}

/// `ProcError` → 宿主结构化错误（跨 IPC 的错误码表）。
///
/// **映射是粗的**（错误码只有两位数的粒度），但方向是确定的，且 `message` 保留
/// 原始原因：
/// - 配置不合格 → `E_INVALID_MANIFEST`（调用方改载荷即可）；
/// - **ABI 契约不匹配 → `E_ABI_MISMATCH`**（独立成码，不与安装失败合并：这是
///   版本兼容问题，调用方该升级插件/宿主，而不是重装——前端据此才能给出正确提示）；
/// - 验签 / hash 失败 → `E_INSTALL_FAILED`（与 `install()` 同类）；
/// - 崩溃预算耗尽 → `E_PLUGIN_DISABLED`（与 D28「回落 disabled」同义）；
/// - 其余（启动失败 / 超时 / 心跳丢失 / 进程终止）→ `E_INSTALL_FAILED`。
///
/// 未映射成 `E_HOST_PANIC`：那会**谎报**“宿主 panic”并把它标成可重试
/// （`retryable() == true`），前端会拿一个确定性的启动失败去无限重试。
fn proc_error_to_host(e: ProcError) -> HostError {
    let code = match &e {
        ProcError::InvalidSpawnConfig(_) => ErrorCode::E_INVALID_MANIFEST,
        ProcError::AbiMismatch { .. } => ErrorCode::E_ABI_MISMATCH,
        ProcError::SignatureInvalid
        | ProcError::HashMismatch
        | ProcError::SpawnFailed(_)
        | ProcError::ProcessTerminated(_)
        | ProcError::Timeout(_)
        | ProcError::HeartbeatLost(_)
        | ProcError::FrameTooLarge { .. }
        | ProcError::StdoutPollution(_)
        | ProcError::ConcurrencyLimit { .. }
        | ProcError::ConfigParse(_) => ErrorCode::E_INSTALL_FAILED,
        ProcError::CrashLimitExceeded { .. } => ErrorCode::E_PLUGIN_DISABLED,
    };
    HostError::new(code, format!("进程执行器失败：{e}"))
}

/// 把恢复引擎状态落盘（**锁顺序 `recovery → recovery_store`，两把锁绝不重叠**，
/// 与 `cmd_recover_report` 同一写法）。
fn persist_recovery_engine(state: &SubstrateState) {
    let engine_snapshot = state.recovery.lock().to_json();
    state.recovery_store.lock().save_json(&engine_snapshot);
}

/// `host_events_publish`：发布事件（self 档）。
///
/// 使用可靠语义（`publish_request`）：溢出返回结构化错误。
pub fn cmd_events_publish(
    state: &SubstrateState,
    publisher: &str,
    topic: &str,
    payload: serde_json::Value,
) -> HostResult<PublishResult> {
    guard("events_publish", || {
        let bus = state.bus.lock();
        bus.publish_request(publisher, topic, payload)
    })?
}

/// `host_events_subscribe`：订阅事件（self 档）。
pub fn cmd_events_subscribe(
    state: &SubstrateState,
    subscriber: &str,
    window: &str,
    topic: &str,
) -> HostResult<SubscribeOutcome> {
    guard("events_subscribe", || {
        let bus = state.bus.lock();
        bus.subscribe(subscriber, window, topic)
    })?
}

/// `host_events_unsubscribe`：退订事件（self 档）。
pub fn cmd_events_unsubscribe(
    state: &SubstrateState,
    token: &str,
) -> HostResult<()> {
    guard("events_unsubscribe", || {
        let bus = state.bus.lock();
        bus.unsubscribe(token)
    })?
}

/// 按订阅者回收多选择器订阅的分组登记（§8-3 零悬挂）。
///
/// 两个必调点：**窗口关闭**（`cleanup_closed_window`）与**插件卸载**
/// （`cmd_registry_admin` 的 Uninstall/Purge）。两处都会先经
/// `dispose_subscriber` 清掉核心订阅——分组登记若不跟着回收，表里留下的
/// 全是指向已消亡 token 的死条目，随开/关窗循环无限累积。
/// 回收键与 wire 层的建组键同源（`plugin_id_of(label)`），主窗的分组也会
/// 在主窗销毁时随 label 原样值被回收。
pub fn prune_subscription_groups(state: &SubstrateState, subscriber: &str) {
    state
        .subscription_groups
        .lock()
        .retain(|_, g| g.subscriber != subscriber);
}

/// `host_events_drain`：拉取本插件待投递帧（self 档）。
///
/// 这是订阅链路的**取件步骤**：`host_events_subscribe` 只是把帧排进
/// 每订阅者队列，前端必须周期性（或订阅后）调用本命令取走帧，
/// 否则队列只进不出。`kind` 为 `event` / `request` / `state`。
pub fn cmd_events_drain(
    state: &SubstrateState,
    subscriber: &str,
    kind: &str,
) -> HostResult<Vec<Frame>> {
    let parsed_kind = ChannelKind::parse(kind).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("未知通道类型 `{kind}`（expected: event | request | state）"),
        )
    })?;
    guard("events_drain", || {
        let bus = state.bus.lock();
        bus.drain(subscriber, parsed_kind)
    })?
}

// ──────────────────────────────────────────────────────────────────────────
// 设置（R7-2：走 `tauron-settings` 的 Store，激活孤儿 crate）
// ──────────────────────────────────────────────────────────────────────────

/// 宿主设置的命名空间（`tauron-settings` 的伪插件 id）。
///
/// 一个就够：宿主设置的键是开放集合（插件各自带自己的键），不需要 per-plugin
/// 命名空间隔离；`SettingsStore` 的四层合并与版本迁移在单命名空间内同样成立。
pub const HOST_SETTINGS_NAMESPACE: &str = "host.settings";

/// 宿主设置文档的 **v1** schema 版本。
///
/// v1 的键是**裸键**（可以含 `.`，例如 `plugin:p.theme`），文档平铺存储。
/// 这正是 R7 之前裸 `HashMap` 的契约。
pub const HOST_SETTINGS_SCHEMA_V1: &str = "1.0.0";

/// 宿主设置文档的**当前**（v2）schema 版本。
///
/// v2 的键是**单段转义**键（见 [`settings_path`]）：一键 = 一段 = 一叶。
pub const HOST_SETTINGS_SCHEMA_V2: &str = "2.0.0";

/// v1 与 v2 共用的文档 schema。
///
/// 两份都是 `additionalProperties: true` 的开放对象——宿主设置的键宿主编译期
/// 枚举不出来。版本号在这里承载的是**键的编码契约**（v1 裸键 / v2 转义键），
/// 不是字段集合；升版的目的是让「键编码变了」这件事可迁移、可审计，
/// 而不是把开放键集合假装成封闭的。
fn host_settings_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": true,
    })
}

/// 把一个设置键编码成 `SettingsStore` 的**单段**点路径。
///
/// 为什么必须转义：`SettingsStore` 用点路径寻址，`.` 是分隔符。既有键
/// （`plugin:p.theme`）含 `.`，直接透传会被拆成嵌套对象——
/// - 前缀键读取会从 `Null` 变成对象（平铺语义变了）；
/// - `a` 已是标量时再写 `a.b`，`merge::write_path` 的
///   「中间节点必须是对象」断言会 panic（被 `guard` 拦成 `E_HOST_PANIC`）。
///
/// 转义是**注入式**的（`%` 先转），因此可逆且不产生碰撞：
/// `a.b` → `a%2Eb`，而字面键 `a%2Eb` → `a%252Eb`。
///
/// 转义表：`%` → `%25`、`.` → `%2E`、`$` → `%24`
/// （`$` 是 `$unset` 的保留前缀，`validate_path` 禁止段以它开头）。
pub fn settings_path(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for ch in key.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '.' => out.push_str("%2E"),
            '$' => out.push_str("%24"),
            _ => out.push(ch),
        }
    }
    out
}

/// v1 → v2：把裸键文档改写成转义键文档（`%unset` 保留键原样放行）。
///
/// v1 文档是平铺对象，键就是设置键本身，所以这里做的是**逐键改名**，
/// 而不是路径重写。
fn migrate_host_settings_v1_to_v2(user: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(m) = user else {
        return user.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, v) in m {
        if k == tauron_settings::UNSET_KEY {
            // v1 文档由裸 `HashMap` 写入，从不含 `$unset`；真出现了就原样留下
            // （不猜它的段语义），免得把数据改坏。
            out.insert(k.clone(), v.clone());
        } else {
            out.insert(settings_path(k), v.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// 注册宿主设置命名空间的 schema（v1 → v2 升版）与 v1→v2 迁移。
fn install_host_settings_schema(store: &mut SettingsStore) {
    // 先 v1 再 v2：注册表要求版本严格递增，这条链本身就是升版的证据。
    store
        .register(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V1, &host_settings_schema())
        .expect("内置的宿主设置 v1 schema 必须可编译");
    store
        .register(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V2, &host_settings_schema())
        .expect("内置的宿主设置 v2 schema 必须可编译");
    store.register_migration(
        HOST_SETTINGS_NAMESPACE,
        Migration::new(
            HOST_SETTINGS_SCHEMA_V1,
            HOST_SETTINGS_SCHEMA_V2,
            migrate_host_settings_v1_to_v2,
        ),
    );
}

/// `SettingsError` → `HostError`。
///
/// 复用 `E_INVALID_MANIFEST`，**不新增错误码**（TS 门禁要求两侧错误码同序）；
/// 这与 `tauron-settings` 自己的约定一致（见该 crate 的模块文档）。
fn settings_to_host_error(e: SettingsError) -> HostError {
    HostError::new(
        ErrorCode::E_INVALID_MANIFEST,
        format!("设置被拒：{e}"),
    )
}

/// **接手一份旧版（v1）宿主设置文档**（R7-2：settings 可迁移）。
///
/// 用途：宿主从磁盘读到的旧版配置走这里进 Store。写入用户层并把**数据版本**
/// 标成 [`HOST_SETTINGS_SCHEMA_V1`]——不标注的话 [`host_settings_migrate`]
/// 无从知道起点，只能拒绝迁移（宁可不迁，也不猜）。
///
/// `doc` 必须是对象（键 = 设置键，值 = 设置值）；否则返回 `E_INVALID_MANIFEST`，
/// 不静默退化成空文档。
pub fn host_settings_adopt_legacy(
    state: &SubstrateState,
    doc: serde_json::Value,
) -> HostResult<()> {
    if !doc.is_object() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "旧版宿主设置文档必须是对象（键 = 设置键）".to_string(),
        ));
    }
    let mut store = state.settings.lock();
    store.set_layer(HOST_SETTINGS_NAMESPACE, tauron_settings::LayerKind::User, doc);
    store.set_data_version(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V1);
    Ok(())
}

/// 把宿主设置从已标注的数据版本迁到**当前** schema 版本。
///
/// # 返回
///
/// 实际应用的迁移步数（已是当前版本、或没有数据时为 `0`）。
///
/// 全有或全无：链路缺失或迁移结果过不了 schema 校验时用户层一个字节都不改，
/// 返回 `E_INVALID_MANIFEST`（不新增错误码）。
pub fn host_settings_migrate(state: &SubstrateState) -> HostResult<usize> {
    let mut store = state.settings.lock();
    store
        .migrate(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V2)
        .map_err(settings_to_host_error)
}

/// 当前宿主设置的数据版本（诊断用；`None` = 既无数据也无标注）。
pub fn host_settings_data_version(state: &SubstrateState) -> Option<String> {
    state
        .settings
        .lock()
        .data_version(HOST_SETTINGS_NAMESPACE)
        .map(str::to_string)
}

/// `host_settings_get`：读取设置。
///
/// **线形不变**（前端契约）：入参 `key: string`，返回任意 JSON；未写过的键
/// 返回 `Null`（不是报错）。读路径不校验 schema——缺键不是错误。
pub fn cmd_settings_get(
    state: &SubstrateState,
    key: &str,
) -> HostResult<serde_json::Value> {
    guard("settings_get", || {
        let path = settings_path(key);
        let store = state.settings.lock();
        let value = store
            .get_key(HOST_SETTINGS_NAMESPACE, &path)
            .map_err(settings_to_host_error)?;
        Ok(value.unwrap_or(serde_json::Value::Null))
    })?
}

/// `host_settings_set`：写入设置。
///
/// **线形不变**（前端契约）：入参 `key: string, value: any`，返回 `()`。
/// 写入**经 [`SettingsStore`]**（不再是裸 `HashMap`）：键会被编码成合法点路径，
/// 值要过命名空间 schema，落盘形态由 Store 的四层结构决定。
pub fn cmd_settings_set(
    state: &SubstrateState,
    key: &str,
    value: serde_json::Value,
) -> HostResult<()> {
    guard("settings_set", || {
        if key.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "设置键不能为空（调用方可能传了 undefined/null）".to_string(),
            ));
        }
        let path = settings_path(key);
        let mut store = state.settings.lock();
        store
            .set(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_NAMESPACE, &path, &value)
            .map_err(settings_to_host_error)?;
        Ok(())
    })?
}

/// `host_settings_adopt_legacy`：把宿主磁盘上读到的旧版设置文档交给 Store。
///
/// 迁移能力的**唯一入口**。缺了它，[`host_settings_adopt_legacy`] 与
/// [`host_settings_migrate`] 就只是 Rust 测试能碰到的孤儿逻辑——真实宿主宁可
/// 自己拼点路径写设置，也不会去调一个没有渠道的迁移。
///
/// **线形**：入参 `doc: object`（键 = 设置键，值 = 设置值），返回 `()`。
/// 非对象文档返回 `E_INVALID_MANIFEST`（不静默退化成空文档）。写入后数据版本
/// 标注为 [`HOST_SETTINGS_SCHEMA_V1`]，[`cmd_settings_migrate`] 才知道起点。
pub fn cmd_settings_adopt_legacy(
    state: &SubstrateState,
    doc: serde_json::Value,
) -> HostResult<()> {
    guard("settings_adopt_legacy", || {
        host_settings_adopt_legacy(state, doc)
    })?
}

/// `host_settings_adopt_legacy` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// **主窗专属**：它接手的是**整份**文档（不是某一个键），会把用户层整体改写并
/// 把数据版本重标为 v1——插件主体能调它等于能清空/重置整个宿主的设置层，也等于
/// 能把 v2 数据倒回 v1（拒绝服务）。设置族的**按键**访问权限（插件只能读写自己
/// 命名空间）在这里没有意义：整份文档跨所有插件与宿主键。
pub fn cmd_settings_adopt_legacy_as(
    caller: &Caller,
    state: &SubstrateState,
    doc: serde_json::Value,
) -> HostResult<()> {
    require_main_window(caller, "host_settings_adopt_legacy")?;
    cmd_settings_adopt_legacy(state, doc)
}

/// `host_settings_migrate`：把设置迁到当前 schema 版本。
///
/// **线形**：无入参，返回迁移**步数**（数字）。`0` = 已是最新（或本就无数据），
/// 且**幂等**（重复调用始终是 0）。全有或全无：链路缺失 / 迁移结果过不了新
/// schema 校验时用户层一个字节都不改，返回 `E_INVALID_MANIFEST`。
///
/// 与 [`cmd_settings_adopt_legacy`] 一样受 [`guard`] 保护——迁移要走 schema
/// 编译与用户数据改写，一旦 panic 必须是 `E_HOST_PANIC` 而不是把 panic  unwind
/// 穿过 IPC 边界。
pub fn cmd_settings_migrate(state: &SubstrateState) -> HostResult<usize> {
    guard("settings_migrate", || host_settings_migrate(state))?
}

/// `host_settings_migrate` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// **主窗专属**：迁移会整体改写用户层（键编码契约换版），且会把数据版本推到
/// 当前版本——插件主体调它等于能对宿主设置做一次全局不可逆改写。与
/// [`cmd_settings_adopt_legacy_as`] 同一条理由：整份文档级操作无法表达
/// 「只准动自己的键」。
pub fn cmd_settings_migrate_as(caller: &Caller, state: &SubstrateState) -> HostResult<usize> {
    require_main_window(caller, "host_settings_migrate")?;
    cmd_settings_migrate(state)
}

/// `host_settings_get` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 主窗可读任意键；插件只能读自己命名空间（`plugin:<自己 id>`）内的键。
/// 越界在**读之前**拒绝（读也不放过：别人的设置值本身就是隐私）。
/// 命令的**线形不变**（入参 `key`、返回任意 JSON 或 `Null`）。
pub fn cmd_settings_get_as(
    caller: &Caller,
    state: &SubstrateState,
    key: &str,
) -> HostResult<serde_json::Value> {
    require_settings_key_scope(caller, key)?;
    cmd_settings_get(state, key)
}

/// `host_settings_set` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 主窗可写任意键；插件只能写自己命名空间内的键。越界在**写之前**拒绝——
/// 拒绝路径**不产生任何写入副作用**（有测试断言 Store 内容逐字不变）。
/// 命令的**线形不变**（入参 `key` + `value`，返回 `()`）。
pub fn cmd_settings_set_as(
    caller: &Caller,
    state: &SubstrateState,
    key: &str,
    value: serde_json::Value,
) -> HostResult<()> {
    require_settings_key_scope(caller, key)?;
    cmd_settings_set(state, key, value)
}

/// `host_notify`：发送通知（P0-5：对接 tauron-notify crate；R7-3：接
/// [`DispatchSink`]）。
///
/// **顺序不变量**：先入环形缓冲，再走系统通知分发。这条不变量的落点在
/// [`tauron_notify::dispatch`] 内部（「先 push 再 send」），本函数不再自己
/// push 一次——重复插入会被 Store 当成重复 id 拒掉，等于把顺序又倒回去了。
///
/// **降级语义**：系统通知失败（不支持 / 未授权 / 致命错误）只让
/// `DispatchOutcome` 变成 `Degraded`，通知照旧留在环形缓冲里，本命令
/// **绝不**因为系统通知失败而返回错误。
///
/// 未注入 `DispatchSink`（底座-only 宿主 / 单测）时没有系统通知通道：
/// 只入应用内缓冲，**不**记 dispatch 日志（没有尝试就没有日志）。
///
/// **入口**：wire 层转调 [`cmd_notify_as`]（署名必须是自己 / 宿主），本函数是
/// **不过身份**的核心。
pub fn cmd_notify(
    state: &SubstrateState,
    plugin_id: &str,
    title: &str,
    body: &str,
) -> HostResult<()> {
    guard("notify", || {
        let entry = NotifyEntry {
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: plugin_id.to_string(),
            kind: NotifyKind::Info,
            title: title.to_string(),
            message: body.to_string(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            read: false,
            data: serde_json::Value::Null,
        };
        {
            let mut store = state.notify_store.lock();
            match state.notify_sink.get() {
                // 有系统通道：交给 `dispatch`（它内部先入缓冲再 send，
                // 并落一条分发日志）。
                Some(sink) => {
                    dispatch(&mut store, sink.as_ref(), entry);
                }
                // 无系统通道：只有应用内这一条路。
                None => {
                    if let Err(e) = store.push(entry) {
                        return Err(HostError::new(
                            ErrorCode::E_HOST_PANIC,
                            format!("通知写入失败: {e}"),
                        ));
                    }
                }
            }
        }

        // 向后兼容：同时写入 NotificationRecord（**只写不读**的兼容日志，
        //    故必须有上限，否则每次 host_notify 都会永久占一份内存）。
        let mut notifications = state.notifications.lock();
        notifications.push(NotificationRecord {
            plugin_id: plugin_id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        if notifications.len() > MAX_NOTIFICATION_LOG {
            let excess = notifications.len() - MAX_NOTIFICATION_LOG;
            notifications.drain(0..excess); // 保留最新 MAX_NOTIFICATION_LOG 条
        }

        Ok(())
    })?
}

/// `host_notify` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// `pluginId` 是通知的**署名**：通知中心按它显示来源、做"来自哪个插件"的过滤，
/// 用户据此决定信不信这条通知。不判定的话，任何插件都能：
///
/// - 以**宿主**名义发通知（伪装成宿主告警 → 钓鱼 / 骗点击）；
/// - 以**别的插件**名义发通知（栽赃：把恶意行为挂到别人名下）。
///
/// 所以规则不是"把命令关成主窗专属"——插件本来就得以自己名义发通知——而是
/// **署名必须是自己**（[`require_self_plugin_scope`]）。拒绝路径**零副作用**：
/// 通知不入环形缓冲、不写兼容日志、不触发系统通知分发（有测试断言条数不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_notify_as(
    caller: &Caller,
    state: &SubstrateState,
    plugin_id: &str,
    title: &str,
    body: &str,
) -> HostResult<()> {
    require_self_plugin_scope(caller, "host_notify", Some(plugin_id))?;
    cmd_notify(state, plugin_id, title, body)
}

/// 通知读端的**可见性过滤**（scoped-read 口径，与 `host_registry_list` 同族）。
///
/// # 为什么这里是"过滤"而不是"拒绝"
///
/// 与 [`require_main_window`] / [`require_self_plugin_scope`] 的语义**不同**，别混用：
/// 那两个是"拒绝或放行"，本函数是"**裁剪结果**"。插件看不见别人的通知是对的，但
/// "插件读自己的通知"本身是正当功能（通知中心就是干这个的），一刀切拒绝会把功能
/// 关掉——所以读端按可见性过滤，写端（`host_notify` / `host_notifications_read`）
/// 才按身份判定。
///
/// - **主窗**：全部可见（现状不变）；
/// - **插件**：只有 `pluginId == 自己` 的条目可见。
///
/// 返回**未分页**的可见集合（时间倒序）。分页由调用方在这之后做，原因见
/// `notifications_list_payload`：反过来先分页再过滤，别人的通知会把窗口占满，
/// 插件自己那几条反而被 `limit` 挤出去（"过滤了但自己的读不到"）。
pub fn visible_notifications<'a>(
    caller: &Caller,
    store: &'a NotifyStore,
) -> Vec<&'a NotifyEntry> {
    // `recent(len)` = 整个环（时间倒序）；不能用 `recent(limit)` 拿到"全部再看"。
    let all = store.recent(store.len());
    match caller.plugin_id() {
        // 主窗：全部可见。
        None => all,
        Some(me) => all.into_iter().filter(|e| e.plugin_id == me).collect(),
    }
}

/// `host_notifications_list` 的快照构造（两条入口共用）。
///
/// **计数与条目必须同源自可见集合**：`total` / `unread` 都在**过滤之后**重新数。
/// 只裁数组、留着全局 `unread`（甚至只留着全局 `total`）仍然是泄露——未读数本身
/// 就能推断"别的插件正在发通知"。
///
/// **`limit` 作用在过滤之后**：先按可见性筛出调用方的集合，再取其中最新 `limit` 条。
/// 因此插件的 `items.len()` 可以小于 `limit` 而 `total` 仍可能更大（分页语义：
/// `total` 是可见集合总数，`items` 是本页），这不矛盾——`total`/`unread` 始终是
/// 可见集合的真值，前端角标因此不会显示别人的条数。
///
/// **`dispatchLog` 同样过滤**：记录里只有 `entryId`（没有 `plugin_id`），所以按
/// `entryId` 反查条目归属。查不到（条目已被环形裁剪）时对插件**不显示**——无法归属
/// 的记录不能证明是自己的；主窗不受影响（保持"条目被裁剪后日志仍在"的既有语义）。
fn notifications_list_payload(
    state: &SubstrateState,
    caller: &Caller,
    limit: Option<usize>,
) -> HostResult<serde_json::Value> {
    guard("notifications_list", || {
        let store = state.notify_store.lock();
        let visible = visible_notifications(caller, &store);
        let total = visible.len();
        let unread = visible.iter().filter(|e| !e.read).count();
        let items: Vec<serde_json::Value> = visible
            .iter()
            .take(limit.unwrap_or(50))
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "pluginId": e.plugin_id,
                    "kind": e.kind.as_str(),
                    "title": e.title,
                    "message": e.message,
                    "ts": e.ts,
                    "read": e.read,
                    "data": e.data,
                })
            })
            .collect();
        let mine = caller.plugin_id();
        let dispatch_log: Vec<serde_json::Value> = store
            .dispatch_log()
            .iter()
            .filter(|r| match mine {
                // 主窗：日志不受影响（含"条目已被裁剪但日志仍在"的既有语义）。
                None => true,
                // 插件：只留能归属到自己的记录。
                Some(me) => store.get(&r.entry_id).is_some_and(|e| e.plugin_id == me),
            })
            .map(|r| {
                serde_json::json!({
                    "entryId": r.entry_id,
                    "outcome": r.outcome.as_str(),
                    "ts": r.ts,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "unread": unread,
            "total": total,
            "items": items,
            "dispatchLog": dispatch_log,
        }))
    })?
}

/// `host_notifications_list` 的**带身份过滤**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条要按身份过滤（危害原文）
///
/// 返回体里是**通知的正文**：`title` / `message` / `data`（`data` 里常见跳转链接、
/// 操作按钮的载荷）。不按身份过滤的话，任何插件调一次就能读到**别的插件**（以及
/// **宿主自己**）的通知正文——而通知正文恰恰是"宿主想对用户说什么"的通道，可能包含
/// 故障详情、待处理项、甚至带 token 的跳转链接。这不是"看见别人存在"这种元信息泄露，
/// 而是**内容**泄露。
///
/// 过滤而不是拒绝（见 [`visible_notifications`]）：插件要能读自己的通知，否则通知
/// 中心对插件不可用。`total` / `unread` / `dispatchLog` 与 `items` **同源**，避免
/// "数组裁了、角标还露着"的残留泄露。
///
/// 命令**线形不变**（入参仍是 `limit`，返回仍是 `{ unread, total, items, dispatchLog }`；
/// 只是插件视角下这四个值都变成"可见集合"上的真值）。
pub fn cmd_notifications_list_as(
    caller: &Caller,
    state: &SubstrateState,
    limit: Option<usize>,
) -> HostResult<serde_json::Value> {
    notifications_list_payload(state, caller, limit)
}

/// `host_notifications_list`：通知中心读取端（`host_notify` 的配对）。
///
/// 断链回归：通知存储此前**只写不读**——`host_notify` 落进环形缓冲就再无
/// 出口，未读计数只增不减、通知中心无处取数。`limit` 缺省 50，按时间倒序。
/// 线形键为 camelCase（`pluginId`），与 TS 契约一致（NotifyEntry 本体是
/// snake_case，此处逐字段映射）。
///
/// **`dispatchLog`（R7-3 新增字段）**：**系统通知分发尝试**的日志，一次
/// `host_notify` 一条，字段 `entryId` / `outcome`（`system` | `degraded` |
/// `failed`）/ `ts`。语义边界写清楚：
///
/// - 它是**尝试**的记录，不是通知本身：条目被环形裁剪后，它的分发记录仍留在
///   日志里（两个环互相独立，各有上限）；
/// - 未注入 `DispatchSink` 时不会有任何记录（没有尝试）；
/// - 它**不受** `limit` 影响（那是通知本身的条数上限）；日志由 Store 自己的
///   环形上限（`tauron_notify::DEFAULT_DISPATCH_LOG_CAPACITY` = 200）约束。
///
/// **入口**：wire 层转调 [`cmd_notifications_list_as`]（按身份过滤可见集合），
/// 本函数是**不过身份**的核心 = 主窗视角（全部可见），行为与 R7 之前逐字一致。
pub fn cmd_notifications_list(
    state: &SubstrateState,
    limit: Option<usize>,
) -> HostResult<serde_json::Value> {
    notifications_list_payload(state, &Caller::MainWindow, limit)
}

/// `host_notifications_read`：标记已读（`id` 缺省 = 全部）。
///
/// **入口**：wire 层转调 [`cmd_notifications_read_as`]（插件只能标记自己的通知），
/// 本函数是**不过身份**的核心。
pub fn cmd_notifications_read(
    state: &SubstrateState,
    id: Option<&str>,
) -> HostResult<serde_json::Value> {
    guard("notifications_read", || {
        let mut store = state.notify_store.lock();
        let marked = match id {
            Some(id) => usize::from(store.mark_read(id)),
            None => store.mark_all_read(),
        };
        Ok(serde_json::json!({ "marked": marked }))
    })?
}

/// `host_notifications_read` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// 这条是**写端**（改的是已读状态），所以与读端不同：不是过滤，而是判定。
///
/// - `id = None` 是"**全部标记已读**"——改的是**全局**未读状态（含宿主自己与所有
///   其他插件）。插件调一次就能把宿主的未读角标清零，用户再也看不到"有事没处理"；
/// - `id = Some(x)` 若 `x` 是**别人的**通知，改的就是别人的已读状态：那条通知在
///   通知中心里直接变成"已读"，等于**替别人把消息吞掉**（用户再也不会被提醒）。
///
/// 规则：主窗任意 `id` / `None`（宿主自己就是通知中心的完整主体）；插件
/// `Some(id)` 只能是**自己**的通知，`None` 一律拒绝（[`require_self_plugin_scope`]
/// 的 `None` 分支 = 宿主级/全局档）。
///
/// **一个必须说明的边界**：`id` 是**通知 id**（不是 pluginId），所以"是不是自己的"
/// 只能先反查条目归属。**查不到（未知 id / 已被环形裁剪）时对插件一律拒绝**——
/// 理由有两条：① 无法归属就证明不了是自己的（规则要求"只能是自己"）；② 若放行并
/// 返回 `marked: 0`，`marked` 就成了**存在性预言机**（`1` = 这个 id 存在，`0` = 不存在），
/// 那正是本批要收掉的信息泄露。主窗路径不变（未知 id → `marked: 0`，不 panic）。
///
/// 拒绝路径**零副作用**：别人的已读状态、全局未读计数一律不动（有测试断言）。
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_notifications_read_as(
    caller: &Caller,
    state: &SubstrateState,
    id: Option<&str>,
) -> HostResult<serde_json::Value> {
    // 归属反查只对插件有意义（主窗任意 id / None 都合法，不做多余查询）。
    if caller.plugin_id().is_some() {
        if let Some(nid) = id {
            let owner = state
                .notify_store
                .lock()
                .get(nid)
                .map(|e| e.plugin_id.clone());
            let Some(owner) = owner else {
                return Err(HostError::new(
                    ErrorCode::E_AUTH_DENIED,
                    format!(
                        "通知 `{nid}` 不存在或已过环形窗口，无法确认它是{} 自己的通知；\
                         对未知 id 放行会让 `marked` 变成存在性预言机（判定在任何副作用之前）",
                        caller.describe()
                    ),
                ));
            };
            // 已知归属：走与本批同一条判定（`Some(别人)` 拒绝）。
            require_self_plugin_scope(caller, "host_notifications_read", Some(&owner))?;
        } else {
            // `None` = 标记全部：全局状态，插件不得主张。
            require_self_plugin_scope(caller, "host_notifications_read", None)?;
        }
    }
    cmd_notifications_read(state, id)
}

/// `host_contributes_register`：注册贡献（self 档）。
pub fn cmd_contributes_register(
    state: &PluginRuntimeState,
    _plugin_id: &str,
    entry: ContributeEntry,
) -> HostResult<()> {
    guard("contributes_register", || {
        let mut contributes = state.contributes.lock();
        contributes.register(entry)
    })?
}

/// `host_contributes_list`：列出贡献（scoped-read 档）。
pub fn cmd_contributes_list(
    state: &PluginRuntimeState,
    kind: Option<&str>,
) -> HostResult<Vec<ContributeEntry>> {
    guard("contributes_list", || {
        let contributes = state.contributes.lock();
        let entries = match kind {
            Some(k) => contributes.list_by_kind(k).into_iter().cloned().collect(),
            None => contributes.list_all().to_vec(),
        };
        Ok(entries)
    })?
}

// ──────────────────────────────────────────────────────────────────────────
// 启动恢复（§4.14：持久化 + 崩溃检测 + 驱动信号）
// ──────────────────────────────────────────────────────────────────────────

/// 阶段对账结果（诊断用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseReconcileOutcome {
    /// 扫描到的注册表插件数。
    pub scanned: usize,
    /// 补发的 `SafemodeEnter`（引擎判定应禁用，注册表尚未标记）。
    pub entered: usize,
    /// 补发的 `SafemodeExit`（引擎判定应启用，注册表仍标记为安全模式禁用）。
    pub exited: usize,
    /// 被忽略的非法迁移（例如对已卸载插件补发，或两次调用之间状态已变）。
    pub ignored: usize,
}

fn phase_name(phase: BootPhase) -> &'static str {
    match phase {
        BootPhase::Normal => "正常启动",
        BootPhase::Safemode => "安全模式",
        BootPhase::Repairmode => "修复模式",
    }
}

/// 把恢复引擎的阶段判定**对账**到注册表生命周期状态机。
///
/// 两个 crate 各管一件事，本函数是唯一的桥：
/// - `tauron-recovery` 负责**判**——连续失败次数 → normal/safemode/repairmode；
/// - 注册表状态机负责**执行**——`Event::SafemodeEnter` →
///   `Action::SetSafemodeDisabled` → `disabled_by_safemode`。后者是
///   `PluginSummary.disabledBySafemode` 的**唯一权威来源**。
///
/// 它是**对账**而非事件处理器：逐个比较注册表实际标志与引擎判定，只在不一致时
/// 补发事件，因此可被任何路径安全、重复地调用（启动上报后、插件上报生命周期后、
/// 管理操作后）。非法迁移被计数而不报错——对已卸载插件补发 `SafemodeExit` 是
/// 合法情形，不是错误。
///
/// 引擎不认识的插件（例如本轮新装）会先按当前必需集合登记进引擎，再参与判定——
/// 决策覆盖面必须与注册表一致，否则新装插件会绕过安全模式。
fn reconcile_recovery_phase(state: &SubstrateState) -> PhaseReconcileOutcome {
    let mut out = PhaseReconcileOutcome::default();

    // 底座-only 宿主没有插件侧写回口 → 没有插件可对账。这是语义正确（无对象），
    // 不是静默跳过错误。
    let Some(sink) = state.plugin_flags.get() else {
        return out;
    };

    // 先取注册表快照（自持内存，不跨锁），再单独持 engine 锁取判定——
    // 两把锁从不重叠持有。
    let snapshots = sink.flag_snapshots();
    let plan: Vec<(PluginId, bool, bool)> = {
        let mut engine = state.recovery.lock();
        snapshots
            .into_iter()
            .filter_map(|(raw_id, disabled_by_safemode)| {
                let id = PluginId::new(&raw_id).ok()?;
                // 注册表是「有哪些插件」的唯一权威来源，恢复引擎只知道被显式
                // 登记过的插件。引擎不认识就按当前必需集合登记——不补齐的话，
                // 新装的插件会绕过安全模式判定（决策覆盖面必须与注册表一致）。
                if engine.plugin_state(id.as_str()).is_none() {
                    engine.register_plugin(id.as_str());
                }
                let should_enable = engine
                    .plugin_state(id.as_str())
                    .map_or(true, |s| s.is_enabled());
                Some((id, should_enable, disabled_by_safemode))
            })
            .collect()
    };

    for (id, should_enable, currently_disabled) in plan {
        out.scanned += 1;
        let event = if !should_enable && !currently_disabled {
            Some(Event::SafemodeEnter)
        } else if should_enable && currently_disabled {
            Some(Event::SafemodeExit)
        } else {
            None
        };
        let Some(event) = event else { continue };
        let entering = event == Event::SafemodeEnter;
        // 判定在底座（引擎）侧完成，写回口只负责把事件补发到插件侧。
        match sink.report_flag_event(id.as_str(), entering) {
            Some(true) => out.ignored += 1,
            Some(false) => {
                if entering {
                    out.entered += 1
                } else {
                    out.exited += 1
                }
            }
            None => out.ignored += 1,
        }
    }
    out
}

/// 恢复引擎当前状态的线形态（`host_recover_boot` 与 `host_recover_report` 共用）。
///
/// R7-1 新增字段 `lastContext`（**既有字段一个都没改名/删除**：前端已消费
/// `phase`/`counter`/`disabledPlugins`/`requiredPlugins`/`loadSource`/`persistence`）。
fn recovery_boot_payload(state: &SubstrateState) -> serde_json::Value {
    let engine = state.recovery.lock();
    let phase = engine.decide_boot_phase();
    let consecutive_failures = engine.counter().consecutive_failures;
    let safemode_failures = engine.counter().safemode_failures;
    // `disabled_plugins` 借用引擎内部 map 的 key，必须在锁内物化。
    let disabled_plugins: Vec<serde_json::Value> = engine
        .disabled_plugins()
        .into_iter()
        .map(|(id, st)| serde_json::json!({ "pluginId": id, "state": st.as_str() }))
        .collect();
    let required_plugins: Vec<String> = engine
        .required_plugins()
        .iter()
        .map(|s| s.clone())
        .collect();

    let store = state.recovery_store.lock();
    let persistence = serde_json::json!({
        "enabled": store.is_enabled(),
        "dir": store.dir().map(|p| p.to_string_lossy().into_owned()),
        "bootInFlight": store.in_flight,
        "lastError": store.last_error.clone(),
    });
    // 崩溃后诊断上下文：`RecoveryStore` 从标记文件读回来的**上一轮**关键事件
    // 摘要（含本次 `load` 识别到的那次崩溃）。锁定在 store 锁内物化。
    let last_context: Vec<serde_json::Value> = store
        .last_context
        .iter()
        .map(boot_context_entry_wire)
        .collect();

    serde_json::json!({
        // 线名与 TS `RecoveryBootResult` 消费字段逐字对齐（camelCase）。
        "phase": phase.as_str(),
        "phaseName": phase_name(phase),
        "counter": {
            "consecutiveFailures": consecutive_failures,
            "safemodeFailures": safemode_failures,
        },
        "disabledPlugins": disabled_plugins,
        "requiredPlugins": required_plugins,
        "loadSource": state.recovery_source.as_str(),
        "persistence": persistence,
        "lastContext": last_context,
    })
}

/// 一条关键事件摘要的线形态（camelCase）。
///
/// `failureKind` 是 [`BootContextEntry::failure_kind`] 的**派生**线名
/// （由 `phase` + `trial` 推出），不是新概念、不参与判定——只为前端少写一份
/// 分支逻辑。`pluginId` / `pluginState` 可以是 `null`（宿主自身 / 未登记）。
fn boot_context_entry_wire(c: &BootContextEntry) -> serde_json::Value {
    serde_json::json!({
        "ts": c.ts,
        "phase": c.phase.as_str(),
        "failureKind": c.failure_kind(),
        "pluginId": c.plugin_id,
        "trial": c.trial,
        "consecutiveFailures": c.consecutive_failures,
        "safemodeFailures": c.safemode_failures,
        "pluginState": c.plugin_state.map(|s| s.as_str()),
    })
}

/// `host_recover_boot`：查询启动恢复状态（§4.14）。
///
/// 这是**只读**命令。恢复引擎由三处驱动，均在本文件中：
/// - [`CommandState::with_adapter_config`]：启动时载入持久化标记并做崩溃检测；
/// - [`cmd_recover_report`]：应用上报启动结果（驱动信号）；
/// - [`cmd_recover_trial_enable`]：安全模式内逐个试验性启用插件。
///
/// 阶段判定落到插件侧的唯一途径是 [`reconcile_recovery_phase`]：它把引擎的判定
/// 补发成 `SafemodeEnter` / `SafemodeExit`，由注册表状态机写入
/// `disabled_by_safemode`——那才是 `<oc-plugin-manager>` 角标读取的值。
pub fn cmd_recover_boot(
    state: &SubstrateState,
) -> HostResult<serde_json::Value> {
    guard("recover_boot", || Ok(recovery_boot_payload(state)))?
}

/// `host_recover_report`：上报启动结果（§4.14 的**驱动信号**）。
///
/// `outcome` 是闭集（`success` | `failure`），表外值直接拒绝——不静默当成
/// failure 处理。
///
/// `pluginId` 只在一种情况下改变计数路径：该插件当前处于 `TrialEnable`
/// （试验性启用）状态时，本次失败按**试验失败**记（`record_trial_failure`，
/// 只累加该插件的试验次数、不累入全局计数），1 次即回落
/// `disabled-by-safemode` 且之后不可再试。否则 `pluginId` 仅作诊断提示。
///
/// 两次上报都会**清除** `bootInFlight` 标记：上报意味着本次启动已经得出明确
/// 结论，下一次启动不应重复计数。真正的崩溃（没来得及上报）由标记文件暴露。
///
/// **入口**：wire 层转调 [`cmd_recover_report_as`]（插件只能报自己的、不能认领
/// 应用级 `None`），本函数是**不过身份**的核心。
pub fn cmd_recover_report(
    state: &SubstrateState,
    outcome: &str,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    guard("recover_report", || {
        let is_success = outcome == "success";
        if !is_success && outcome != "failure" {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "host_recover_report 的 outcome 只接受 `success` 或 `failure`，收到 `{outcome}`"
                ),
            ));
        }

        let engine_action: String = {
            let mut engine = state.recovery.lock();
            if is_success {
                engine.record_boot_success();
                "bootSuccess".to_string()
            } else {
                match plugin_id {
                    Some(pid)
                        if engine.plugin_state(pid) == Some(RecoveryPluginState::TrialEnable) =>
                    {
                        // 试验性启用插件失败：按试验记（只累加该插件的试验次数），
                        // 并把这一条带真实时间戳写进诊断上下文。
                        let st = engine.record_trial_failure_at(pid, recovery::now_ms());
                        st.as_str().to_string()
                    }
                    _ => {
                        // **归因而非入账**：`pluginId` 只是「疑似故障插件」提示，
                        // 绝不因此把该插件记进试验失败预算（那会让安全模式里
                        // 崩过一次的插件永久失去试启用机会）。
                        engine.record_boot_failure_suspected_at(plugin_id, recovery::now_ms());
                        "bootFailure".to_string()
                    }
                }
            }
        };

        // 判定落地到注册表，并清除 in-flight 标记。
        let phase = reconcile_recovery_phase(state);
        // 两把锁**顺序持有、绝不重叠**：先在 recovery 锁内取出序列化快照并释放，
        // 再单独持 recovery_store 锁落盘。若在持 recovery_store 锁时去取
        // recovery 锁，会与 `recovery_boot_payload` 的 recovery → recovery_store
        // 顺序构成 ABBA 死锁；而持 recovery_store 锁调用 `recovery_boot_payload`
        // 更是直接的自死锁（`parking_lot` 不重入）。
        let engine_snapshot = state.recovery.lock().to_json();
        {
            let mut store = state.recovery_store.lock();
            store.in_flight = false;
            store.save_json(&engine_snapshot);
        }

        let payload = recovery_boot_payload(state);
        let mut result = payload.clone();
        result["outcome"] = serde_json::json!(if is_success {
            "success"
        } else {
            "failure"
        });
        result["engineAction"] = serde_json::json!(engine_action);
        result["suspectedPlugin"] = serde_json::json!(plugin_id);
        result["phaseReconcile"] = serde_json::to_value(phase).unwrap_or_default();
        Ok(result)
    })?
}

/// `host_recover_report` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// `pluginId` 决定这次上报的后果落在谁头上，而后果**是真的、且不可逆**：
///
/// - 该插件正处于 `TrialEnable` 时，本次 `failure` 走 `record_trial_failure_at`——
///   **消耗的是它的试验预算**，1 次即把它打回 `disabled-by-safemode` 且**之后不可
///   再试**。所以不判定的话，插件 A 报一条 `"failure", pluginId = "com.b"` 就能把
///   正在被人工试启的插件 B 永久关掉（跨插件 DoS：动的是别人的恢复配额）；
/// - 非试验态时它会写进**故障归因**（`suspectedPlugin` / 诊断上下文里的
///   `pluginId`）——插件能借此把故障栽赃到别的插件名下，运维按诊断去排查错对象；
/// - 两条路径都会推进**全局**启动失败计数（一次 `success` 才清零）。`pluginId` 为
///   `None` 表示「**应用级**启动结果」，那更是插件无权主张的档位：反复报
///   `failure` 就能把整个应用推进安全模式 / 修复模式。
///
/// 规则因此是"署名必须是自己"（[`require_self_plugin_scope`]）：主窗可报任意
/// `pluginId` 或 `None`（宿主才是"这次启动成没成"的权威），插件只能报**自己**。
/// 拒绝路径**零副作用**：别人的试验预算、阶段、计数器、诊断上下文一律不动
/// （有测试拿"正处于 `TrialEnable` 的 B"当靶子断言这一点）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_recover_report_as(
    caller: &Caller,
    state: &SubstrateState,
    outcome: &str,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    require_self_plugin_scope(caller, "host_recover_report", plugin_id)?;
    cmd_recover_report(state, outcome, plugin_id)
}

/// `host_recover_trial_enable`：安全模式内试验性启用一个插件。
///
/// 引擎只允许 `phase == Safemode` 且该插件未被试验失败过。发事件前先做一次
/// 阶段对账：刚装载、尚未被对账标记的插件停在 `Installed` 等非 DISABLED 态，
/// 直接发试启事件会撞非法迁移（事件被拒但引擎已改，两侧发散）；对账先把应
/// 禁用者标成 DISABLED + 安全模式标志，试启才有合法迁移起点。
///
/// 成功后发 `TrialEnable`（D28）而非 `SafemodeExit`：`SetTrial` 让注册表记入
/// 独立试启预算，且 `trial_from_safemode` 置位后该插件**自行上报错误**时会走
/// D28 回落（1 次即打回 DISABLED + 安全模式标志），与引擎侧的试验失败判定
/// 同向——两侧不是各自为政（反向同步见 `cmd_lifecycle_report`）。
/// 返回的 `phaseReconcile` 即本调用前置对账的结果（诊断用）。
///
/// **主窗专属（轮 11 第二批）**：试启是**恢复管理操作**（改别人的插件状态、
/// 消耗别人的试验预算），wire 层转调 [`cmd_recover_trial_enable_as`] 判定，
/// 本函数是**不过身份**的核心。
pub fn cmd_recover_trial_enable(
    state: &PluginRuntimeState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    guard("recover_trial_enable", || {
        let id = PluginId::new(plugin_id)?;
        // 先登记进引擎（恢复管辖范围内），再对账、再试启。
        {
            let mut engine = state.recovery.lock();
            engine.register_plugin(id.as_str());
        }
        let phase_reconcile = reconcile_recovery_phase(state);
        // 必须先取出结果再分支：`match state.recovery.lock().trial_enable(..)` 会让
        // 临时 guard 被临时值生命周期延长规则持有到整个 match 结束，于是错误分支里
        // 再次 `state.recovery.lock()` 就是自死锁（`parking_lot` 不重入，表现为
        // 空转而非挂起）。
        let trial = state.recovery.lock().trial_enable(id.as_str());
        match trial {
            Ok(()) => {}
            Err(tauron_recovery::RecoveryError::RestrictedInSafemode) => {
                let phase = state.recovery.lock().decide_boot_phase().as_str();
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("插件 `{id}` 只能在安全模式下试验性启用；当前阶段是 `{phase}`"),
                ));
            }
            Err(tauron_recovery::RecoveryError::TrialExhausted(pid)) => {
                return Err(HostError::new(
                    ErrorCode::E_PLUGIN_DISABLED,
                    format!("插件 `{pid}` 已试验失败，回落 disabled-by-safemode，不可再次试启"),
                ));
            }
            Err(e) => {
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("试验性启用失败：{e}"),
                ));
            }
        }

        // 试启成功后让注册表状态机跟着走 D28 试启迁移（§4.14：引擎负责判，
        // 状态机负责执行）。插件已在运行则无需再动。注册表侧的守卫若拒绝
        //（例如其独立试验预算已尽），以 `trialEnable:rejected` 如实回传——
        // 下一次对账会按引擎判定再补发 `SafemodeExit` 收敛（引擎是权威，
        // 注册表预算是纵深防御）。
        let engine_action = if let Some(out) = state.registry.find(&id).map(|e| e.state.state) {
            use tauron_host::lifecycle::State;
            if out == State::Enabled || out == State::Running {
                "alreadyEnabled".to_string()
            } else {
                match state.registry.report_event(&id, Event::TrialEnable) {
                    Ok(o) => format!("trialEnable:{}", o.to.as_str()),
                    Err(_) => "trialEnable:rejected".to_string(),
                }
            }
        } else {
            "notInRegistry".to_string()
        };

        let payload = recovery_boot_payload(state);
        let mut result = payload.clone();
        result["pluginId"] = serde_json::json!(id.as_str());
        result["engineAction"] = serde_json::json!(engine_action);
        result["phaseReconcile"] = serde_json::to_value(phase_reconcile).unwrap_or_default();
        Ok(result)
    })?
}

/// `host_recover_trial_enable` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// 试启是**恢复管理操作**，不是插件自己能发起的事：
///
/// - 它改的是**别人的**插件状态——把 `disabled-by-safemode` 的插件重新放行；
/// - 它**消耗别人的试验预算**（引擎侧 1 次即回落、之后不可再试），所以插件 A 反复
///   对插件 B 调它，等于把 B 唯一的一次试启用机会烧掉（跨插件配额攻击）；
/// - 它会推进阶段对账（`reconcile_recovery_phase`），把安全模式判定写回注册表——
///   一个插件不该有权驱动全局恢复状态机。
///
/// 判定与 [`require_main_window`] 的其余调用点同源、同码（[`ErrorCode::E_AUTH_DENIED`]），
/// 且在实际试启**之前**：拒绝时引擎阶段、插件状态、试验预算一律不动。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_recover_trial_enable_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_recover_trial_enable")?;
    cmd_recover_trial_enable(state, plugin_id)
}

/// `host_brand_info`：品牌信息。
///
/// （`host_market_check` 的实现在"壳扩展"一节，与 download/install 放在一起——
/// R8 把三条商城命令的线形统一成有类型的结构，三个定义不该分散在两处。）
pub fn cmd_brand_info(
    _state: &SubstrateState,
) -> HostResult<serde_json::Value> {
    // 桩：返回空对象（TS `BrandInfo` 全字段可选，返回 Null 会让
    // `info.name` 对 null 取属性而崩溃）。委托目标 tauron-brand crate
    // 尚未接线（见 overview 组件表的接线状态披露）。
    Ok(serde_json::json!({}))
}

// ──────────────────────────────────────────────────────────────────────────
// i18n（§4.20：语言状态单一来源 + 资源包 + 缺失键计数）
// ──────────────────────────────────────────────────────────────────────────

/// 校验语言代码。
///
/// [`I18nEngine::set_locale`] 与 `ResourceBundle::new` 都不做校验（内部走
/// `Locale::new`，永不失败），所以类型化校验必须放在这一层：空串或超过 35 字符
/// 的语言代码会污染回退链。
fn validated_locale(raw: &str) -> HostResult<tauron_i18n::Locale> {
    raw.parse::<tauron_i18n::Locale>().map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("非法语言代码 `{raw}`：{e}"),
        )
    })
}

fn validated_key(raw: &str) -> HostResult<()> {
    if raw.trim().is_empty() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "i18n key 不能为空（调用方可能传了 undefined/null）".to_string(),
        ));
    }
    Ok(())
}

/// i18n 引擎当前状态的线形态（多个命令共用）。
fn i18n_state_payload(engine: &I18nEngine) -> serde_json::Value {
    let locales = engine.registered_locales();
    let mut bundle_keys = serde_json::Map::new();
    for locale in &locales {
        bundle_keys.insert(
            locale.clone(),
            serde_json::json!(engine.get_bundle(locale).map_or(0, ResourceBundle::len)),
        );
    }
    serde_json::json!({
        "locale": engine.locale().as_str(),
        "rtl": engine.is_rtl(),
        "fallbackChain": engine.fallback_chain(),
        "registeredLocales": locales,
        "bundleKeys": bundle_keys,
        "missingTotal": engine.missing_total(),
    })
}

/// `host_i18n_t`：翻译一个 key。
///
/// 回退链由引擎负责：当前语言 → 短形式（`zh-CN` → `zh`）→ 默认语言
/// （`en-US`）。**全部缺失时返回 key 本身**并计入缺失键计数——这是刻意设计：
/// 让用户看到 `oc.settings.title` 比看到空按钮更能暴露缺失文案，而空串会让
/// 问题彻底隐形。缺失可观测性见 `host_i18n_stats` 的 `missingTotal`。
pub fn cmd_i18n_t(
    state: &SubstrateState,
    key: &str,
) -> HostResult<String> {
    guard("i18n_t", || {
        validated_key(key)?;
        Ok(state.i18n.lock().t(key))
    })?
}

/// `host_i18n_t_params`：带 `{{param}}` 占位替换的翻译。
pub fn cmd_i18n_t_params(
    state: &SubstrateState,
    key: &str,
    params: serde_json::Map<String, serde_json::Value>,
) -> HostResult<String> {
    guard("i18n_t_params", || {
        validated_key(key)?;
        let mut owned = Vec::with_capacity(params.len());
        for (name, value) in &params {
            // 参数值必须是字符串：模板替换不接受 JSON 对象/数组。
            let Some(text) = value.as_str() else {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("i18n 参数 `{name}` 的值必须是字符串，收到 `{value}`"),
                ));
            };
            owned.push((name.clone(), text.to_string()));
        }
        let pairs: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        Ok(state.i18n.lock().t_params(key, &pairs))
    })?
}

/// `host_i18n_set_locale` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// 语言**不是"某个插件的设置"**，而是整个应用界面的单一状态源：`state.i18n` 的 locale
/// 同时决定宿主 UI 与**所有**插件界面的呈现（`host_i18n_t` 的回退链第一环就是它）。
/// 因此插件调它 = **跨插件的全局状态篡改**：
///
/// - 把语言切到别的语种，别的插件（和宿主）的界面**当场变成另一种语言**——用户
///   明明在中文环境里看到的却是日文菜单，而界面文案正是安全提示（"即将删除"、
///   "允许访问文件"）的载体，篡改它可以骗过用户；
/// - 它不是拒绝服务那么轻：切换是静默成功的（返回 200 语义的正常状态体），
///   用户只会觉得"这个应用坏了"，而根因在另一个插件的一次调用里。
///
/// 主窗是唯一合法主体（用户自己的偏好设置）。拒绝码复用既有的
/// [`ErrorCode::E_AUTH_DENIED`]，判定在**写入引擎之前**：拒绝时 locale 与
/// `registeredLocales` 一律不动（有测试断言引擎状态逐字不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_i18n_set_locale_as(
    caller: &Caller,
    state: &SubstrateState,
    locale: &str,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_i18n_set_locale")?;
    cmd_i18n_set_locale(state, locale)
}

/// `host_i18n_set_locale`：切换语言（§4.20：语言状态的单一来源）。
///
/// **入口**：wire 层转调 [`cmd_i18n_set_locale_as`]（仅主窗：切的是全局语言），
/// 本函数是**不过身份**的核心。
pub fn cmd_i18n_set_locale(
    state: &SubstrateState,
    locale: &str,
) -> HostResult<serde_json::Value> {
    guard("i18n_set_locale", || {
        let validated = validated_locale(locale)?;
        let mut engine = state.i18n.lock();
        engine
            .set_locale(validated.as_str())
            .map_err(|e| HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("切换语言失败：{e}")))?;
        Ok(i18n_state_payload(&engine))
    })?
}

/// `host_i18n_load`：装载一个语言包。
///
/// **合并语义**：同一语言的包已存在时按 key 合并，而不是整体替换——否则先装
/// 应用文案、再装插件文案会把应用自己的文案覆盖掉。
///
/// 传入 `pluginId` 时，每个 key 自动加 `plugin:<id>.oc.` 前缀（§4.20 命名空间）；
/// 已经带该前缀的 key 原样保留（幂等）。插件卸载时由
/// [`cmd_i18n_cleanup_plugin`] / `cmd_registry_admin` 的 Uninstall/Purge 回收。
///
/// **入口**：wire 层转调 [`cmd_i18n_load_as`]（`pluginId` = 命名空间归属，插件
/// 只能装自己的；`None` = 宿主文案，插件不得主张），本函数是**不过身份**的核心。
pub fn cmd_i18n_load(
    state: &SubstrateState,
    locale: &str,
    entries: serde_json::Map<String, serde_json::Value>,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    guard("i18n_load", || {
        let validated = validated_locale(locale)?;
        let plugin_prefix = match plugin_id {
            Some(pid) => Some(PluginId::new(pid)?.as_str().to_string()),
            None => None,
        };

        // 读-改-写必须在**同一把锁**内完成：拆成「锁内读 bundle → 锁外拼 →
        // 再锁内写回」会让并发装载丢更新——两个调用各读到同一旧快照，后写者
        // 整体覆盖先写者刚装入的文案。
        let mut engine = state.i18n.lock();
        let mut bundle = engine
            .get_bundle(validated.as_str())
            .cloned()
            .unwrap_or_else(|| ResourceBundle::new(validated.as_str()));

        let mut new_keys = 0usize;
        for (key, value) in &entries {
            // 文案值必须是字符串：非字符串的条目说明 bundle 文件格式错了。
            let Some(text) = value.as_str() else {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("i18n 条目 `{key}` 的值必须是字符串，收到 `{value}`"),
                ));
            };
            let full = match &plugin_prefix {
                Some(prefix) if !key.starts_with(&format!("plugin:{prefix}.")) => {
                    I18nEngine::plugin_key(prefix, key)
                }
                _ => key.clone(),
            };
            if bundle.get(&full).is_none() {
                new_keys += 1;
            }
            bundle.insert(&full, text);
        }
        engine.add_resource_bundle(bundle);

        let mut payload = i18n_state_payload(&engine);
        payload["loadedKeys"] = serde_json::json!(entries.len());
        payload["newKeys"] = serde_json::json!(new_keys);
        payload["pluginId"] = serde_json::json!(plugin_prefix);
        Ok(payload)
    })?
}

/// `host_i18n_load` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// `pluginId` 在这里就是**命名空间归属**：传 `Some(id)` 时每个 key 会被加上
/// `plugin:<id>.oc.` 前缀，于是"谁装的文案"完全由这个入参决定。不判定的话：
///
/// - 插件 A 能以 `pluginId = "com.b"` 装载文案，**写进 B 的命名空间**——B 界面上
///   出现的每一句文案（按钮标签、确认框正文、危险操作提示）都可被 A 改写，
///   这是**命名空间投毒**：不需要执行任何代码就能伪造 B 呈现给用户的内容；
/// - 传 `None` 则装进**宿主级命名空间**（无前缀的应用文案），插件能覆盖应用自己的
///   文案——同样是不执行代码就能改整个应用的说法。
///
/// 规则是"命名空间必须是自己"（[`require_self_plugin_scope`]）：主窗可装任意
/// `pluginId` 或 `None`（宿主本来就是应用文案、以及代插件装载的一方），插件只能装
/// 自己的。拒绝路径**零副作用**：`entries` 里的任何 key 都不会进入引擎
/// （有测试断言目标命名空间在引擎里**不存在**）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_i18n_load_as(
    caller: &Caller,
    state: &SubstrateState,
    locale: &str,
    entries: serde_json::Map<String, serde_json::Value>,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    require_self_plugin_scope(caller, "host_i18n_load", plugin_id)?;
    cmd_i18n_load(state, locale, entries, plugin_id)
}

/// `host_i18n_stats`：i18n 状态与缺失键可观测性。
pub fn cmd_i18n_stats(
    state: &SubstrateState,
) -> HostResult<serde_json::Value> {
    guard("i18n_stats", || Ok(i18n_state_payload(&state.i18n.lock())))?
}

/// `host_i18n_cleanup_plugin`：清除一个插件的全部文案。
///
/// 返回清除的 key 数。插件卸载（`host_registry_admin` 的 Uninstall/Purge）
/// 会自动调用它；这里独立暴露是给「插件被禁用但仍在注册表里」的情形用。
///
/// **入口**：wire 层转调 [`cmd_i18n_cleanup_plugin_as`]（插件只能清自己的文案，
/// 清别人就是跨插件销毁），本函数是**不过身份**的核心。
pub fn cmd_i18n_cleanup_plugin(
    state: &SubstrateState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    guard("i18n_cleanup_plugin", || {
        let id = PluginId::new(plugin_id)?;
        let mut engine = state.i18n.lock();
        let removed = engine.cleanup_plugin(id.as_str());
        drop(engine);

        let mut payload = i18n_state_payload(&state.i18n.lock());
        payload["pluginId"] = serde_json::json!(id.as_str());
        payload["removed"] = serde_json::json!(removed);
        Ok(payload)
    })?
}

/// `host_i18n_cleanup_plugin` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// 这条命令**销毁**目标插件的全部文案（`plugin:<id>.oc.` 前缀的 key，跨所有语言包）。
/// 不判定的话，插件 A 调一次 `pluginId = "com.b"` 就能**清空 B 的全部界面文案**：
/// B 之后每个 `host_i18n_t` 都会落空——按设计（"缺失时返回 key 本身"）B 的界面会
/// 变成一屏 `plugin:com.b.oc.xxx` 原始 key。这是纯破坏性的跨插件操作，且不需要
/// 任何权限授予即可调用，所以判定必须落在参数上（[`require_self_plugin_scope`]）。
///
/// 主窗可清任意插件（卸载/禁用回收的正当路径：`cmd_registry_admin` 的
/// Uninstall/Purge 内部就会调核心 `cmd_i18n_cleanup_plugin`），插件只能清自己的。
/// 拒绝路径**零副作用**：别人的 bundle 一个 key 都不会掉（有测试断言它仍在）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_i18n_cleanup_plugin_as(
    caller: &Caller,
    state: &SubstrateState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    require_self_plugin_scope(caller, "host_i18n_cleanup_plugin", Some(plugin_id))?;
    cmd_i18n_cleanup_plugin(state, plugin_id)
}

// ──────────────────────────────────────────────────────────────────────────
// 窗口管理命令（P1-1：Rust 窗口命令族；R8 §1：平台部分改走 WindowSink）
//
// **R8 之前的样子**：这里只有 `guard(...) + Ok(())` 桩，真实窗口操作写在
// `tauri.rs` 的 `#[tauri::command]` 包装器里（`window.minimize()` 等）。
// 于是"命令做了什么"分成两处，且平台那一半在单测里完全不可达。
//
// **现在**：核心命令一律转调 [`SubstrateState::window_sink`]。`label` 由 wire 层
// 从真实 `WebviewWindow` 取（沿用"身份/目标只从宿主侧来"的既有口径），核心不解析、
// 不信任任何前端入参。
// ──────────────────────────────────────────────────────────────────────────

/// `host_window_minimize`：最小化**调用方自己**的窗口。
pub fn cmd_window_minimize(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_minimize", || state.window_sink.minimize(label))?
}

/// `host_window_maximize`：最大化调用方的窗口。
pub fn cmd_window_maximize(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_maximize", || state.window_sink.maximize(label))?
}

/// `host_window_restore`：还原调用方的窗口（从最大化/最小化）。
pub fn cmd_window_restore(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_restore", || state.window_sink.restore(label))?
}

/// `host_window_close`：关闭调用方的窗口。
///
/// 窗口销毁后的回收**不在这里**：宿主在 `on_window_event(Destroyed)` 里调
/// [`crate::tauri::cleanup_closed_window`]（订阅/分组/pending 调用一起回收）。
/// 在这里顺手清会漏掉"用户点标题栏关闭"这条路径——那是同一件事，不该有两套清理。
pub fn cmd_window_close(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_close", || state.window_sink.close(label))?
}

/// `host_window_quit`：退出应用。
pub fn cmd_window_quit(state: &SubstrateState) -> HostResult<()> {
    guard("window_quit", || state.window_sink.quit())?
}

/// `host_window_relaunch`：**先对账恢复阶段、再重启**（主窗专属，R8 §3）。
///
/// # 顺序为什么不能反（代码里的显式顺序就是这条不变量）
///
/// 重启前必须先 [`reconcile_recovery_phase`]，因为对账是**把引擎的判定写回注册表**
/// 的唯一途径（`disabled_by_safemode` 的权威来源）。反过来的话：本次进程带着
/// 「注册表标志尚未与引擎判定对齐」的状态退出，而恢复标记与阶段计数器是**跨进程
/// 持久化**的——下一次启动读回的仍是旧判定，于是"该禁用的插件没被禁用 → 再次
/// 启动失败 → 再重启"，形成重启循环（正是安全模式要打断的那类循环）。
///
/// 顺序在代码里是**两行、按序执行**（先 `reconcile`，再 `relaunch`），对账结果也随
/// 返回值一起给出去，因此这个顺序在运行时可观测、可断言
/// （见 `sink_tests::window_relaunch_reconciles_before_requesting_restart`）。
///
/// # 谁能调
///
/// **仅主窗**：重启是应用级动作，插件 webview 触发它等于把"重启风暴"的开关交给
/// 任意插件（而且它同时改写全局恢复阶段判定）。判定在代码层执行（不只是 ACL）。
pub fn cmd_window_relaunch_as(caller: &Caller, state: &SubstrateState) -> HostResult<WindowRelaunchOutcome> {
    require_main_window(caller, "host_window_relaunch")?;
    guard("window_relaunch", || {
        // ① **先**对账：把恢复引擎的阶段判定补发到插件侧（顺序不可交换，见文档）。
        let reconcile = reconcile_recovery_phase(state);
        // ② **后**重启。降级实现返回 `false`（非 Tauri 宿主没有重启原语）——
        //    此时如实上报，不假装已经重启。
        let requested = state.window_sink.relaunch()?;
        Ok(WindowRelaunchOutcome {
            reconcile,
            relaunch_requested: requested,
            reason: (!requested).then(|| {
                "宿主无重启原语（非 Tauri 宿主 / 未注入 TauriWindowSink）：\
                 对账已完成，但**没有**请求任何重启"
                    .to_string()
            }),
        })
    })?
}

/// `host_window_create`：为**已注册**插件创建主面板窗口（主窗专属，R8 §3）。
///
/// # 语义与拒绝面
///
/// - label **由核心铸造**，恒为 `plugin-<id>`（身份模型的载体，不由入参决定）；
/// - `<id>` 必须**已存在于注册表**：不存在的 id 一律拒绝（错误码复用
///   [`ErrorCode::E_UNKNOWN_PLUGIN`]），**不创建半态窗口**——一个 label 指向
///   不存在插件的 webview 会被身份解析当作合法单元（label 就是身份），却没有任何
///   注册表条目与它对应，之后所有 self 档命令都会以"未注册"失败；
/// - 插件必须声明 `entry.ui`：没有 UI 入口就没有可加载的页面。缺它 → 拒绝
///   （[`ErrorCode::E_INVALID_MANIFEST`]）。**不用一个编造的默认 URL 顶上**——
///   那会把"配置缺失"变成"能打开但内容是错的"；
/// - URL **只来自 manifest**（线形里没有 url 字段）：让调用方指定 URL 等于让主窗
///   把"带插件身份的 webview"指向任意地址，而 label 决定身份 → 提权跳板。
///
/// # 清理
///
/// 创建后的回收**复用既有钩子**：label 是 `plugin-<id>`，`cleanup_closed_window`
/// 正是用这个前缀推出插件 id 来回收订阅 / 分组 / pending 调用的。宿主只需保证
/// `on_window_event(Destroyed)` 那一行已接线（示例应用 `main.rs` 里有）。
///
/// # 谁能调
///
/// **仅主窗**：为任意已注册插件开窗（含被安全模式禁用的插件）是宿主管理面动作。
pub fn cmd_window_create_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    req: &WindowCreateRequest,
) -> HostResult<WindowCreateOutcome> {
    require_main_window(caller, "host_window_create")?;
    guard("window_create", || {
        let id = PluginId::new(&req.plugin_id)?;
        // 注册表是"这个插件存在"的唯一权威来源。`require` 失败 = `E_UNKNOWN_PLUGIN`，
        // 且**在任何窗口副作用之前**失败。
        let entry = state.registry.require(&id)?;
        let url = entry.manifest.entry.ui.clone().ok_or_else(|| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "插件 `{id}` 的 manifest 未声明 `entry.ui`：没有 UI 入口就没有可加载的页面，\
                     拒绝创建窗口（不编造默认 URL）"
                ),
            )
        })?;

        let spec = WindowCreateSpec {
            // 身份 label 的**唯一**铸造点（与 `PluginIdentity::new` 同一约定）。
            label: format!("plugin-{id}"),
            plugin_id: id.as_str().to_string(),
            url,
            title: req
                .title
                .clone()
                .unwrap_or_else(|| entry.manifest.name.clone()),
            width: req.width.unwrap_or(DEFAULT_WINDOW_WIDTH),
            height: req.height.unwrap_or(DEFAULT_WINDOW_HEIGHT),
        };

        let created = state.window_sink.create(&spec)?;
        Ok(WindowCreateOutcome {
            label: spec.label,
            plugin_id: spec.plugin_id,
            created,
            reason: (!created).then(|| {
                "宿主未实现窗口创建（非 Tauri 宿主 / 未注入 TauriWindowSink）：\
                 **没有**创建任何窗口"
                    .to_string()
            }),
        })
    })?
}

// ──────────────────────────────────────────────────────────────────────────
// 壳扩展状态与命令（P2：窗口持久化 / 剪贴板 / 深链接 / 对话框 / 更新）
// ──────────────────────────────────────────────────────────────────────────

/// 壳扩展进程内状态。
///
/// 说明：窗口几何与剪贴板是**真实进程内行为**（可被读取验证）；
/// 对话框在无 Tauri 对话框插件的环境下返回"取消"（None/false），
/// 更新命令为模拟结果（诚实标注 `simulated: true`）。
#[derive(Debug, Clone, Default)]
pub struct ShellExtState {
    /// 窗口几何 (x, y, width, height)：由 set_position/set_size 更新。
    pub window_rect: (i32, i32, u32, u32),
    /// 进程内剪贴板文本。
    pub clipboard: String,
    /// 已注册的深链接协议（None = 未注册）。
    pub deep_link_protocol: Option<String>,
    /// 更新状态机：None → downloaded → installed。
    pub update_state: Option<String>,
    /// **origin 允许清单（R4-D2）**：由 [`AdapterConfig::origin_allowlist`] 装配，
    /// 由 `tauri::origin_gate` 在命令分发入口读取。空 = 不启用。
    pub origin_allowlist: Vec<String>,
}

/// `host_window_set_position`：移动调用方窗口到 `(x, y)`（逻辑像素/DIP）。
///
/// **两件事，都要做**（R8）：
/// 1. 写 `shell_ext.window_rect` —— 进程内的几何账本（窗口状态恢复读它）；
/// 2. 转调 [`SubstrateState::window_sink`] 执行真实移动。
///
/// 只做 1 = 几何只在账本里变了（R8 之前的桩就是这样）；只做 2 = 账本失真。
/// 顺序是**先记账后落平台**：平台调用失败时账本已记下用户意图，而失败的 `Err`
/// 会如实返回，不会假装移动成功。
pub fn cmd_window_set_position(
    state: &SubstrateState,
    label: &str,
    x: i32,
    y: i32,
) -> HostResult<()> {
    guard("window_set_position", || {
        {
            let mut ext = state.shell_ext.lock();
            ext.window_rect.0 = x;
            ext.window_rect.1 = y;
        }
        state.window_sink.set_position(label, x, y)
    })?
}

/// `host_window_set_size`：缩放调用方窗口（逻辑像素/DIP）。记账 + 落平台，同
/// [`cmd_window_set_position`]。
pub fn cmd_window_set_size(
    state: &SubstrateState,
    label: &str,
    width: u32,
    height: u32,
) -> HostResult<()> {
    guard("window_set_size", || {
        {
            let mut ext = state.shell_ext.lock();
            ext.window_rect.2 = width;
            ext.window_rect.3 = height;
        }
        state.window_sink.set_size(label, width, height)
    })?
}

/// `host_clipboard_write`：写入进程内剪贴板。
pub fn cmd_clipboard_write(
    state: &SubstrateState,
    text: String,
) -> HostResult<()> {
    guard("clipboard_write", || {
        state.shell_ext.lock().clipboard = text;
        Ok(())
    })?
}

/// `host_clipboard_read`：读取进程内剪贴板。
pub fn cmd_clipboard_read(
    state: &SubstrateState,
) -> HostResult<String> {
    guard("clipboard_read", || {
        Ok(state.shell_ext.lock().clipboard.clone())
    })?
}

/// `host_deep_link_register` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// # R8 曾把它当 self-service——那是错的，这里是修正
///
/// 它写的不是"某个插件自己的东西"，而是**应用级**的两处共享状态：
///
/// 1. `shell_ext.deep_link_protocol`（整个应用当前认哪个协议）——插件换值就是
///    **把整个应用的深链接入口改到自己名下**；
/// 2. `deep-link` 公共 topic 的声明（EventBus 的授权前提）——换值会连带
///    [`cmd_deep_link_register`] 的"先注销旧值"路径，把**上一个协议**从 OS 关联里
///    摘掉（`unregister(old)`）。
///
/// 组合起来的真实攻击面：插件 A 调一次 `register("evil")`，应用的深链接入口就被
/// 改成 `evil://`，用户点原本的 `tauron://` 链接**不再拉起本应用**（旧关联已被注销），
/// 而 A 可以用自己的协议接管后续 URL 投递（含带 token 的跳转链接）。这是跨插件/
/// 应用级的越权面，不是"插件配置自己的协议"。
///
/// 主窗是唯一合法主体。拒绝码复用既有 [`ErrorCode::E_AUTH_DENIED`]，判定在
/// **任何副作用之前**：拒绝时 `shell_ext.deep_link_protocol` 与 topic 声明、
/// 以及 sink 的 `unregister` 一律不动（有测试断言协议值逐字不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_deep_link_register_as(
    caller: &Caller,
    state: &SubstrateState,
    protocol: String,
) -> HostResult<()> {
    require_main_window(caller, "host_deep_link_register")?;
    cmd_deep_link_register(state, protocol)
}

/// `host_deep_link_register`：注册深链接协议。
///
/// # 这条命令做两件事，其中**只有一件是真的平台事**
///
/// 1. **应用层（真实）**：记录协议到 `shell_ext`，并声明 `deep-link` 公共 topic——
///    这是前端 `subscribe` 与 [`deep_link_delivered`] 投递的授权前提（EventBus 要求
///    先声明后订阅/发布）。这一步任何时候都会发生。
/// 2. **平台层（走 sink）**：转调 [`SubstrateState::deep_link_sink`]。
///    ⚠️ **本仓的 sink 实现全部是降级路径**（`tauri-plugin-deep-link` 不在依赖闭包
///    内）：`register` 只记账并（Tauri 实现下）向前端发一个"未做 OS 注册"的信号，
///    `native_supported()` 恒 `false`。也就是说：**OS 不会把 `tauron://` 交给本应用**，
///    本命令只是让"URL 到了宿主之后"的那一段可用（谁把 URL 送进来是宿主自己的事，
///    例如命令行/单实例转发）。
///
/// # 为什么换协议时要先注销（`unregister` 的真实消费者）
///
/// 协议名是 OS 关联的键：`tauron` → `other` 的换名如果不注销旧值，系统里会留下
/// 旧协议的关联（用户点旧链接仍会拉起本应用，而应用层已按新协议解析）。
/// 因此核心在写新值之前先对旧值调 `unregister`——这不是预留接口。
///
/// **入口**：wire 层转调 [`cmd_deep_link_register_as`]（仅主窗：注册的是**应用级**
/// 协议，插件改它 = 把整个应用的深链接入口改到自己名下），本函数是**不过身份**的核心。
pub fn cmd_deep_link_register(
    state: &SubstrateState,
    protocol: String,
) -> HostResult<()> {
    guard("deep_link_register", || {
        // 空协议不是一个"稍微不对"的协议名，而是一次无意义的 OS 注册请求。
        if protocol.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "深链接协议不能为空（调用方可能传了 undefined/null）".to_string(),
            ));
        }
        let previous = state.shell_ext.lock().deep_link_protocol.clone();
        if let Some(old) = previous.filter(|old| old != &protocol) {
            state.deep_link_sink.unregister(&old)?;
        }
        {
            let mut ext = state.shell_ext.lock();
            ext.deep_link_protocol = Some(protocol.clone());
        }
        // 声明 `deep-link` 公共 topic：这是前端 subscribe 与
        // deep_link_delivered 投递的授权前提（EventBus 要求先声明后订阅/发布）。
        {
            let bus = state.bus.lock();
            bus.declare_topics(
                DEEP_LINK_PUBLISHER,
                &[EventDecl {
                    topic: DEEP_LINK_TOPIC.to_string(),
                    public: true,
                }],
            )?;
        }
        // 平台侧注册放在最后：应用层状态与 topic 都到位之后才谈"让 OS 认这个协议"。
        state.deep_link_sink.register(&protocol)
    })?
}

/// 深链接事件的 topic 与伪发布者（框架内部源，非插件）。
pub const DEEP_LINK_TOPIC: &str = "deep-link";
pub const DEEP_LINK_PUBLISHER: &str = "core.deep-link";

/// 深链接到达入口（OS 协议回调侧 glue 调用点）。
///
/// 生产接线：`tauri-plugin-deep-link` 的回调里调用本函数，把收到的 URL
/// 发布到 EventBus 的 `deep-link` 主题；前端 `DeepLinkClient.subscribe()`
/// 即在该 URL 上收到（含 parseUrl 结构化解析）。
///
/// 这不是一条新 IPC 命令（JS 不可调用，防伪造）——只供 Rust 侧回调使用。
pub fn deep_link_delivered(
    state: &SubstrateState,
    url: &str,
) -> HostResult<()> {
    guard("deep_link_delivered", || {
        let payload = serde_json::json!({
            "url": url,
            "protocol": state.shell_ext.lock().deep_link_protocol.clone(),
        });
        let bus = state.bus.lock();
        // Event 通道：与前端 backend.listen('deep-link') 的 drain 通道一致。
        let res = bus.publish(DEEP_LINK_PUBLISHER, DEEP_LINK_TOPIC, payload, ChannelKind::Event);
        if res.dropped {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                "deep-link topic 未声明：须先调用 host_deep_link_register",
            ));
        }
        Ok(())
    })?
}

/// `host_market_check` 的返回值（camelCase 线形，**有类型**）。
///
/// R8 之前这里是裸 `serde_json::json!({ "available": false })`：`simulated` 这件事
/// 只写在注释里，前端**没有任何字段**可以据此判断"这是模拟结果"。现在它是线字段。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCheckResult {
    /// 是否有可用更新。当前恒 `false`（没有更新源）。
    pub available: bool,
    /// **是否模拟结果**：当前恒 `true`——本命令不做任何真实可用性探测
    /// （无 `tauri-plugin-updater`，也不请求任何 endpoint）。
    pub simulated: bool,
    /// 可用版本号；无则 `null`。
    pub version: Option<String>,
    /// 为什么不可用 / 为什么是模拟结果；成功且非模拟时为 `null`。
    pub reason: Option<String>,
}

/// `host_market_download` / `host_market_install` 的返回值（camelCase 线形）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketUpdateResult {
    /// 本次命令是否执行成功（**不是**"更新是否真的落地"——那看 `simulated`）。
    pub ok: bool,
    /// **是否模拟结果**：当前恒 `true`——没有下载任何字节、没有验签、没有替换文件。
    pub simulated: bool,
    /// 目标版本号；缺省入参时为 `null`。
    pub version: Option<String>,
    /// 为什么是模拟结果；真实连线后为 `null`。
    pub reason: Option<String>,
}

/// `host_market_check`：检查更新。
///
/// ⚠️ **这是桩，而且现在会如实说出来**：返回值里 `simulated: true` +
/// `reason` 写明"未接入更新源"。委托目标（`tauron-distribute` / `tauron-market`）
/// 尚未接线（见组件表的接线状态披露）；接入时只需替换本函数体，线形不变
/// （`simulated` 改为 `false`、`reason` 置 `null`）。
///
/// **主窗专属（轮 11 第二批）**：商城命令操作的是**宿主级产物**（更新源 / 安装
/// 包），插件不得触发；wire 层转调 [`cmd_market_check_as`] 判定，本函数是
/// **不过身份**的核心。
pub fn cmd_market_check(_state: &SubstrateState) -> HostResult<MarketCheckResult> {
    guard("market_check", || {
        Ok(MarketCheckResult {
            available: false,
            simulated: true,
            version: None,
            reason: Some(
                "未接入更新源：`tauri-plugin-updater` 不在依赖闭包内，本命令不做任何真实\
                 可用性探测（没有请求任何 endpoint）；这是本地桩结果，不是网络结论"
                    .to_string(),
            ),
        })
    })?
}

/// `host_market_download`：**模拟**下载更新。
///
/// 真实行为只有一件：把进程内 `update_state` 推到 `downloaded:<version>`。
/// `ok: true` 的语义是"命令成功执行"，**不是**"更新已下载"——判断后者必须看
/// `simulated`。返回里有 `reason` 写明这一点，所以前端不必靠猜。
///
/// **主窗专属（轮 11 第二批）**：wire 层转调 [`cmd_market_download_as`]；本函数是
/// **不过身份**的核心。
pub fn cmd_market_download(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    guard("market_download", || {
        let mut ext = state.shell_ext.lock();
        ext.update_state = Some(format!("downloaded:{}", version.unwrap_or("unknown")));
        Ok(MarketUpdateResult {
            ok: true,
            simulated: true,
            version: version.map(str::to_string),
            reason: Some(
                "模拟下载：**没有**下载任何字节、没有写入任何文件、没有校验签名；\
                 只是把进程内 updateState 推进到 downloaded"
                    .to_string(),
            ),
        })
    })?
}

/// `host_market_install`：**模拟**安装更新（同 [`cmd_market_download`] 的诚实口径）。
///
/// **主窗专属（轮 11 第二批）**：wire 层转调 [`cmd_market_install_as`]；本函数是
/// **不过身份**的核心。
pub fn cmd_market_install(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    guard("market_install", || {
        let mut ext = state.shell_ext.lock();
        ext.update_state = Some(format!("installed:{}", version.unwrap_or("unknown")));
        Ok(MarketUpdateResult {
            ok: true,
            simulated: true,
            version: version.map(str::to_string),
            reason: Some(
                "模拟安装：**没有**替换任何二进制/文件、没有触发任何重启流程；\
                 只是把进程内 updateState 推进到 installed"
                    .to_string(),
            ),
        })
    })?
}

/// `host_market_check` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这三条是主窗专属（危害原文）
///
/// 商城命令操作的是**宿主级产物**，不是某个插件的东西：
///
/// - `check` 会拿调用方下发的**更新源 endpoint / 公钥**去探更新（接入后）：插件能
///   借此把宿主指向自己的更新源（供应链投毒的第一步），或当成内网探测跳板；
/// - `download` / `install` 会推进宿主的 `updateState`（接入后就是**替换应用自身的
///   二进制**）——没有任何"某个插件"能成为这类操作的主体，让插件 webview 调它等于
///   把宿主自身的更新通道交给插件；
/// - 三条都不收身份参数，所以判定只能落在**整条命令**上（与
///   `host_runtime_spawn` 同一条 [`require_main_window`]，同码 `E_AUTH_DENIED`）。
///
/// 现状是桩（`simulated: true`、不请求网络、不写文件），但**不因此放松判定**：
/// 桩的线形与真实实现同形，接线时若判定缺席，越权面会在没人注意时从"无害桩"
/// 变成"供应链入口"。拒绝路径**零副作用**：`updateState` 不被写（有测试断言）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_market_check_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<MarketCheckResult> {
    require_main_window(caller, "host_market_check")?;
    cmd_market_check(state)
}

/// `host_market_download` 的**带身份判定**版本（见 [`cmd_market_check_as`] 的危害说明）。
pub fn cmd_market_download_as(
    caller: &Caller,
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    require_main_window(caller, "host_market_download")?;
    cmd_market_download(state, version)
}

/// `host_market_install` 的**带身份判定**版本（见 [`cmd_market_check_as`] 的危害说明）。
pub fn cmd_market_install_as(
    caller: &Caller,
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    require_main_window(caller, "host_market_install")?;
    cmd_market_install(state, version)
}

/// `host_dialog_open`：打开文件/目录对话框。
///
/// 转调 [`SubstrateState::dialog_sink`]。**缺省实现与 Tauri 实现当前都是降级路径**
/// （无 `tauri-plugin-dialog`）：返回 `Ok(None)` = 取消，见 [`NoopDialogSink`]
/// 与 `tauri.rs` 的 `TauriDialogSink` 文档注释——那里写清了"没弹原生对话框"。
pub fn cmd_dialog_open(
    state: &SubstrateState,
    multiple: bool,
    directory: bool,
) -> HostResult<Option<String>> {
    guard("dialog_open", || state.dialog_sink.open_file(multiple, directory))?
}

/// `host_dialog_save`：保存文件对话框（`Ok(None)` = 取消）。
pub fn cmd_dialog_save(
    state: &SubstrateState,
    default_name: Option<&str>,
) -> HostResult<Option<String>> {
    guard("dialog_save", || state.dialog_sink.save_file(default_name))?
}

/// `host_dialog_message`：消息对话框。
///
/// `kind` 是**闭集**（`info` | `error` | `warning`，缺省 `info`）：R8 之前这个字段
/// 被整个忽略（写错了也没人知道），现在表外值直接拒绝
/// （[`ErrorCode::E_INVALID_MANIFEST`]，参数错用参数错码），不静默当成 `info`。
pub fn cmd_dialog_message(
    state: &SubstrateState,
    title: &str,
    message: &str,
    kind: Option<&str>,
) -> HostResult<()> {
    guard("dialog_message", || {
        let kind = validated_dialog_kind(kind)?;
        state.dialog_sink.message(kind, title, message)
    })?
}

/// `host_dialog_confirm`：确认对话框。
///
/// 转调 [`SubstrateState::dialog_sink`]。降级实现恒 `Ok(false)`：它的语义是
/// **"没弹过对话框"**，既不是"用户点了否"也不是"用户点了是"——破坏性操作不得用
/// 它当用户确认（前端 `DialogClient.confirm` 的文档里有同样的话）。
pub fn cmd_dialog_confirm(
    state: &SubstrateState,
    title: &str,
    message: &str,
) -> HostResult<bool> {
    guard("dialog_confirm", || state.dialog_sink.confirm(title, message))?
}

/// 对话框 `kind` 的闭集校验（缺省 `info`）。
fn validated_dialog_kind(kind: Option<&str>) -> HostResult<&'static str> {
    match kind {
        None | Some("info") => Ok("info"),
        Some("error") => Ok("error"),
        Some("warning") => Ok("warning"),
        Some(other) => Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("对话框 kind `{other}` 非法：只接受 info / error / warning（缺省 info）"),
        )),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tauron_host::{
        manifest::{EventDecl, EventsDecl, PermissionIndex, PluginManifest, PluginType},
        ErrorCode,
    };
    // P0-2 测试用具：fake 启动面只实现 trait，不碰真进程。
    use tauron_proc::{ProcResult, SpawnedProc};

    /// R4-D2 装配链路：`AdapterConfig.origin_allowlist` 必须流到 origin 门读取的
    /// `shell_ext` 上；默认装配 = 不启用（兼容既有宿主）。
    #[test]
    fn origin_allowlist_flows_from_adapter_config_to_shell_ext() {
        let configured = CommandState::with_adapter_config(AdapterConfig {
            origin_allowlist: vec!["http://127.0.0.1:63896".to_string()],
            ..AdapterConfig::default()
        });
        assert_eq!(
            configured.shell_ext.lock().origin_allowlist,
            vec!["http://127.0.0.1:63896".to_string()]
        );
        assert!(
            CommandState::new()
                .shell_ext
                .lock()
                .origin_allowlist
                .is_empty(),
            "默认装配必须不启用 origin 门（缺省放行，兼容既有宿主）"
        );
    }

    fn empty_index() -> PermissionIndex {
        PermissionIndex {
            version: 1,
            generated_at: None,
            entries: vec![],
        }
    }

    fn test_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: PluginId::new(id).unwrap(),
            name: "Test".to_string(),
            version: semver::Version::parse("1.0.0").unwrap(),
            plugin_type: PluginType::Js,
            entry: tauron_host::manifest::EntrySpec {
                js: Some("index.js".to_string()),
                sidecar: None,
                wasm: None,
                ui: None,
            },
            permissions: vec![],
            scopes: Default::default(),
            platforms: vec![],
            framework: semver::VersionReq::parse(">=1.0.0, <3.0.0").unwrap(),
            abi: None,
            contributes: Default::default(),
            settings_schema: None,
            events: EventsDecl { publish: vec![], subscribe: vec![] },
            host_functions: vec![],
            min_allowed_version: None,
            signature: None,
            publisher: None,
        }
    }

    #[test]
    fn command_state_new() {
        let state = CommandState::new();
        let result = cmd_registry_list_all(&state);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn command_state_with_config() {
        let config = RegistryConfig {
            max_plugins: 4,
            max_active_identities: 4,
            max_pending_calls: 100,
            pending_ttl: std::time::Duration::from_secs(10),
            plugin_filter: None,
        };
        let state = CommandState::with_config(config);
        assert!(cmd_registry_list_all(&state).is_ok());
    }

    #[test]
    fn cmd_registry_list_visible_empty() {
        let state = CommandState::new();
        let result = cmd_registry_list(&state, None, &[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn cmd_registry_list_all_empty() {
        let state = CommandState::new();
        let result = cmd_registry_list_all(&state);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn cmd_lifecycle_report_unknown_label() {
        let state = CommandState::new();
        let result = cmd_lifecycle_report(
            &state,
            "plugin-test.nonexistent",
            None,
            Event::Enable,
        );
        assert!(result.is_err());
        // label_to_plugin_id 应解析成功，但 registry 中没有该插件
        assert_eq!(result.unwrap_err().code, ErrorCode::E_UNKNOWN_PLUGIN);
    }

    #[test]
    fn cmd_lifecycle_report_forged_identity() {
        let state = CommandState::new();
        let result = cmd_lifecycle_report(
            &state,
            "plugin-p.real",
            Some("p.fake"),
            Event::Enable,
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn cmd_events_publish_undeclared_topic_dropped() {
        let state = CommandState::new();
        let result = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.producer.ready",
            serde_json::json!({"ready": true}),
        );
        assert!(result.is_ok(), "未声明的 topic 不应报错，应静默丢弃");
        let pr = result.unwrap();
        assert!(pr.dropped, "未声明的 topic 应被丢弃");
    }

    #[test]
    fn cmd_events_publish_declared_topic() {
        let state = CommandState::new();
        // 先声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.producer.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 再发布
        let result = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.producer.ready",
            serde_json::json!({"ready": true}),
        );
        assert!(result.is_ok());
        let pr = result.unwrap();
        assert!(pr.delivered == 0, "无订阅者时应 delivered=0");
        assert!(!pr.dropped);
    }

    #[test]
    fn cmd_events_publish_wrong_publisher_dropped() {
        let state = CommandState::new();
        // 声明 topic（声明者 = p.producer）
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.producer.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 另一个插件尝试发布 → 被丢弃（不报错，但 dropped=true）
        let result = cmd_events_publish(
            &state,
            "p.forged",
            "plugin:p.producer.ready",
            serde_json::json!({"forged": true}),
        );
        // publish_request 对越界发布不报错（静默丢弃）
        assert!(result.is_ok());
        let pr = result.unwrap();
        assert!(pr.dropped, "越界发布应被丢弃");
    }

    #[test]
    fn cmd_events_subscribe_declared_public_topic() {
        let state = CommandState::new();
        // 声明 public topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.producer.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 另一个插件订阅 → 成功（public topic 无需审批）
        let result = cmd_events_subscribe(
            &state,
            "p.consumer",
            "window-1",
            "plugin:p.producer.ready",
        );
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert!(!outcome.duplicate);
        assert!(!outcome.token.is_empty());
    }

    #[test]
    fn cmd_events_subscribe_idempotent() {
        let state = CommandState::new();
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p", &decls).unwrap();
        }
        // 同 subscriber × window × topic 重复订阅 → 幂等返回同一 token
        let r1 = cmd_events_subscribe(&state, "c1", "w1", "plugin:p.ready").unwrap();
        let r2 = cmd_events_subscribe(&state, "c1", "w1", "plugin:p.ready").unwrap();
        assert_eq!(r1.token, r2.token);
        assert!(!r1.duplicate);
        assert!(r2.duplicate);
    }

    #[test]
    fn cmd_events_drain_completes_subscribe_publish_consume_chain() {
        // 断链回归测试：订阅 → 发布 → drain 必须取到帧。
        // 此前 drain 无任何调用者，队列只进不出，前端永远收不到事件。
        let state = CommandState::new();
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.producer.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }

        cmd_events_subscribe(&state, "p.consumer", "window-1", "plugin:p.producer.ready")
            .expect("subscribe should succeed");

        let pr = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.producer.ready",
            serde_json::json!({"v": 1}),
        )
        .expect("publish should succeed");
        assert_eq!(pr.delivered, 1, "publish must enqueue for the subscriber");

        // 取件：host_events_publish 走可靠语义（Request 通道）
        let frames = cmd_events_drain(&state, "p.consumer", "request")
            .expect("drain should succeed");
        assert_eq!(frames.len(), 1, "drain must return the queued frame");
        assert_eq!(frames[0].topic, "plugin:p.producer.ready");
        assert_eq!(frames[0].payload, serde_json::json!({"v": 1}));

        // event 通道不受影响（各通道队列独立）
        let event_frames = cmd_events_drain(&state, "p.consumer", "event").unwrap();
        assert!(event_frames.is_empty(), "channels are independent");

        // 取完即空（不重复投递）
        let again = cmd_events_drain(&state, "p.consumer", "request").unwrap();
        assert!(again.is_empty(), "frames must not be redelivered");
    }

    #[test]
    fn cmd_events_drain_rejects_unknown_kind() {
        let state = CommandState::new();
        let err = cmd_events_drain(&state, "c1", "bogus").expect_err("must reject");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn channel_kind_parse_matches_serde_kebab_case() {
        use tauron_host::eventbus::ChannelKind;
        assert_eq!(ChannelKind::parse("event"), Some(ChannelKind::Event));
        assert_eq!(ChannelKind::parse("request"), Some(ChannelKind::Request));
        assert_eq!(ChannelKind::parse("state"), Some(ChannelKind::State));
        assert_eq!(ChannelKind::parse("Event"), None, "大小写敏感");
        assert_eq!(ChannelKind::parse(""), None);
        // 与 serde 序列化互逆
        for kind in [ChannelKind::Event, ChannelKind::Request, ChannelKind::State] {
            let s = serde_json::to_value(kind).unwrap();
            assert_eq!(ChannelKind::parse(s.as_str().unwrap()), Some(kind));
        }
    }

    #[test]
    fn cmd_events_subscribe_private_topic_rejected() {
        let state = CommandState::new();
        // 声明私有 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.private".to_string(),
                public: false,
            }];
            bus.declare_topics("p", &decls).unwrap();
        }
        // 另一个插件订阅私有 topic → 拒绝
        let result = cmd_events_subscribe(&state, "c1", "w1", "plugin:p.private");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn cmd_events_unsubscribe_unknown_token_idempotent() {
        let state = CommandState::new();
        let result = cmd_events_unsubscribe(&state, "fake-token");
        assert!(result.is_ok(), "退订不存在的 token 应幂等成功");
    }

    #[test]
    fn cmd_events_full_cycle() {
        let state = CommandState::new();
        // 1. 声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 2. 订阅
        let sub = cmd_events_subscribe(&state, "p.consumer", "w1", "plugin:p.ready").unwrap();
        // 3. 发布
        let pub_result = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.ready",
            serde_json::json!({"ready": true}),
        )
        .unwrap();
        assert!(pub_result.delivered == 1, "应投递到 1 个订阅者");
        // 4. 退订
        cmd_events_unsubscribe(&state, &sub.token).unwrap();
    }

    #[test]
    fn cmd_cancel_unknown_call() {
        let state = CommandState::new();
        let result = cmd_cancel(&state, "unknown-call-id");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn cmd_call_end_unknown_call() {
        let state = CommandState::new();
        let result = cmd_call_end(&state, "unknown-call-id");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn cmd_settings_get_returns_null() {
        let state = CommandState::new();
        let result = cmd_settings_get(&state, "some.key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn cmd_notify_succeeds() {
        let state = CommandState::new();
        let result = cmd_notify(&state, "p.audio", "Title", "Body");
        assert!(result.is_ok());
    }

    #[test]
    fn notifications_list_and_read_roundtrip() {
        let state = CommandState::new();
        cmd_notify(&state, "com.a", "T1", "B1").unwrap();
        cmd_notify(&state, "com.b", "T2", "B2").unwrap();

        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 2);
        assert_eq!(snap["unread"], 2);
        let items = snap["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        // 时间倒序：最后推入的排最前；线形键必须 camelCase。
        assert_eq!(items[0]["title"], "T2");
        assert_eq!(items[0]["pluginId"], "com.b");

        // 单条已读。
        let id = items[0]["id"].as_str().unwrap().to_string();
        let r = cmd_notifications_read(&state, Some(&id)).unwrap();
        assert_eq!(r["marked"], 1);
        let snap = cmd_notifications_list(&state, Some(1)).unwrap();
        assert_eq!(snap["items"].as_array().unwrap().len(), 1, "limit 生效");
        assert_eq!(snap["items"][0]["read"], true);
        assert_eq!(snap["unread"], 1, "unread 是全量计数，不受 limit 影响");

        // 全部已读。
        let r = cmd_notifications_read(&state, None).unwrap();
        assert_eq!(r["marked"], 1);
        assert_eq!(cmd_notifications_list(&state, None).unwrap()["unread"], 0);

        // 未知 id：marked 0，不 panic。
        assert_eq!(
            cmd_notifications_read(&state, Some("nope")).unwrap()["marked"],
            0
        );
    }

    #[test]
    fn uninstall_recycles_plugin_notifications() {
        let state = CommandState::new();
        state
            .registry
            .install(&empty_index(), test_manifest("com.a"))
            .unwrap();
        cmd_notify(&state, "com.a", "T", "B").unwrap();
        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();
        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 0, "卸载必须回收该插件的通知");
    }

    // ────────────────────────────────────────────────────────────
    // R5 / P0-1：流式帧走完整命令面
    // ────────────────────────────────────────────────────────────

    /// 记录型载体：断言「帧真的派发到了接收方」。
    #[derive(Default)]
    struct StreamRecorder {
        frames: parking_lot::Mutex<Vec<StreamFrame>>,
    }

    impl tauron_host::stream::StreamSink for StreamRecorder {
        fn send(&self, frame: &StreamFrame) -> HostResult<()> {
            self.frames.lock().push(frame.clone());
            Ok(())
        }
    }

    impl StreamRecorder {
        fn frames(&self) -> Vec<StreamFrame> {
            self.frames.lock().clone()
        }
    }

    fn installed_state(id: &str) -> CommandState {
        let state = CommandState::new();
        state
            .registry
            .install(&empty_index(), test_manifest(id))
            .unwrap();
        // 发起调用前必须处于 Enabled：`call_begin` 对 INSTALLED 直接拒绝
        // （E_PLUGIN_DISABLED），这是生命周期门，不是本轮的流式逻辑。
        cmd_registry_admin(&state, id, RegistryAdminOp::Enable).unwrap();
        state
    }

    /// 方案 R5「验证」项：**写 3 帧 → 收 3 帧 + end**，且 seq 连续。
    #[test]
    fn stream_roundtrip_three_frames_then_end() {
        let state = installed_state("com.a");
        let call = cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "format", serde_json::json!({}))
            .unwrap();

        // wire 层把 channel 转成 sink 后在这里登记（生产路径见 `wire_plugin_call`）。
        let sink = Arc::new(StreamRecorder::default());
        state
            .registry
            .stream_bind(&call.call_id, &call.plugin_id, sink.clone())
            .unwrap();

        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();
        assert_eq!(opened.call_id, call.call_id);

        for i in 1..=3u64 {
            let f = cmd_stream_write(
                &state,
                "com.a",
                &opened.stream_id,
                Some(serde_json::json!({ "i": i })),
                None,
            )
            .unwrap();
            assert_eq!(f.seq, i, "seq 由宿主铸且连续");
            assert_eq!(f.kind, StreamKind::Data);
        }
        let end = cmd_stream_close(&state, "com.a", &opened.stream_id, "end").unwrap();
        assert_eq!(end.seq, 4, "终帧占一个 seq");

        let frames = sink.frames();
        assert_eq!(frames.len(), 4, "接收方必须真的收到 3 帧 + end");
        assert_eq!(frames[3].kind, StreamKind::End);
        assert_eq!(frames[1].args_json.as_ref().unwrap()["i"], 2);

        // 终帧后句柄失效（不静默成功）。
        let err = cmd_stream_write(&state, "com.a", &opened.stream_id, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
    }

    /// `argsRaw` 是二进制载荷的**真实出口**（P0-1 的全部意义）：
    /// 字节到达接收方时仍是字节，不经 base64、也不被丢弃。
    #[test]
    fn stream_carries_binary_payload_without_base64() {
        let state = installed_state("com.a");
        let call = cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
            .unwrap();
        let sink = Arc::new(StreamRecorder::default());
        state
            .registry
            .stream_bind(&call.call_id, &call.plugin_id, sink.clone())
            .unwrap();
        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();

        let bytes = vec![0u8, 159, 146, 150, 255];
        cmd_stream_write(&state, "com.a", &opened.stream_id, None, Some(bytes.clone())).unwrap();

        let frames = sink.frames();
        assert_eq!(frames[0].args_raw.as_deref(), Some(bytes.as_slice()));
        assert!(frames[0].args_json.is_none());
    }

    /// 没有载体的调用**不得**开出「帧进虚空」的流：宁可显式失败。
    #[test]
    fn stream_open_without_carrier_is_rejected() {
        let state = installed_state("com.a");
        let call = cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
            .unwrap();
        let err = cmd_stream_open(&state, "com.a", &call.call_id).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
        assert!(err.message.contains("没有帧载体"), "message={}", err.message);
    }

    /// 跨插件写帧必须被拒（句柄是 UUID，但不靠「猜不到」兜底）。
    #[test]
    fn stream_write_from_another_plugin_is_denied() {
        let state = installed_state("com.a");
        state
            .registry
            .install(&empty_index(), test_manifest("com.b"))
            .unwrap();
        let call = cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
            .unwrap();
        let sink = Arc::new(StreamRecorder::default());
        state
            .registry
            .stream_bind(&call.call_id, &call.plugin_id, sink.clone())
            .unwrap();
        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();

        let err = cmd_stream_write(&state, "com.b", &opened.stream_id, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(sink.frames().is_empty(), "被拒的写不得留下帧");
    }

    /// 关流的 `kind` 是闭集：`data` 不是终帧，`done` 是拼写错误。
    #[test]
    fn stream_close_rejects_non_terminal_or_unknown_kind() {
        let state = installed_state("com.a");
        let call = cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
            .unwrap();
        let sink = Arc::new(StreamRecorder::default());
        state
            .registry
            .stream_bind(&call.call_id, &call.plugin_id, sink)
            .unwrap();
        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();

        let err = cmd_stream_close(&state, "com.a", &opened.stream_id, "data").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        let err = cmd_stream_close(&state, "com.a", &opened.stream_id, "done").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("未知帧种类"), "message={}", err.message);
        // 未终结的句子柄应仍可写。
        assert!(cmd_stream_write(&state, "com.a", &opened.stream_id, None, None).is_ok());
    }

    #[test]
    fn cmd_recover_boot_succeeds() {
        let state = CommandState::new();
        let result = cmd_recover_boot(&state);
        assert!(result.is_ok());
        let json = result.unwrap();
        assert_eq!(json["phase"], "normal");
        // 线名必须与 TS RecoveryBootResult 消费字段一致（camelCase）。
        assert_eq!(json["phaseName"], "正常启动");
        assert_eq!(json["counter"]["consecutiveFailures"], 0);
        assert_eq!(json["counter"]["safemodeFailures"], 0);
    }

    #[test]
    fn cmd_recover_report_drives_phase_and_reconciles_registry() {
        let state = CommandState::new();
        // 注册表里有一个（非必需）插件：安全模式应该把它标记为禁用。
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 表外 outcome 直接拒绝——不静默当成 failure 处理。
        assert_eq!(
            cmd_recover_report(&state, "crashed", None).unwrap_err().code,
            ErrorCode::E_INVALID_MANIFEST
        );

        // 两次失败 → 安全模式，且判定要落到注册表的 disabledBySafemode 上。
        for _ in 0..2 {
            let r = cmd_recover_report(&state, "failure", None).unwrap();
            assert_eq!(r["engineAction"], "bootFailure");
            assert_eq!(r["outcome"], "failure");
        }
        assert_eq!(cmd_recover_boot(&state).unwrap()["phase"], "safemode");
        assert!(find_summary(&state, "com.a").unwrap().disabled_by_safemode,
            "安全模式下非必需插件必须被标记");
        // 上报即结论明确：bootInFlight 必须清零，否则下次启动会重复计数。
        assert_eq!(
            cmd_recover_boot(&state).unwrap()["persistence"]["bootInFlight"],
            serde_json::json!(false)
        );

        // 上报成功 → 计数清零 + 阶段回 normal + 注册表禁用标记清除。
        let r = cmd_recover_report(&state, "success", None).unwrap();
        assert_eq!(r["engineAction"], "bootSuccess");
        assert_eq!(r["phase"], "normal");
        assert!(!find_summary(&state, "com.a").unwrap().disabled_by_safemode,
            "恢复正常后禁用标记必须清除");
    }

    #[test]
    fn cmd_recover_report_failure_scoped_to_trial_plugin() {
        let state = CommandState::new();

        // 正常阶段不允许试验性启用。
        assert_eq!(
            cmd_recover_trial_enable(&state, "com.a").unwrap_err().code,
            ErrorCode::E_STATE_INVALID_TRANSITION
        );

        // 两次失败 → 安全模式。
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        let r = cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(r["phase"], "safemode");
        assert_eq!(r["engineAction"], "notInRegistry");
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("trial-enable")
        );
        // 试启中的插件**不算禁用**：`disabledPlugins` 按 `!is_enabled()` 过滤，
        // `TrialEnable` 属于启用。TS 侧 `DisabledPlugin['state']` 的取值集合
        // 据此收窄——这里锁住该不变式，改过滤条件时必须同步改类型。
        let ids: Vec<&str> = r["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["pluginId"].as_str())
            .collect();
        assert!(!ids.contains(&"com.a"), "试启中的插件不得出现在 disabledPlugins");

        // 试启中失败 → 按试验失败记（只累该插件的试验次数）。
        let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
        assert_eq!(r["engineAction"], "disabled-by-safemode");
        assert_eq!(r["suspectedPlugin"], "com.a");
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("disabled-by-safemode")
        );
        // 全局启动计数**不得**被这次试验失败累加——那会把单个插件的失败误判
        // 成整个应用的崩溃。
        assert_eq!(r["counter"]["consecutiveFailures"], 2);
        assert_eq!(r["phase"], "safemode");

        // 预算耗尽：不可再次试启。
        assert_eq!(
            cmd_recover_trial_enable(&state, "com.a").unwrap_err().code,
            ErrorCode::E_PLUGIN_DISABLED
        );
    }

    /// 试启主链路闭环：安全模式 → 标记在册 → 试启（引擎改判 + 注册表走 D28
    /// `TrialEnable`，清标志）→ 试验失败（引擎回落 + 对账重新标记禁用）。
    ///
    /// 断链回归：若试启仍发 `SafemodeExit`，注册表不记独立试验预算、
    /// `trial_from_safemode` 不置位，该插件自行上报错误时走不到 D28 回落
    /// （与引擎的试验判定各自为政）；若不前置对账，刚装载、尚未被标记的
    /// 插件停在 `Installed`，试启事件会撞非法迁移。
    #[test]
    fn cmd_recover_trial_enable_drives_registry_trial_state() {
        let state = CommandState::new();
        state
            .registry
            .install(&empty_index(), test_manifest("com.a"))
            .unwrap();

        // 两次失败 → 安全模式，对账把 com.a 标进注册表（DISABLED + 标志）。
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Disabled);
        assert!(s.disabled_by_safemode);

        // 试启：引擎改判 trial-enable + 注册表走 D28 TrialEnable（清标志、记预算）。
        let r = cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(r["engineAction"], "trialEnable:ENABLED");
        assert_eq!(r["phaseReconcile"]["scanned"], serde_json::json!(1));
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Enabled);
        assert!(!s.disabled_by_safemode, "试启必须清安全模式标志");
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("trial-enable")
        );

        // 试验失败 → 引擎回落，对账把注册表重新标回禁用（两侧闭环）。
        let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
        assert_eq!(r["engineAction"], "disabled-by-safemode");
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Disabled);
        assert!(s.disabled_by_safemode, "试验失败后注册表必须重新标记");
    }

    /// D28 反向同步边：试启期间插件**自报错误**（`ErrorRetryable`）→ 注册表按
    /// D28 回落并计一次试验失败，引擎必须同步得知。若不同步，引擎仍记它
    /// `TrialEnable`，同一调用尾部的对账会把注册表刚打上的安全模式标志当成
    /// 不一致再补发 `SafemodeExit` 清掉——「插件报错 → 标志被清 → 插件重入」
    /// 往返抖动。
    #[test]
    fn lifecycle_trial_error_syncs_engine_trial_failure() {
        let state = CommandState::new();
        state
            .registry
            .install(&empty_index(), test_manifest("com.a"))
            .unwrap();
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("trial-enable")
        );

        // 插件 webview 自报试启期间的可重试错误。
        let out = cmd_lifecycle_report(
            &state,
            "plugin-com.a",
            Some("com.a"),
            tauron_host::lifecycle::Event::ErrorRetryable,
        )
        .unwrap();
        assert!(!out.illegal, "试启期间报错必须走 D28 回落迁移");

        // 引擎已同步为回落，注册表重新标记禁用——对账静默（不再补发清标志事件）。
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("disabled-by-safemode")
        );
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Disabled);
        assert!(
            s.disabled_by_safemode,
            "标志必须保留：引擎已知失败，对账不得把它清掉"
        );
        // 再对账一次，证明是稳态而非一次侥幸。
        reconcile_recovery_phase(&state);
        assert!(find_summary(&state, "com.a").unwrap().disabled_by_safemode);
    }

    #[test]
    fn recover_boot_and_report_are_lock_order_safe() {
        // 回归测试：恢复链上有两把锁（recovery / recovery_store），任何一处
        // 「持锁 A 再取锁 B」与「持锁 B 再取锁 A」并存就是 ABBA 死锁；而
        // 「持 recovery_store 锁调用 recovery_boot_payload」是自死锁
        // （`parking_lot` 不重入）。两者都表现为**空转而非报错**，所以必须有
        // 并发用例盯着——单线程用例抓不到。
        //
        // 语义断言放在单线程段：两条线程共享同一个引擎，失败计数会互相干扰
        // （可能已被推到修复模式），断言 trial 结果会变成对竞态的断言。
        {
            let s = CommandState::new();
            assert!(cmd_recover_report(&s, "failure", None).is_ok());
            assert!(cmd_recover_report(&s, "failure", None).is_ok());
            assert!(cmd_recover_trial_enable(&s, "com.t0").is_ok());
            assert!(cmd_recover_report(&s, "failure", Some("com.t0")).is_ok());
            assert!(cmd_recover_report(&s, "success", None).is_ok());
            assert!(cmd_recover_boot(&s).is_ok());
        }

        use std::sync::{Arc, Barrier};
        let state = Arc::new(CommandState::new());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let state = state.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0..200usize {
                    let pid = format!("com.t{i}");
                    // 只关心「都能返回」——死锁会表现为永不返回，而不是 Err。
                    let _ = cmd_recover_boot(&state);
                    let _ = cmd_recover_report(&state, "failure", None);
                    let _ = cmd_recover_report(&state, "failure", Some(&pid));
                    let _ = cmd_recover_trial_enable(&state, &pid);
                    let _ = cmd_recover_report(&state, "success", None);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    // ────────────────────────────────────────────────────────────
    // R7-1：崩溃 → 重启 → boot 回传 lastContext
    // ────────────────────────────────────────────────────────────

    /// 带恢复持久化目录的装配配置（其余取缺省）。
    fn recovery_cfg(dir: &std::path::Path) -> AdapterConfig {
        AdapterConfig {
            recovery_data_dir: Some(dir.to_path_buf()),
            ..AdapterConfig::default()
        }
    }

    #[test]
    fn recover_boot_returns_last_context_after_crash_and_restart() {
        let t = tempfile::tempdir().unwrap();

        // ── 第 1 轮：上报一次带插件归因的失败（干净退出，in_flight 被清零）。
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
            assert_eq!(r["outcome"], "failure");
            let ctx = r["lastContext"].as_array().unwrap();
            assert_eq!(ctx.len(), 1, "本轮失败必须立刻进上下文");
            assert_eq!(ctx[0]["pluginId"], "com.a");
            assert_eq!(ctx[0]["failureKind"], "boot-failure");
        }

        // ── 第 2 轮：重启（干净退出后重开）→ 上下文必须从磁盘回来。
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            let boot = cmd_recover_boot(&state).unwrap();
            assert_eq!(boot["loadSource"], "restored");
            // `load` 一律把本轮标成「进行中」。
            assert_eq!(boot["persistence"]["bootInFlight"], true);
            // 上一轮上报过 → 本次不该再算一次崩溃（计数仍是 1，不是 2）。
            assert_eq!(boot["counter"]["consecutiveFailures"], 1);
            let ctx = boot["lastContext"].as_array().unwrap();
            assert_eq!(ctx.len(), 1, "上一轮的上下文必须跨进程回传");
            assert_eq!(ctx[0]["pluginId"], "com.a");
            assert_eq!(ctx[0]["failureKind"], "boot-failure");
            assert!(ctx[0]["ts"].as_u64().unwrap() > 0, "时间戳不能是 0");
            // 本轮**没有**上报就退出 = 崩溃。第 3 轮应能识别。
        }

        // ── 第 3 轮：识别出上一次崩溃，并把崩溃本身追加成一条摘要。
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["persistence"]["bootInFlight"], true, "本轮尚未上报");
        assert_eq!(boot["counter"]["consecutiveFailures"], 2, "崩溃由 load 自己计数");
        let ctx = boot["lastContext"].as_array().unwrap();
        assert_eq!(ctx.len(), 2, "历史 + 本次崩溃各一条");
        assert_eq!(ctx[0]["pluginId"], "com.a", "最旧的是上一轮那条");
        assert_eq!(ctx[1]["pluginId"], serde_json::Value::Null, "崩溃无法归因到插件");
        assert_eq!(ctx[1]["failureKind"], "boot-failure");
        assert_eq!(ctx[1]["trial"], false);
    }

    #[test]
    fn recover_boot_returns_context_after_a_safemode_restart() {
        // R7 验证项原文：「safemode 重启后 cmd_recover_boot 回传 context」。
        let t = tempfile::tempdir().unwrap();
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            cmd_recover_report(&state, "failure", None).unwrap();
            cmd_recover_report(&state, "failure", None).unwrap();
        }

        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["phase"], "safemode");
        assert_eq!(boot["phaseName"], "安全模式");
        let ctx = boot["lastContext"].as_array().unwrap();
        assert_eq!(ctx.len(), 2, "两次失败都必须回传");
        assert_eq!(ctx[0]["consecutiveFailures"], 1);
        assert_eq!(ctx[1]["consecutiveFailures"], 2);
        assert_eq!(ctx[1]["phase"], "normal", "两次都发生在正常模式阶段");
        assert_eq!(ctx[1]["failureKind"], "boot-failure");
    }

    #[test]
    fn recover_boot_context_keeps_the_attributed_plugin_without_spending_its_trial_budget() {
        // 归因进上下文，但**不得**入账试验预算：安全模式里某插件崩过一次
        // 就永久失去试启用机会，是越权式的连带惩罚。
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        cmd_recover_report(&state, "failure", None).unwrap();
        cmd_recover_report(&state, "failure", Some("com.suspect")).unwrap();

        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["phase"], "safemode");
        let ctx = boot["lastContext"].as_array().unwrap();
        assert_eq!(ctx[1]["pluginId"], "com.suspect", "疑似故障插件要留在上下文里");
        // 该插件仍可试验性启用（没有被记进 trial_failures）。
        let enabled = cmd_recover_trial_enable(&state, "com.suspect");
        assert!(enabled.is_ok(), "疑似归因不该消耗试验预算：{enabled:?}");
    }

    #[test]
    fn recover_boot_keeps_every_pre_existing_field() {
        // 线形回归：既有字段不得改名/删除（前端已消费），R7 只做**增加**。
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        for field in [
            "phase",
            "phaseName",
            "counter",
            "disabledPlugins",
            "requiredPlugins",
            "loadSource",
            "persistence",
        ] {
            assert!(boot.get(field).is_some(), "R7 不得删掉既有字段 {field}");
        }
        for field in ["consecutiveFailures", "safemodeFailures"] {
            assert!(boot["counter"].get(field).is_some(), "counter 缺 {field}");
        }
        for field in ["enabled", "dir", "bootInFlight", "lastError"] {
            assert!(
                boot["persistence"].get(field).is_some(),
                "persistence 缺 {field}"
            );
        }
        assert!(boot["lastContext"].is_array(), "R7 新增 lastContext");
    }

    #[test]
    fn recover_boot_context_is_empty_on_a_fresh_boot_not_fabricated() {
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["loadSource"], "fresh");
        assert!(
            boot["lastContext"].as_array().unwrap().is_empty(),
            "首次启动没有上下文，不许伪造"
        );
    }

    #[test]
    fn recovery_marker_write_failure_does_not_break_boot_or_report() {
        // 落盘 best-effort：把目标路径用目录占住，让 rename 失败。
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("recovery-state.json")).unwrap();

        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        // 启动查询照常成功，失败原因如实暴露在 persistence.lastError。
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["persistence"]["enabled"], true);
        assert!(
            boot["persistence"]["lastError"]
                .as_str()
                .unwrap()
                .contains("落盘失败"),
            "落盘失败必须可见：{}",
            boot["persistence"]["lastError"]
        );
        // 上报也照常成功——落盘失败绝不能阻断启动流程。
        let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
        assert_eq!(r["outcome"], "failure");
        let ctx = r["lastContext"].as_array().unwrap();
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0]["pluginId"], "com.a", "写失败不影响内存里的上下文");
    }

    // ══════════════════════════════════════════════════════════════════════
    // R7 收口：命令层身份判定（主体解析 / 特权仅主窗 / 设置族键空间）
    //
    // 这些用例都走 `*_as` 系列入口——**就是 wire 层包装器转调的那两个函数**
    // （`window.label()` 那一跳留在 `tauri.rs`，见那里的编译期签名证据）。
    // 每条「插件被拒」的用例都配一条「主窗通过」的对照，证明拒绝来自身份，
    // 而不是命令被整体关死。
    // ══════════════════════════════════════════════════════════════════════

    fn plugin_caller(id: &str) -> Caller {
        Caller::Plugin(id.to_string())
    }

    /// 畸形 label **绝不**降级成主窗（提权），且必须报既有的身份码。
    #[test]
    fn malformed_label_is_rejected_and_never_downgrades_to_main_window() {
        // 都是 `plugin-` 前缀但 id 非法：空 / 单段 / 首段大写 / 结尾点。
        for bad in ["plugin-", "plugin-p", "plugin-P.a", "plugin-p.a."] {
            let err = Caller::from_label(bad)
                .expect_err("畸形 label 必须被拒——降级成主窗即等于伪造 label 提权");
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{bad}");
            assert!(err.message.contains(bad), "错误消息要带原始 label：{}", err.message);
            assert!(
                err.message.contains("绝不降级"),
                "消息必须点明不降级：{}",
                err.message
            );
        }
        // 正常解析（既有模型）：无前缀 = 主窗；`plugin-<合法 id>` = 插件。
        assert_eq!(Caller::from_label("main").unwrap(), Caller::MainWindow);
        assert_eq!(
            Caller::from_label("plugin-com.example.a").unwrap(),
            Caller::Plugin("com.example.a".to_string())
        );
        assert!(!Caller::from_label("plugin-com.example.a").unwrap().is_main_window());
        assert_eq!(
            Caller::from_label("plugin-com.example.a").unwrap().plugin_id(),
            Some("com.example.a")
        );
    }

    /// 档位表与代码判定**同源**：表里每一条 `privileged` 命令都必须被
    /// `require_main_window` 拒绝插件主体（表增了特权命令却写下别的档位，这里就红）。
    #[test]
    fn every_privileged_command_in_the_authz_table_denies_plugins() {
        let mut checked = 0;
        for c in tauron_host::authz::ADMIN_COMMANDS {
            assert_eq!(
                c.tier,
                tauron_host::authz::AuthTier::Privileged,
                "{} 在 ADMIN_COMMANDS 里必须是 privileged",
                c.command
            );
            let err = require_main_window(&plugin_caller("com.x"), c.command).unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{}", c.command);
            assert!(
                require_main_window(&Caller::MainWindow, c.command).is_ok(),
                "主窗必须是特权命令的合法主体"
            );
            checked += 1;
        }
        assert!(checked >= 3, "档位表里的特权命令不该少于 3 条（校验没跑空）");
    }

    /// `host_registry_admin`：插件主体被拒且**零副作用**；主窗能禁用。
    #[test]
    fn plugin_caller_cannot_use_registry_admin_but_main_window_can() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let id = PluginId::new("com.a").unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed
        );

        let err = cmd_registry_admin_as(
            &plugin_caller("com.a"),
            &state,
            "com.a",
            RegistryAdminOp::Disable,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed,
            "拒绝路径不得改插件状态（判定必须在副作用之前）"
        );

        // 对照：主窗能禁用（证明拒绝来自身份而不是命令被整体关死）。
        let out =
            cmd_registry_admin_as(&Caller::MainWindow, &state, "com.a", RegistryAdminOp::Disable)
                .unwrap();
        assert_eq!(out.to, tauron_host::lifecycle::State::Disabled);
    }

    /// `host_registry_list_all`：插件主体被拒（全量列表本身就是 scoped-read 要遮的信息）。
    #[test]
    fn plugin_caller_cannot_list_all_plugins_but_main_window_can() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let err = cmd_registry_list_all_as(&plugin_caller("com.a"), &state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        let all = cmd_registry_list_all_as(&Caller::MainWindow, &state).unwrap();
        assert_eq!(all.len(), 1, "主窗必须拿得到全量");
    }

    /// `host_runtime_spawn`：插件主体不得起**别人的** sidecar（越权执行原语）。
    ///
    /// 判别力设计：攻击者目标选一个**尚未启动**的插件 B——若判定缺失，这次调用会
    /// 真的走到启动面（`call_count` 变 2）。用同一个插件会因幂等返回旧租约而
    /// 「看起来也对」，失去判别力。
    #[test]
    fn plugin_caller_cannot_spawn_another_plugins_sidecar() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        for (id, sidecar) in [("com.proc.a", "a.exe"), ("com.proc.b", "b.exe")] {
            state
                .registry
                .install(&empty_index(), process_manifest(id, Some(sidecar)))
                .unwrap();
            enabled_process_plugin(&state, id);
        }

        // 对照：主窗起 A → 真的经过启动面。
        cmd_runtime_spawn_as(&Caller::MainWindow, &state, "com.proc.a", &valid_profile()).unwrap();
        assert_eq!(fake.call_count(), 1);

        // 插件 A 的主体去起 B：拒绝，且启动面**一次都不许新增调用**。
        let err = cmd_runtime_spawn_as(
            &plugin_caller("com.proc.a"),
            &state,
            "com.proc.b",
            &valid_profile(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            fake.call_count(),
            1,
            "拒绝路径一次都不许新增启动调用——否则等于插件越权起了 B 的 sidecar"
        );
        assert_eq!(
            state.registry.runtime_len(),
            1,
            "拒绝路径不得给 B 留下租约"
        );
    }

    /// `host_runtime_health`：插件主体不得查进程健康（带 pid、且会驱动崩溃检测）。
    #[test]
    fn plugin_caller_cannot_query_process_health_but_main_window_can() {
        let (state, _fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle =
            cmd_runtime_spawn_as(&Caller::MainWindow, &state, "com.proc", &valid_profile()).unwrap();

        let err = cmd_runtime_health_as(&plugin_caller("com.proc"), &state, &handle.lease)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        // 对照：主窗用**同一个租约**查得到（证明拒绝来自身份，不是租约无效）。
        let health =
            cmd_runtime_health_as(&Caller::MainWindow, &state, &handle.lease).unwrap();
        assert_eq!(health.pid, handle.pid);
        assert!(health.alive);
    }

    /// 设置族键空间：插件只能碰自己的命名空间；主窗任意键（现状不变）。
    #[test]
    fn plugin_caller_can_only_touch_its_own_settings_namespace() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");

        // 自己的键：读写都通过（含命名空间根与子键）。
        cmd_settings_set_as(&a, &state, "plugin:p.a.theme", serde_json::json!("dark")).unwrap();
        cmd_settings_set_as(&a, &state, "plugin:p.a", serde_json::json!("root")).unwrap();
        assert_eq!(
            cmd_settings_get_as(&a, &state, "plugin:p.a.theme").unwrap(),
            serde_json::json!("dark")
        );

        // 别人的键：读、写都拒；写被拒后 Store 里**没有**任何痕迹。
        for key in ["plugin:p.b.theme", "theme", "host.settings"] {
            let err = cmd_settings_set_as(&a, &state, key, serde_json::json!("hijack"))
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "写 {key} 必须被拒");
            assert_eq!(
                cmd_settings_get_as(&Caller::MainWindow, &state, key).unwrap(),
                serde_json::Value::Null,
                "越界写入不得落进 Store（{key}）"
            );
        }
        let err = cmd_settings_get_as(&a, &state, "plugin:p.b.theme").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "读别人的键也必须拒");

        // **前缀包含误伤**：`plugin:p.a` 的插件不得借 `starts_with` 写穿 `plugin:p.ab.x`。
        let err =
            cmd_settings_set_as(&a, &state, "plugin:p.ab.x", serde_json::json!(1)).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::E_AUTH_DENIED,
            "`plugin:p.a` 不是 `plugin:p.ab.x` 的命名空间（前缀包含误伤）"
        );
        // 反向同理：`plugin:p.ab` 碰不到 `plugin:p.a.x`，但自己的键没问题。
        let ab = plugin_caller("p.ab");
        assert!(cmd_settings_set_as(&ab, &state, "plugin:p.a.x", serde_json::json!(1)).is_err());
        cmd_settings_set_as(&ab, &state, "plugin:p.ab.x", serde_json::json!(1)).unwrap();

        // 对照：主窗任意键（含宿主级键）读写照旧。
        cmd_settings_set_as(&Caller::MainWindow, &state, "theme", serde_json::json!("light"))
            .unwrap();
        assert_eq!(
            cmd_settings_get_as(&Caller::MainWindow, &state, "theme").unwrap(),
            serde_json::json!("light")
        );
    }

    /// 迁移入口：**仅主窗**（整份文档级操作无法用「只准动自己的键」表达）。
    #[test]
    fn settings_migration_entries_are_main_window_only() {
        let state = CommandState::new();
        let plugin = plugin_caller("p.a");

        let err = cmd_settings_adopt_legacy_as(&plugin, &state, serde_json::json!({"theme": "dark"}))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            host_settings_data_version(&state),
            None,
            "拒绝路径不得标注数据版本"
        );
        assert_eq!(
            cmd_settings_get_as(&Caller::MainWindow, &state, "theme").unwrap(),
            serde_json::Value::Null,
            "拒绝路径不得写入任何设置"
        );

        let err = cmd_settings_migrate_as(&plugin, &state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            host_settings_data_version(&state),
            None,
            "拒绝路径不得把空数据标成最新版"
        );

        // 对照：主窗走得通（接手 → 迁一步 → 再迁 0 步）。
        cmd_settings_adopt_legacy_as(
            &Caller::MainWindow,
            &state,
            serde_json::json!({"plugin:p.a.theme": "dark"}),
        )
        .unwrap();
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V1)
        );
        assert_eq!(cmd_settings_migrate_as(&Caller::MainWindow, &state).unwrap(), 1);
        assert_eq!(cmd_settings_migrate_as(&Caller::MainWindow, &state).unwrap(), 0);
    }

    // ──────────────────────────────────────────────────────────────────────
    // 轮 11 第二批：身份绑定参数 / 主窗专属（跨插件副作用与冒名原语）
    //
    // 每条"拒绝"都配了两类对照，否则测试没有判别力：
    // ① 主窗（或插件对自己）**能**通过——证明拒绝来自身份而不是命令被整体关死；
    // ② 拒绝路径**零副作用**——证明判定落在副作用之前（而不是"先做了再报错"）。
    // ──────────────────────────────────────────────────────────────────────

    /// `host_notify`：署名必须是自己。插件以别人/宿主名义发通知一律拒绝且零副作用。
    #[test]
    fn plugin_caller_can_only_notify_under_its_own_name() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");

        // 自己名义 → 入缓冲（也进兼容日志）。
        cmd_notify_as(&a, &state, "p.a", "T", "B").unwrap();
        assert_eq!(state.notify_store.lock().len(), 1);
        assert_eq!(state.notifications.lock().len(), 1);

        // 冒名（别的插件 / 宿主）→ 拒绝，且**一条都不入**：环形缓冲、兼容日志、
        // 分发日志（未注入 sink 时不写日志，这里以两个真实存储为准）都不动。
        for claimed in ["p.b", "host", "tauron"] {
            let err = cmd_notify_as(&a, &state, claimed, "T", "B").unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "以 `{claimed}` 名义必须被拒");
            assert_eq!(
                state.notify_store.lock().len(),
                1,
                "拒绝路径不得入环形缓冲（署名 {claimed}）"
            );
            assert_eq!(
                state.notifications.lock().len(),
                1,
                "拒绝路径不得写兼容日志（署名 {claimed}）"
            );
        }
        assert_eq!(cmd_notifications_list(&state, None).unwrap()["total"], 1);

        // 对照：主窗可发任意署名（宿主自己就是这些通知的来源）。
        cmd_notify_as(&Caller::MainWindow, &state, "p.b", "T", "B").unwrap();
        cmd_notify_as(&Caller::MainWindow, &state, "host", "T", "B").unwrap();
        assert_eq!(state.notify_store.lock().len(), 3);

        // **危害实证**（不是推测）：判定缺席时同一个存储会照收别人的署名——
        // 直接调不过身份的核心 `cmd_notify` 即可复现。
        cmd_notify(&state, "p.b", "T", "B").unwrap();
        assert_eq!(
            state.notify_store.lock().len(),
            4,
            "冒名通知在不过身份的核心上会被照收——这正是 gate 要挡的东西"
        );
    }

    /// `host_i18n_cleanup_plugin`：插件只能清自己的文案（清别人 = 跨插件销毁）。
    #[test]
    fn plugin_caller_can_only_clean_its_own_i18n_bundle() {
        let state = CommandState::new();
        // 两个插件各装一份文案（主窗代装：这是正当路径）。
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "A 的标题")]), Some("p.a")).unwrap();
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "B 的标题")]), Some("p.b")).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        let b_key = I18nEngine::plugin_key("p.b", "title");
        assert_eq!(cmd_i18n_t(&state, &b_key).unwrap(), "B 的标题");

        // 清别人 → 拒绝，且 B 的 bundle **仍在**（逐 key 可读）。
        let err = cmd_i18n_cleanup_plugin_as(&plugin_caller("p.a"), &state, "p.b").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            cmd_i18n_t(&state, &b_key).unwrap(),
            "B 的标题",
            "拒绝路径不得销毁别人的文案（否则 B 的界面会变成一屏原始 key）"
        );

        // 清自己 → 生效（证明"拒绝"不是命令整体不可用）。
        let r = cmd_i18n_cleanup_plugin_as(&plugin_caller("p.a"), &state, "p.a").unwrap();
        assert_eq!(r["removed"], 1);
        let a_key = I18nEngine::plugin_key("p.a", "title");
        assert_eq!(
            cmd_i18n_t(&state, &a_key).unwrap(),
            a_key,
            "清掉之后该 key 落空（按设计返回 key 本身）"
        );

        // 对照：主窗清任意插件（卸载/禁用的回收路径）。
        let r = cmd_i18n_cleanup_plugin_as(&Caller::MainWindow, &state, "p.b").unwrap();
        assert_eq!(r["removed"], 1);
    }

    /// `host_i18n_load`：命名空间归属必须是自己；`None`（宿主文案）插件不得主张。
    #[test]
    fn plugin_caller_can_only_load_into_its_own_i18n_namespace() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");

        // 自己命名空间 → 装载成功。
        cmd_i18n_load_as(&a, &state, "zh-CN", entries(&[("title", "我的")]), Some("p.a"))
            .unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        assert_eq!(
            cmd_i18n_t(&state, &I18nEngine::plugin_key("p.a", "title")).unwrap(),
            "我的"
        );

        // 别人的命名空间 → 拒绝，且**引擎里没有** `plugin:p.b.oc.*`。
        let err = cmd_i18n_load_as(
            &a,
            &state,
            "zh-CN",
            entries(&[("title", "投毒")]),
            Some("p.b"),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(
            state
                .i18n
                .lock()
                .get_bundle("zh-CN")
                .and_then(|b| b.get(&I18nEngine::plugin_key("p.b", "title")))
                .is_none(),
            "命名空间投毒：拒绝路径不得往别人的命名空间写任何 key"
        );

        // 宿主级文案（`None`）→ 插件不得主张。
        let err = cmd_i18n_load_as(&a, &state, "zh-CN", entries(&[("oc.app", "伪造")]), None)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(
            state
                .i18n
                .lock()
                .get_bundle("zh-CN")
                .and_then(|b| b.get("oc.app"))
                .is_none(),
            "宿主命名空间不得被插件写入"
        );

        // 对照：主窗带 `None` 装载宿主级文案照旧通过。
        cmd_i18n_load_as(
            &Caller::MainWindow,
            &state,
            "zh-CN",
            entries(&[("oc.app", "应用标题")]),
            None,
        )
        .unwrap();
        assert_eq!(cmd_i18n_t(&state, &"oc.app".to_string()).unwrap(), "应用标题");
        // 主窗也能代插件装载（卸载/重装等宿主流程）。
        cmd_i18n_load_as(
            &Caller::MainWindow,
            &state,
            "zh-CN",
            entries(&[("title", "代装")]),
            Some("p.b"),
        )
        .unwrap();
        assert_eq!(
            cmd_i18n_t(&state, &I18nEngine::plugin_key("p.b", "title")).unwrap(),
            "代装"
        );
    }

    /// 把 state 推到「安全模式 + `p.b` 处于试验性启用」，供下面两条测试用。
    fn state_with_b_under_trial() -> CommandState {
        let state = CommandState::new();
        // 两次失败 → 安全模式（BootCounter::SAFEMODE_THRESHOLD）。
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        cmd_recover_trial_enable(&state, "p.b").unwrap();
        assert_eq!(
            state.recovery.lock().plugin_state("p.b").map(|s| s.as_str()),
            Some("trial-enable"),
            "前置状态没搭起来，后面的断言就没有意义"
        );
        state
    }

    /// `host_recover_report`：插件不得替**别人**报 failure，也不得认领应用级 `None`。
    ///
    /// 靶子选「正处于 `TrialEnable` 的 `p.b`」：这是**唯一**会让 `pluginId` 真的
    /// 伤到别人的路径（`record_trial_failure_at` 烧掉它 1 次的试验预算 → 永久
    /// `disabled-by-safemode`）。选一个"没启动过的插件"当靶子会因为该路径不入账而
    /// 看不出差别。
    #[test]
    fn plugin_caller_cannot_report_failure_for_another_plugin() {
        let state = state_with_b_under_trial();
        let a = plugin_caller("p.a");

        // 前置：诊断上下文里此刻有两条（两次启动失败，`pluginId` 均为 `None`）。
        let ctx_before: Vec<Option<String>> = state
            .recovery
            .lock()
            .context()
            .iter()
            .map(|e| e.plugin_id.clone())
            .collect();
        assert_eq!(ctx_before.len(), 2);
        assert!(ctx_before.iter().all(|p| p.is_none()), "前置上下文不该已经指向 B");

        // 替 B 报 failure → 拒绝。
        let err = cmd_recover_report_as(&a, &state, "failure", Some("p.b")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        // **零副作用**：B 仍在试启、试验预算为 0、阶段与全局计数一动不动、
        // 诊断上下文里没有多出"B 崩了"的纪录。
        assert_eq!(
            state.recovery.lock().plugin_state("p.b").map(|s| s.as_str()),
            Some("trial-enable"),
            "拒绝路径不得消耗别人的试验预算"
        );
        assert_eq!(
            state.recovery.lock().counter().trial_failures.get("p.b").copied(),
            None,
            "别人的试验失败计数必须还是空的"
        );
        assert_eq!(state.recovery.lock().counter().consecutive_failures, 2);
        assert_eq!(state.recovery.lock().counter().safemode_failures, 0);
        assert_eq!(state.recovery.lock().decide_boot_phase().as_str(), "safemode");
        let ctx_after: Vec<Option<String>> = state
            .recovery
            .lock()
            .context()
            .iter()
            .map(|e| e.plugin_id.clone())
            .collect();
        assert_eq!(
            ctx_after, ctx_before,
            "拒绝路径不得把故障栽赃进诊断上下文（长度与归因都必须原样）"
        );
        assert!(
            !ctx_after.iter().any(|p| p.as_deref() == Some("p.b")),
            "上下文里不得出现被冒名的插件"
        );

        // 应用级结果（`None`）同样不归插件主张。
        let err = cmd_recover_report_as(&a, &state, "failure", None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(state.recovery.lock().counter().consecutive_failures, 2, "全局计数不得动");

        // 对照一：插件报**自己**照旧通过（self 档，不是"插件不能上报"）。
        let ok = cmd_recover_report_as(&a, &state, "failure", Some("p.a")).unwrap();
        assert_eq!(ok["outcome"], "failure");
        assert_eq!(ok["suspectedPlugin"], "p.a");

        // 对照二：主窗带任意 pluginId / `None` 照旧通过。
        let fresh = state_with_b_under_trial();
        let ok = cmd_recover_report_as(&Caller::MainWindow, &fresh, "failure", Some("p.b")).unwrap();
        assert_eq!(ok["suspectedPlugin"], "p.b");
        let ok = cmd_recover_report_as(&Caller::MainWindow, &fresh, "success", None).unwrap();
        assert_eq!(ok["engineAction"], "bootSuccess");
    }

    /// **危害实证**：判定缺席时，替别人报一次 `failure` 就能永久关掉它的试启用机会。
    ///
    /// 这条刻意调用**不过身份**的核心 `cmd_recover_report`，把"gate 挡住了什么"变成
    /// 可执行证据（而不是文档里的一句形容词）。它同时锁住"一旦有人把 `_as` 里的
    /// 判定摘掉，会失去什么"。
    #[test]
    fn recover_report_without_the_identity_check_burns_another_plugins_trial_budget() {
        let state = state_with_b_under_trial();
        cmd_recover_report(&state, "failure", Some("p.b")).unwrap();

        assert_eq!(
            state.recovery.lock().plugin_state("p.b").map(|s| s.as_str()),
            Some("disabled-by-safemode"),
            "未过身份的核心：B 被替报一次就回落（这就是 gate 要挡的伤害）"
        );
        assert_eq!(
            state.recovery.lock().counter().trial_failures.get("p.b").copied(),
            Some(1),
            "别人的试验预算被消耗"
        );
        // 不可逆：B 连合法试启都做不了了（TRIAL_MAX_ATTEMPTS = 1）。
        assert_eq!(
            cmd_recover_trial_enable_as(&Caller::MainWindow, &state, "p.b")
                .unwrap_err()
                .code,
            ErrorCode::E_PLUGIN_DISABLED
        );
    }

    /// `host_recover_trial_enable`：仅主窗，且拒绝路径不消耗任何试启预算。
    #[test]
    fn plugin_caller_cannot_trial_enable_a_plugin_but_main_window_can() {
        let state = state_with_b_under_trial();
        let state2 = CommandState::new();
        for _ in 0..2 {
            cmd_recover_report(&state2, "failure", None).unwrap();
        }

        // 插件（哪怕是"自己的" id）→ 拒绝，且引擎侧毫无动静。
        let err = cmd_recover_trial_enable_as(&plugin_caller("p.a"), &state2, "p.a").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state2.recovery.lock().plugin_state("p.a"),
            None,
            "拒绝路径不得把插件登记进恢复引擎"
        );
        assert_eq!(state2.recovery.lock().counter().trial_failures.get("p.a").copied(), None);

        // 对照：主窗试启同一个插件 → 真的进了 TrialEnable（拒绝不是命令被关死）。
        let r = cmd_recover_trial_enable_as(&Caller::MainWindow, &state2, "p.a").unwrap();
        assert_eq!(r["phase"], "safemode");
        assert_eq!(
            state2.recovery.lock().plugin_state("p.a").map(|s| s.as_str()),
            Some("trial-enable")
        );
        // 已经试启中的 state 上再让插件调一次，也必须被拒（不能"借"主窗的试启）。
        let err = cmd_recover_trial_enable_as(&plugin_caller("p.b"), &state, "p.b").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
    }

    /// `host_market_check/download/install`：仅主窗，且拒绝路径不写 `updateState`。
    #[test]
    fn plugin_caller_cannot_use_market_commands_but_main_window_can() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");
        // 哨兵值：拒绝路径不得覆盖它（"没写"与"写成别的东西"要能区分）。
        state.shell_ext.lock().update_state = Some("sentinel".to_string());

        let err = cmd_market_check_as(&a, &state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        let err = cmd_market_download_as(&a, &state, Some("2.0.0")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        let err = cmd_market_install_as(&a, &state, Some("2.0.0")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.shell_ext.lock().update_state.as_deref(),
            Some("sentinel"),
            "拒绝路径不得推进 updateState"
        );

        // 对照：主窗三条全通（返回线形不变，仍是模拟结果的诚实口径）。
        let check = cmd_market_check_as(&Caller::MainWindow, &state).unwrap();
        assert!(check.simulated && !check.available);
        let dl = cmd_market_download_as(&Caller::MainWindow, &state, Some("2.0.0")).unwrap();
        assert!(dl.ok && dl.simulated);
        assert_eq!(state.shell_ext.lock().update_state.as_deref(), Some("downloaded:2.0.0"));
        let ins = cmd_market_install_as(&Caller::MainWindow, &state, Some("2.0.0")).unwrap();
        assert!(ins.ok && ins.simulated);
        assert_eq!(state.shell_ext.lock().update_state.as_deref(), Some("installed:2.0.0"));
    }

    /// 畸形 label（`plugin-` 前缀但 id 非法）走既有拒绝路径，**不得**降级成主窗。
    ///
    /// 这是第二批所有判定共用的一道前提：`Caller::from_label` 只可能给出
    /// `MainWindow` / `Plugin`，畸形输入在解析处就被拒（`Caller` 类型刻意没有
    /// `Invalid` 变体）——所以下面的核心判定拿到的 caller 一定是可信主体。
    /// 验收标准 1（方案 §9-1）：harness 类宿主**只有底座**也要"功能完整"。
    ///
    /// 本用例只构造 [`SubstrateState`]——**不建** `PluginRuntimeState`、不碰注册表、
    /// 不碰 contributes/执行模型，逐域各打一次真实调用：shell（窗口 → Sink）、
    /// settings、i18n、notify、recovery、事件总线、brand。
    ///
    /// 为什么需要它：`cargo check` 只能证明"能编译"，不能证明"底座不是空壳"。
    /// 可编译形态的独立证据在 `examples/minimal-app` 的 `--features substrate-only`
    /// （那里连 `PluginRuntimeState` 都不 `manage`）。
    #[test]
    fn substrate_only_host_is_functionally_complete() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());
        let main = Caller::MainWindow;

        // ① shell：窗口命令经 `WindowSink`（缺省 `MemoryWindowSink` 留痕、不假装原生）。
        cmd_window_minimize(&state, "main").expect("shell 域：窗口命令必须可用");
        cmd_window_set_size(&state, "main", 800, 600).expect("shell 域：改尺寸必须可用");

        // ② settings：主窗读写（插件档的键空间判定另有专测）。
        cmd_settings_set_as(&main, &state, "app.theme", serde_json::json!("dark"))
            .expect("settings 域：主窗必须可写");
        assert_eq!(
            cmd_settings_get_as(&main, &state, "app.theme").unwrap(),
            serde_json::json!("dark"),
            "settings 域：写进去的要能原样读出来"
        );

        // ③ i18n：装载 → 切语言 → 取词。
        let mut entries = serde_json::Map::new();
        entries.insert("app.title".to_string(), serde_json::json!("标题"));
        cmd_i18n_load_as(&main, &state, "zh-CN", entries, None).expect("i18n 域：装载必须可用");
        cmd_i18n_set_locale_as(&main, &state, "zh-CN").expect("i18n 域：切语言必须可用");
        assert_eq!(
            cmd_i18n_t(&state, "app.title").unwrap(),
            "标题",
            "i18n 域：装载后必须真能取到该语言的词条"
        );

        // ④ notify：写入 → 读回（同一底座状态，不依赖插件运行时）。
        cmd_notify_as(&main, &state, "host", "标题", "正文").expect("notify 域：发送必须可用");
        let list = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(
            list.get("total").and_then(|v| v.as_u64()),
            Some(1),
            "notify 域：写入后必须能读回一条（{list}）"
        );

        // ⑤ recovery：启动载荷 + 成功上报（持久化目录为 None = 仅进程内，属有意降级）。
        let boot = cmd_recover_boot(&state).expect("recovery 域：启动载荷必须可用");
        assert!(
            boot.as_object().is_some_and(|o| !o.is_empty()),
            "recovery 域：启动载荷不能是空对象（{boot}）"
        );
        cmd_recover_report_as(&main, &state, "success", None).expect("recovery 域：上报必须可用");

        // ⑥ 事件总线（底座 IPC 面）：发布必须成功。
        cmd_events_publish(&state, "host", "app.ready", serde_json::json!({ "n": 1 }))
            .expect("ipc 域：发布必须可用");

        // ⑦ brand：形状必须是对象（桩返回 `{}` 而不是 `null`——后者会让前端取属性崩溃）。
        assert!(
            cmd_brand_info(&state).unwrap().is_object(),
            "brand 域：必须是对象而不是 null"
        );
    }

    #[test]
    fn malformed_labels_cannot_reach_the_new_identity_checks_as_main_window() {
        let state = CommandState::new();
        for bad in ["plugin-", "plugin-..", "plugin-a b"] {
            let err = Caller::from_label(bad).unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{bad}");
        }
        // 判定的第二个入口同样只认主窗：`require_self_plugin_scope` 对主窗放行任意
        // 署名、对插件只放行自己——两块判定不重叠，也不是"二选一"。
        assert!(require_self_plugin_scope(&Caller::MainWindow, "host_notify", Some("x")).is_ok());
        assert!(
            require_self_plugin_scope(&Caller::MainWindow, "host_notify", None).is_ok()
        );
        assert!(require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", Some("p.a")).is_ok());
        for claimed in [Some("p.b"), None] {
            assert_eq!(
                require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", claimed)
                    .unwrap_err()
                    .code,
                ErrorCode::E_AUTH_DENIED
            );
        }
        // 两个拒绝分支的消息必须说清"为什么"（诊断时能区分冒名与宿主级主张）。
        let impersonate =
            require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", Some("p.b"))
                .unwrap_err();
        assert!(impersonate.message.contains("p.b") && impersonate.message.contains("p.a"));
        let host_ns = require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", None)
            .unwrap_err();
        assert!(host_ns.message.contains("宿主级") || host_ns.message.contains("应用级"));
        let _ = state;
    }

    // ──────────────────────────────────────────────────────────────────────
    // 轮 11 第三批：通知读端（过滤）/ 通知写端 / 全局语言 / 应用级深链接
    //
    // 读端是**过滤**（插件必须仍能读自己的），写端与后两条是**判定**。
    // 每条都配：① 主窗对照 ② 越权零副作用 ③ "不过判定的内核会造成什么"的实证。
    // ──────────────────────────────────────────────────────────────────────

    /// 通知读端：插件只看得到署名是自己的条目，且 `total`/`unread` **同源**。
    #[test]
    fn plugin_caller_only_lists_its_own_notifications() {
        let state = CommandState::new();
        cmd_notify(&state, "p.a", "A1", "body-a1").unwrap();
        cmd_notify(&state, "p.b", "B1", "body-b1").unwrap();
        cmd_notify(&state, "p.a", "A2", "body-a2").unwrap();
        assert_eq!(state.notify_store.lock().len(), 3, "前置：三条真的都写进去了");

        let a = plugin_caller("p.a");
        let snap = cmd_notifications_list_as(&a, &state, None).unwrap();
        assert_eq!(snap["total"], 2, "total 必须是**可见**集合的真值（不是全局 3）");
        assert_eq!(snap["unread"], 2, "unread 同理——只裁数组会留下未读数泄露");
        let items = snap["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        for it in items {
            assert_eq!(it["pluginId"], "p.a", "不得出现别人的条目");
        }
        assert_eq!(items[0]["title"], "A2", "过滤之后仍按时间倒序");
        let wire = serde_json::to_string(&snap).unwrap();
        assert!(
            !wire.contains("body-b1") && !wire.contains("B1"),
            "别人的标题/正文一个字都不许出现在返回体里（含任何字段）"
        );

        // `limit` 作用在**过滤之后**：全局最新那条是**别人的**，插件仍能拿到自己最新那条。
        cmd_notify(&state, "p.b", "B2", "body-b2").unwrap();
        let page = cmd_notifications_list_as(&a, &state, Some(1)).unwrap();
        assert_eq!(
            page["items"].as_array().unwrap().len(),
            1,
            "先分页再过滤会让插件自己的通知被别人的挤掉（这里会变成 0 条）"
        );
        assert_eq!(page["items"][0]["title"], "A2");
        assert_eq!(page["total"], 2, "total 不受 limit 影响（可见集合总数）");
        assert_eq!(page["unread"], 2);

        // 对照：主窗照旧看全部（现状不变）。
        let all = cmd_notifications_list_as(&Caller::MainWindow, &state, None).unwrap();
        assert_eq!(all["total"], 4);
        assert_eq!(all["items"].as_array().unwrap().len(), 4);
        assert_eq!(cmd_notifications_list_as(&Caller::MainWindow, &state, Some(1)).unwrap()["items"][0]["title"], "B2");
    }

    /// **危害实证**：不过身份的内核把别人的通知正文（可能是敏感内容）整份交出去。
    #[test]
    fn notifications_list_without_scoping_leaks_other_plugins_content() {
        let state = CommandState::new();
        cmd_notify(&state, "p.b", "B 的故障详情", "reset-token=secret-b").unwrap();

        let leak = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(leak["total"], 1);
        assert!(
            serde_json::to_string(&leak).unwrap().contains("secret-b"),
            "未过滤时别人的通知正文（含 data 里的跳转载荷）完整可读——这就是 gate 要挡的东西"
        );

        // 同一个存储、同一个"插件 A"视角，走 gate 后什么都看不到。
        let scoped = cmd_notifications_list_as(&plugin_caller("p.a"), &state, None).unwrap();
        assert_eq!(scoped["total"], 0);
        assert!(!serde_json::to_string(&scoped).unwrap().contains("secret-b"));
    }

    /// 通知读端的 `dispatchLog` 同样按身份过滤（记录里只有 `entryId`，要反查归属）。
    #[test]
    fn plugin_caller_only_lists_its_own_dispatch_log() {
        let state = CommandState::new();
        let sink = AdapterMockSink::new(true, false);
        assert!(state.notify_sink.set(sink.clone()).is_ok());
        cmd_notify(&state, "p.a", "A", "a").unwrap();
        cmd_notify(&state, "p.b", "B", "b").unwrap();

        // 前置：主窗视角下日志确实有两条（否则下面的断言没有意义）。
        let all = cmd_notifications_list_as(&Caller::MainWindow, &state, None).unwrap();
        assert_eq!(all["dispatchLog"].as_array().unwrap().len(), 2);

        let scoped = cmd_notifications_list_as(&plugin_caller("p.a"), &state, None).unwrap();
        let log = scoped["dispatchLog"].as_array().unwrap();
        assert_eq!(log.len(), 1, "只留能归属到自己的分发记录");
        assert_eq!(
            log[0]["entryId"],
            scoped["items"][0]["id"],
            "留下的那条必须指向自己的条目"
        );
    }

    /// 通知写端：插件只能标记自己的通知；`None`（全部已读）与未知 id 一律拒绝。
    #[test]
    fn plugin_caller_can_only_mark_its_own_notifications_read() {
        let state = CommandState::new();
        cmd_notify(&state, "p.a", "A", "a").unwrap();
        cmd_notify(&state, "p.b", "B", "b").unwrap();
        let (a_id, b_id) = {
            let store = state.notify_store.lock();
            let ids: Vec<(String, String)> = store
                .recent(store.len())
                .iter()
                .map(|e| (e.plugin_id.clone(), e.id.clone()))
                .collect();
            let pick = |who: &str| ids.iter().find(|(p, _)| p == who).unwrap().1.clone();
            (pick("p.a"), pick("p.b"))
        };
        let a = plugin_caller("p.a");

        // 标记**别人的** → 拒绝，且 B 仍是未读、全局未读计数不动。
        let err = cmd_notifications_read_as(&a, &state, Some(&b_id)).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(
            !state.notify_store.lock().get(&b_id).unwrap().read,
            "别人的已读状态不得被篡改（那等于替别人把消息吞掉）"
        );
        assert_eq!(state.notify_store.lock().unread_count(), 2);

        // 未知 id → 拒绝：放行会让 `marked` 变成"这个 id 存不存在"的预言机。
        let err = cmd_notifications_read_as(&a, &state, Some("nope")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(state.notify_store.lock().unread_count(), 2);

        // `None` = 标记全部（全局状态）→ 拒绝，且一条都不许被清。
        let err = cmd_notifications_read_as(&a, &state, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.notify_store.lock().unread_count(),
            2,
            "拒绝路径不得清全局未读（否则宿主的角标会静默归零）"
        );

        // 标记**自己的** → 生效。
        let r = cmd_notifications_read_as(&a, &state, Some(&a_id)).unwrap();
        assert_eq!(r["marked"], 1);
        assert_eq!(state.notify_store.lock().unread_count(), 1);

        // 对照：主窗任意 id / `None` 照旧；未知 id 仍是 `marked: 0`（既有行为不变）。
        cmd_notifications_read_as(&Caller::MainWindow, &state, Some(&b_id)).unwrap();
        assert!(state.notify_store.lock().get(&b_id).unwrap().read);
        assert_eq!(
            cmd_notifications_read_as(&Caller::MainWindow, &state, None).unwrap()["marked"],
            0
        );
        assert_eq!(
            cmd_notifications_read_as(&Caller::MainWindow, &state, Some("nope")).unwrap()["marked"],
            0
        );
    }

    /// **危害实证**：不过身份的内核一次调用就能清掉**所有人**的未读。
    #[test]
    fn notifications_read_without_the_identity_check_can_wipe_the_unread_state_of_others() {
        let state = CommandState::new();
        cmd_notify(&state, "host", "宿主重要告警", "磁盘快满了").unwrap();
        cmd_notify(&state, "p.b", "B", "b").unwrap();
        assert_eq!(state.notify_store.lock().unread_count(), 2);

        // 不过身份的核心：`None` = 全部已读。
        assert_eq!(cmd_notifications_read(&state, None).unwrap()["marked"], 2);
        assert_eq!(
            state.notify_store.lock().unread_count(),
            0,
            "未过身份的核心：一次调用把宿主与所有插件的未读清零（用户再也不会被提醒）"
        );
        // 单条路径同理：替别人标记已读。
        let state2 = CommandState::new();
        cmd_notify(&state2, "p.b", "B", "b").unwrap();
        let b_id = state2.notify_store.lock().recent(1)[0].id.clone();
        assert_eq!(
            cmd_notifications_read(&state2, Some(&b_id)).unwrap()["marked"],
            1
        );
        assert!(state2.notify_store.lock().get(&b_id).unwrap().read);
    }

    /// `host_i18n_set_locale`：仅主窗（切的是全局语言：宿主 UI + 所有插件）。
    #[test]
    fn plugin_caller_cannot_change_the_global_locale_but_main_window_can() {
        let state = CommandState::new();
        let before = cmd_i18n_stats(&state).unwrap();
        let current = before["locale"].as_str().unwrap().to_string();
        let other = if current == "ja-JP" { "ko-KR" } else { "ja-JP" };

        let err = cmd_i18n_set_locale_as(&plugin_caller("p.a"), &state, other).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            cmd_i18n_stats(&state).unwrap(),
            before,
            "拒绝路径不得改语言（引擎状态与已注册语言包逐字不变）"
        );
        // 也确认没有"悄悄改了一半"：locale 与回退链都还是原值。
        assert_eq!(cmd_i18n_stats(&state).unwrap()["locale"], current.as_str());
        assert_eq!(state.i18n.lock().locale().as_str(), current.as_str());

        // 对照：主窗切换生效。
        let out = cmd_i18n_set_locale_as(&Caller::MainWindow, &state, other).unwrap();
        assert_eq!(out["locale"], other);
        assert_eq!(state.i18n.lock().locale().as_str(), other);
    }

    /// **危害实证**：不过身份的内核让一个插件改掉**所有人**界面的语言。
    #[test]
    fn i18n_set_locale_without_the_identity_check_changes_the_language_for_everyone() {
        let state = CommandState::new();
        let before = cmd_i18n_stats(&state).unwrap()["locale"].as_str().unwrap().to_string();
        let other = if before == "ja-JP" { "ko-KR" } else { "ja-JP" };

        cmd_i18n_set_locale(&state, other).unwrap();
        assert_eq!(state.i18n.lock().locale().as_str(), other);
        assert_ne!(before, other);
        // 受影响的是**同一个引擎**：插件自己与宿主的每一次 host_i18n_t 都改了口径，
        // 也就是"用户明明在中文环境里看到日文界面，而根因在另一个插件的一次调用里"。
        assert_eq!(
            cmd_i18n_stats(&state).unwrap()["locale"],
            other,
            "全局语言被改：宿主 UI 与所有插件的界面同时换语种"
        );
    }

    /// `host_deep_link_register`：仅主窗（注册的是**应用级**协议，且会注销上一个协议）。
    #[test]
    fn plugin_caller_cannot_register_an_app_level_deep_link_but_main_window_can() {
        let state = CommandState::new();
        // 正当路径：主窗先注册 `tauron`。
        cmd_deep_link_register_as(&Caller::MainWindow, &state, "tauron".to_string()).unwrap();
        assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("tauron"));

        // 插件接管 → 拒绝，且应用级协议**逐字不动**（也没有触发注销旧值）。
        let err = cmd_deep_link_register_as(&plugin_caller("p.a"), &state, "evil".to_string())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.shell_ext.lock().deep_link_protocol.as_deref(),
            Some("tauron"),
            "拒绝路径不得改应用级协议（也不得把上一个协议从 OS 关联里摘掉）"
        );

        // 对照：主窗换协议生效。
        cmd_deep_link_register_as(&Caller::MainWindow, &state, "other".to_string()).unwrap();
        assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("other"));
    }

    /// **危害实证**：不过身份的内核让插件把整个应用的深链接入口改到自己名下。
    #[test]
    fn deep_link_register_without_the_identity_check_lets_a_plugin_take_over_the_protocol() {
        let state = CommandState::new();
        cmd_deep_link_register_as(&Caller::MainWindow, &state, "tauron".to_string()).unwrap();
        // 不过身份的核心（插件能直接调到的就是它）。
        cmd_deep_link_register(&state, "evil".to_string()).unwrap();
        assert_eq!(
            state.shell_ext.lock().deep_link_protocol.as_deref(),
            Some("evil"),
            "未过身份的核心：应用认的协议被改成插件给的值（用户的 tauron:// 链接不再拉起本应用）"
        );
    }

    #[test]
    fn cmd_market_check_returns_consumable_stub() {
        let state = CommandState::new();
        let result = cmd_market_check(&state);
        assert!(result.is_ok());
        let check = result.unwrap();
        // 必须是 TS 可直接消费的形状：Null 会让前端 result.available 崩溃。
        assert!(!check.available);
        // R8：**模拟**这件事必须是线字段，而不是只写在注释里。
        assert!(check.simulated, "本地桩结果必须如实标 simulated");
        assert!(
            check.reason.as_deref().is_some_and(|r| r.contains("未接入更新源")),
            "必须写明为什么不可用：{:?}",
            check.reason
        );
        assert_eq!(
            serde_json::to_value(&check).unwrap(),
            serde_json::json!({
                "available": false,
                "simulated": true,
                "version": null,
                "reason": check.reason.clone(),
            }),
            "线形字段名/数量必须与 TS 同步（camelCase、无额外字段）"
        );
    }

    #[test]
    fn cmd_brand_info_returns_object_stub() {
        let state = CommandState::new();
        let result = cmd_brand_info(&state);
        assert!(result.is_ok());
        // 空对象（BrandInfo 全字段可选），不得是 Null。
        assert_eq!(result.unwrap(), serde_json::json!({}));
    }

    fn find_summary(
        state: &PluginRuntimeState,
        plugin_id: &str,
    ) -> Option<PluginSummary> {
        state.registry.list_all().into_iter().find(|p| p.id == plugin_id)
    }

    fn entries(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), serde_json::json!(v));
        }
        m
    }

    #[test]
    fn cmd_i18n_t_missing_key_returns_key_and_counts_miss() {
        let state = CommandState::new();
        // 全部缺失时返回 key 本身（**不是空串**）：让用户看到 `oc.x` 比看到空
        // 按钮更能暴露缺失文案，空串会让问题彻底隐形。
        assert_eq!(
            cmd_i18n_t(&state, "oc.settings.title").unwrap(),
            "oc.settings.title"
        );
        assert_eq!(cmd_i18n_stats(&state).unwrap()["missingTotal"], 1);
    }

    #[test]
    fn cmd_i18n_t_rejects_empty_key() {
        let state = CommandState::new();
        assert_eq!(
            cmd_i18n_t(&state, "   ").unwrap_err().code,
            ErrorCode::E_INVALID_MANIFEST
        );
    }

    #[test]
    fn cmd_i18n_load_set_locale_and_translate() {
        let state = CommandState::new();
        cmd_i18n_load(
            &state,
            "zh-CN",
            entries(&[("oc.hello", "你好"), ("oc.by.name", "你好，{{name}}")]),
            None,
        )
        .unwrap();

        let switched = cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        assert_eq!(switched["locale"], "zh-CN");
        assert_eq!(switched["rtl"], serde_json::json!(false));
        assert_eq!(
            switched["fallbackChain"],
            serde_json::json!(["zh-CN", "zh", "en-US"])
        );

        assert_eq!(cmd_i18n_t(&state, "oc.hello").unwrap(), "你好");
        assert_eq!(
            cmd_i18n_t_params(&state, "oc.by.name", entries(&[("name", "世界")])).unwrap(),
            "你好，世界"
        );
    }

    #[test]
    fn cmd_i18n_t_falls_back_through_chain_without_counting_miss() {
        let state = CommandState::new();
        cmd_i18n_load(&state, "en-US", entries(&[("oc.menu.file", "File")]), None).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        // zh-CN 缺 → 回退链（zh → en-US）命中。回退链命中不算缺失：它成功翻译了，
        // 缺失计数只统计回退链耗尽的情况（那才是需要补文案的）。
        assert_eq!(cmd_i18n_t(&state, "oc.menu.file").unwrap(), "File");
        assert_eq!(cmd_i18n_stats(&state).unwrap()["missingTotal"], 0);

        // 回退链耗尽才计数。
        assert_eq!(cmd_i18n_t(&state, "oc.nowhere").unwrap(), "oc.nowhere");
        assert_eq!(cmd_i18n_stats(&state).unwrap()["missingTotal"], 1);
    }

    #[test]
    fn cmd_i18n_load_merges_instead_of_replacing() {
        let state = CommandState::new();
        cmd_i18n_load(&state, "zh-CN", entries(&[("oc.a", "a1")]), None).unwrap();
        let result = cmd_i18n_load(&state, "zh-CN", entries(&[("oc.b", "b1")]), None).unwrap();
        assert_eq!(result["loadedKeys"], 1);
        assert_eq!(result["newKeys"], 1);
        // 合并而不是替换：两次装载的 key 都必须还在。
        assert_eq!(result["bundleKeys"]["zh-CN"], 2);
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        assert_eq!(cmd_i18n_t(&state, "oc.a").unwrap(), "a1");
        assert_eq!(cmd_i18n_t(&state, "oc.b").unwrap(), "b1");
    }

    #[test]
    fn cmd_i18n_load_namespaces_plugin_keys_and_cleanup_removes_only_prefix() {
        let state = CommandState::new();

        let loaded = cmd_i18n_load(
            &state,
            "zh-CN",
            entries(&[("title", "我的插件"), ("plugin:p.my-plugin.oc.owned", "已带前缀")]),
            Some("p.my-plugin"),
        )
        .unwrap();
        assert_eq!(loaded["pluginId"], "p.my-plugin");
        assert_eq!(loaded["loadedKeys"], 2);
        assert_eq!(loaded["newKeys"], 2);

        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        // 带前缀的 key 可翻译；未带前缀的原始 key 不存在（不会双写）。
        assert_eq!(cmd_i18n_t(&state, "plugin:p.my-plugin.oc.title").unwrap(), "我的插件");
        assert_eq!(
            cmd_i18n_t(&state, "plugin:p.my-plugin.oc.owned").unwrap(),
            "已带前缀"
        );
        assert_eq!(cmd_i18n_t(&state, "title").unwrap(), "title");
        // 已经是完整前缀的 key 原样保留，不会被二次前缀。
        assert_eq!(loaded["bundleKeys"]["zh-CN"], serde_json::json!(2));

        // 另一个插件的文案必须不受影响。
        cmd_i18n_load(&state, "zh-CN", entries(&[("x", "X")]), Some("p.other")).unwrap();

        let cleaned = cmd_i18n_cleanup_plugin(&state, "p.my-plugin").unwrap();
        assert_eq!(cleaned["pluginId"], "p.my-plugin");
        assert_eq!(cleaned["removed"], 2);
        assert_eq!(cmd_i18n_t(&state, "plugin:p.other.oc.x").unwrap(), "X");
        assert_eq!(
            cmd_i18n_t(&state, "plugin:p.my-plugin.oc.title").unwrap(),
            "plugin:p.my-plugin.oc.title"
        );
    }

    #[test]
    fn cmd_i18n_set_locale_rejects_invalid_locale() {
        let state = CommandState::new();
        // 空串与超长串都会污染回退链，必须在适配层拦下（引擎自身不校验）。
        for bad in ["", &"a".repeat(36)] {
            assert_eq!(
                cmd_i18n_set_locale(&state, bad).unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );
        }
        assert!(cmd_i18n_set_locale(&state, "he-IL").is_ok());
        // RTL 语言必须反映在状态里（前端布局要依赖它）。
        assert_eq!(cmd_i18n_stats(&state).unwrap()["rtl"], serde_json::json!(true));
    }

    #[test]
    fn cmd_i18n_load_rejects_non_string_value_and_bad_plugin_id() {
        let state = CommandState::new();
        let mut bad = serde_json::Map::new();
        bad.insert("oc.x".to_string(), serde_json::json!(42));
        assert_eq!(
            cmd_i18n_load(&state, "zh-CN", bad, None).unwrap_err().code,
            ErrorCode::E_INVALID_MANIFEST
        );
        // 插件 id 走同一个校验规则（反域名格式）。
        assert_eq!(
            cmd_i18n_load(&state, "zh-CN", entries(&[("a", "b")]), Some("not-valid")).unwrap_err().code,
            ErrorCode::E_INVALID_MANIFEST
        );
    }

    #[test]
    fn cmd_i18n_stats_reports_consumable_shape() {
        let state = CommandState::new();
        cmd_i18n_load(&state, "zh-CN", entries(&[("oc.a", "a")]), None).unwrap();
        cmd_i18n_t(&state, "oc.missing").unwrap();

        let stats = cmd_i18n_stats(&state).unwrap();
        // 线名必须是 camelCase，且字段齐全（前端按此渲染）。
        for field in [
            "locale", "rtl", "fallbackChain", "registeredLocales", "bundleKeys", "missingTotal",
        ] {
            assert!(stats.get(field).is_some(), "缺少字段 {field}");
        }
        assert_eq!(stats["locale"], "en-US");
        assert!(stats["registeredLocales"].as_array().unwrap().contains(&serde_json::json!("zh-CN")));
        assert_eq!(stats["bundleKeys"]["zh-CN"], 1);
        assert_eq!(stats["missingTotal"], 1);
    }

    #[test]
    fn cmd_settings_set_get_roundtrip() {
        let state = CommandState::new();
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("dark")).unwrap();
        let result = cmd_settings_get(&state, "plugin:p.theme").unwrap();
        assert_eq!(result, serde_json::json!("dark"));
    }

    // ────────────────────────────────────────────────────────────
    // R7-2：settings 走 tauron-settings 的 Store
    // ────────────────────────────────────────────────────────────

    #[test]
    fn settings_are_backed_by_a_store_with_a_registered_schema() {
        // 裸 HashMap 过不了这三条：没有注册表、没有数据版本、没有迁移步。
        let state = CommandState::new();
        cmd_settings_set(&state, "k", serde_json::json!(1)).unwrap();
        let store = state.settings.lock();
        let entry = store
            .registry()
            .get(HOST_SETTINGS_NAMESPACE)
            .expect("宿主设置命名空间必须已注册 schema");
        assert_eq!(entry.schema_version, HOST_SETTINGS_SCHEMA_V2, "当前版本是 v2");
        assert_eq!(store.migration_count(HOST_SETTINGS_NAMESPACE), 1, "v1→v2 迁移已注册");
        assert_eq!(
            store.data_version(HOST_SETTINGS_NAMESPACE),
            Some(HOST_SETTINGS_SCHEMA_V2),
            "写入必须标注数据版本"
        );
    }

    #[test]
    fn settings_keys_are_single_segment_paths_so_flat_semantics_hold() {
        // 点路径不做转义的话：`a` 先写成标量，再写 `a.b` 会撞上
        // `merge::write_path` 的中间节点断言（panic → E_HOST_PANIC），
        // 而且 `get("a")` 会从标量变成对象。转义后这两条都成立。
        let state = CommandState::new();
        cmd_settings_set(&state, "a", serde_json::json!(1)).unwrap();
        cmd_settings_set(&state, "a.b", serde_json::json!(2)).unwrap();
        assert_eq!(cmd_settings_get(&state, "a").unwrap(), serde_json::json!(1));
        assert_eq!(cmd_settings_get(&state, "a.b").unwrap(), serde_json::json!(2));
        // 编码是注入式的：字面键 `a%2Eb` 与 `a.b` 不碰撞。
        cmd_settings_set(&state, "a%2Eb", serde_json::json!(3)).unwrap();
        assert_eq!(cmd_settings_get(&state, "a.b").unwrap(), serde_json::json!(2));
        assert_eq!(cmd_settings_get(&state, "a%2Eb").unwrap(), serde_json::json!(3));
    }

    #[test]
    fn settings_path_encoding_is_injective() {
        assert_eq!(settings_path("plugin:p.theme"), "plugin:p%2Etheme");
        assert_eq!(settings_path("%"), "%25");
        assert_eq!(settings_path("$unset"), "%24unset");
        assert_ne!(settings_path("a.b"), settings_path("a%2Eb"));
        assert_eq!(settings_path("plain"), "plain");
    }

    #[test]
    fn settings_set_rejects_empty_key() {
        let state = CommandState::new();
        let e = cmd_settings_set(&state, "  ", serde_json::json!(1)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn settings_bridge_migrates_v1_documents_to_v2() {
        let state = CommandState::new();
        // 旧版宿主配置：裸键平铺（R7 之前的裸 HashMap 形态）。
        host_settings_adopt_legacy(
            &state,
            serde_json::json!({"plugin:p.theme": "dark", "other.key": 7}),
        )
        .unwrap();
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V1)
        );

        // v1 的裸键在 v2 编码下**读不到**——这正是迁移必须存在的原因，
        // 而不是「迁不迁都一样」。
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::Value::Null,
            "未迁移前不该假装读得出来"
        );

        let steps = host_settings_migrate(&state).unwrap();
        assert_eq!(steps, 1, "v1 → v2 恰好一步");
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V2)
        );
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::json!("dark")
        );
        assert_eq!(cmd_settings_get(&state, "other.key").unwrap(), serde_json::json!(7));

        // 幂等：再迁一次是 0 步。
        assert_eq!(host_settings_migrate(&state).unwrap(), 0);
    }

    #[test]
    fn settings_migration_commands_round_trip_through_the_command_layer() {
        // 本用例刻意走**命令层**入口（`cmd_*`，即 `host_settings_adopt_legacy` /
        // `host_settings_migrate` 两个 Tauri 包装器转调的那两个函数），而不是底层的
        // `host_settings_*` pub fn——只测底层等于没验证「宿主有渠道调得到」。
        let state = CommandState::new();
        // 旧版文档：裸键平铺（R7 之前的裸 HashMap 形态），含一个带 `.` 的键。
        cmd_settings_adopt_legacy(
            &state,
            serde_json::json!({"plugin:p.theme": "dark", "lang": "zh-CN"}),
        )
        .unwrap();
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V1),
            "接手旧文档必须标注起点版本，否则 migrate 只能拒绝"
        );
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::Value::Null,
            "v1 裸键在 v2 编码下读不到——迁移不是可选项"
        );

        // 迁移：v1 → v2 恰好一步。
        assert_eq!(cmd_settings_migrate(&state).unwrap(), 1, "v1 → v2 恰好一步");
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V2)
        );
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::json!("dark"),
            "带 `.` 的键迁移后必须经命令层读得出来"
        );
        assert_eq!(
            cmd_settings_get(&state, "lang").unwrap(),
            serde_json::json!("zh-CN")
        );

        // 幂等：再迁一次是 0 步，数据不动。
        assert_eq!(cmd_settings_migrate(&state).unwrap(), 0);
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::json!("dark")
        );
    }

    #[test]
    fn settings_adopt_legacy_command_rejects_non_object_documents() {
        let state = CommandState::new();
        let e = cmd_settings_adopt_legacy(&state, serde_json::json!(["a", "b"])).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        // 被拒**不留痕**：既没写用户层，也没标版本。留下一个「有数据无版本」的
        // 半态会让后续 migrate 永远拒绝，比不写更糟。
        assert_eq!(host_settings_data_version(&state), None, "被拒的文档不得留版本标注");
        assert_eq!(cmd_settings_get(&state, "a").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn settings_migration_command_refuses_unlabelled_existing_data() {
        // 命令层的错误口径必须与底层一致（不能把拒绝悄悄吞成 0 步）。
        let state = CommandState::new();
        state.settings.lock().set_layer(
            HOST_SETTINGS_NAMESPACE,
            tauron_settings::LayerKind::User,
            serde_json::json!({"k": 1}),
        );
        let e = cmd_settings_migrate(&state).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("未标注"), "{}", e.message);
    }

    #[test]
    fn settings_adopt_legacy_rejects_non_object_documents() {
        let state = CommandState::new();
        let e = host_settings_adopt_legacy(&state, serde_json::json!("not-an-object")).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn settings_migration_refuses_unlabelled_existing_data() {
        let state = CommandState::new();
        // 直接塞用户层（模拟磁盘上的数据）却不标版本 → 不猜起点，拒绝迁移。
        state.settings.lock().set_layer(
            HOST_SETTINGS_NAMESPACE,
            tauron_settings::LayerKind::User,
            serde_json::json!({"k": 1}),
        );
        let e = host_settings_migrate(&state).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("未标注"), "{}", e.message);
    }

    #[test]
    fn settings_new_store_has_no_data_version_before_any_write() {
        let state = CommandState::new();
        assert_eq!(host_settings_data_version(&state), None);
    }

    // ────────────────────────────────────────────────────────────
    // R7-3：notify 接 DispatchSink
    // ────────────────────────────────────────────────────────────

    /// 记录型 mock sink：证明 `host_notify` **真的**调了 dispatch。
    struct AdapterMockSink {
        supported: bool,
        fail: bool,
        calls: parking_lot::Mutex<Vec<String>>,
    }

    impl AdapterMockSink {
        fn new(supported: bool, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                supported,
                fail,
                calls: parking_lot::Mutex::new(Vec::new()),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.lock().len()
        }
    }

    impl tauron_notify::DispatchSink for AdapterMockSink {
        fn send(&self, entry: &NotifyEntry) -> Result<bool, String> {
            self.calls.lock().push(entry.id.clone());
            if self.fail {
                return Err("system 通道炸了".into());
            }
            Ok(self.supported)
        }
    }

    #[test]
    fn cmd_notify_dispatches_through_the_injected_sink() {
        let state = CommandState::new();
        let sink = AdapterMockSink::new(true, false);
        assert!(state.notify_sink.set(sink.clone()).is_ok());

        cmd_notify(&state, "p.audio", "T", "B").unwrap();

        assert_eq!(sink.call_count(), 1, "必须真的调了 sink.send");
        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 1, "通知必须入库");
        let log = snap["dispatchLog"].as_array().unwrap();
        assert_eq!(log.len(), 1, "dispatchLog 必须有记录");
        assert_eq!(log[0]["outcome"], "system");
        assert_eq!(
            log[0]["entryId"],
            snap["items"][0]["id"],
            "日志指的就是这条通知"
        );
        assert_eq!(sink.calls.lock()[0], snap["items"][0]["id"].as_str().unwrap());
    }

    #[test]
    fn cmd_notify_degrades_without_losing_the_notification() {
        let state = CommandState::new();
        let sink = AdapterMockSink::new(true, true); // 致命错误
        assert!(state.notify_sink.set(sink.clone()).is_ok());

        // 关键：系统通知失败时 `host_notify` **不返回错误**。
        cmd_notify(&state, "p.audio", "T", "B").expect("系统通知失败不得让 host_notify 报错");

        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 1, "降级后通知仍须在环形缓冲里");
        assert_eq!(snap["unread"], 1);
        let log = snap["dispatchLog"].as_array().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0]["outcome"], "degraded");
        assert_eq!(sink.call_count(), 1, "降级不重试");
    }

    #[test]
    fn cmd_notify_without_a_sink_logs_nothing() {
        // 没有系统通道 = 没有 dispatch 尝试 = 没有日志（不伪造一条 degraded）。
        let state = CommandState::new();
        assert!(state.notify_sink.get().is_none());
        cmd_notify(&state, "p.audio", "T", "B").unwrap();
        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 1);
        assert!(snap["dispatchLog"].as_array().unwrap().is_empty());
    }

    #[test]
    fn notifications_list_keeps_existing_fields_and_adds_dispatch_log() {
        // 既有字段不得改名/删除：前端已消费它们。
        let state = CommandState::new();
        cmd_notify(&state, "p.audio", "T", "B").unwrap();
        let snap = cmd_notifications_list(&state, None).unwrap();
        for field in ["unread", "total", "items"] {
            assert!(snap.get(field).is_some(), "缺少既有字段 {field}");
        }
        assert!(snap.get("dispatchLog").is_some(), "R7-3 新增 dispatchLog");
        for field in ["id", "pluginId", "kind", "title", "message", "ts", "read", "data"] {
            assert!(snap["items"][0].get(field).is_some(), "items 缺字段 {field}");
        }
    }

    #[test]
    fn cmd_contributes_register_and_list() {
        let state = CommandState::new();
        let entry = ContributeEntry {
            plugin_id: "p.example".to_string(),
            kind: "command".to_string(),
            id: "hello".to_string(),
            label: "Hello Command".to_string(),
        };
        cmd_contributes_register(&state, "p.example", entry.clone()).unwrap();
        let list = cmd_contributes_list(&state, None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "hello");
    }

    #[test]
    fn cmd_contributes_register_duplicate_rejected() {
        let state = CommandState::new();
        let entry = ContributeEntry {
            plugin_id: "p.example".to_string(),
            kind: "command".to_string(),
            id: "hello".to_string(),
            label: "Hello".to_string(),
        };
        cmd_contributes_register(&state, "p.example", entry.clone()).unwrap();
        let result = cmd_contributes_register(&state, "p.example", entry);
        assert!(result.is_err());
    }

    #[test]
    fn cmd_contributes_list_by_kind() {
        let state = CommandState::new();
        cmd_contributes_register(&state, "p1", ContributeEntry {
            plugin_id: "p1".to_string(),
            kind: "command".to_string(),
            id: "cmd1".to_string(),
            label: "Cmd 1".to_string(),
        }).unwrap();
        cmd_contributes_register(&state, "p2", ContributeEntry {
            plugin_id: "p2".to_string(),
            kind: "panel".to_string(),
            id: "panel1".to_string(),
            label: "Panel 1".to_string(),
        }).unwrap();
        let commands = cmd_contributes_list(&state, Some("command")).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "cmd1");
    }

    #[test]
    fn cmd_notify_records_notification() {
        let state = CommandState::new();
        cmd_notify(&state, "p.audio", "Title", "Body").unwrap();
        let notifications = state.notifications.lock();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].plugin_id, "p.audio");
        assert_eq!(notifications[0].title, "Title");
    }

    #[test]
    fn notify_legacy_log_is_bounded_and_keeps_newest() {
        // 兼容日志只写不读：必须不能随通知次数无限增长。
        let state = CommandState::new();
        for i in 0..(MAX_NOTIFICATION_LOG + 5) {
            cmd_notify(&state, "p.audio", &format!("T{i}"), "Body").unwrap();
        }
        let notifications = state.notifications.lock();
        assert_eq!(notifications.len(), MAX_NOTIFICATION_LOG, "日志应被裁剪到上限");
        // 保留的是**最新**的一批（丢掉最早 5 条）。
        assert_eq!(notifications[0].title, "T5");
        assert_eq!(
            notifications[MAX_NOTIFICATION_LOG - 1].title,
            format!("T{}", MAX_NOTIFICATION_LOG + 4)
        );
    }

    #[test]
    fn notify_record_serializes_as_camel_case() {
        // 潜在线上形态必须与 TS `NotificationRecord`（pluginId…）一致：
        // 该结构当前无线上读取方，但字段名不一致会让未来的接线静默读到 undefined。
        let rec = NotificationRecord {
            plugin_id: "p.audio".to_string(),
            title: "T".to_string(),
            body: "B".to_string(),
            timestamp: 7,
        };
        let v = serde_json::to_value(&rec).unwrap();
        assert!(v.get("pluginId").is_some(), "应为 camelCase：{v}");
        assert!(v.get("plugin_id").is_none(), "不应保留蛇形键：{v}");
    }

    #[test]
    fn guard_catches_panic() {
        // 验证所有命令入口都经过 guard()。
        // 模拟一个会导致 panic 的调用（registry 内部 panic 应被捕获）。
        // 这里用 lifecycle_report 测试：对一个不存在的插件，
        // registry 应返回 E_UNKNOWN_PLUGIN 而非 panic。
        let state = CommandState::new();
        let result = cmd_lifecycle_report(
            &state,
            "plugin-test.nonexistent",
            None,
            Event::Enable,
        );
        assert!(result.is_err());
        // 错误码不应是 E_HOST_PANIC（因为逻辑正确处理了未知插件）
        assert_ne!(result.unwrap_err().code, ErrorCode::E_HOST_PANIC);
    }

    #[test]
    fn command_state_default() {
        let state = CommandState::default();
        assert!(cmd_registry_list_all(&state).is_ok());
    }

    #[test]
    fn concurrent_registry_access() {
        let state = Arc::new(CommandState::new());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let state = Arc::clone(&state);
            handles.push(std::thread::spawn(move || {
                cmd_registry_list_all(&state).is_ok()
            }));
        }

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|&r| r), "所有并发读取应成功");
    }

    #[test]
    fn concurrent_bus_access() {
        let state = Arc::new(CommandState::new());
        let mut handles = Vec::new();

        // 先声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p", &decls).unwrap();
        }

        for i in 0..10 {
            let state = Arc::clone(&state);
            handles.push(std::thread::spawn(move || {
                cmd_events_publish(
                    &state,
                    "p",
                    "plugin:p.ready",
                    serde_json::json!({"seq": i}),
                ).is_ok()
            }));
        }

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|&r| r), "所有并发发布应成功");
    }

    #[test]
    fn concurrent_subscribe_publish() {
        let state = Arc::new(CommandState::new());

        // 声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl {
                topic: "plugin:p.ready".to_string(),
                public: true,
            }];
            bus.declare_topics("p", &decls).unwrap();
        }

        let mut handles = Vec::new();
        for i in 0..5 {
            let state = Arc::clone(&state);
            let id = i;
            handles.push(std::thread::spawn(move || {
                let _sub = cmd_events_subscribe(&state, &format!("c{id}"), "w1", "plugin:p.ready");
                cmd_events_publish(
                    &state,
                    "p",
                    "plugin:p.ready",
                    serde_json::json!({"seq": id}),
                )
            }));
        }

        let results: Vec<Result<_, _>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|r| r.is_ok()), "所有并发操作应成功");
    }

    #[test]
    fn registry_install_and_list() {
        let state = CommandState::new();
        // 安装一个插件
        let manifest = test_manifest("p.test");
        let index = empty_index();
        let id = state.registry.install(&index, manifest).unwrap();
        // 列表应包含该插件
        let list = cmd_registry_list_all(&state).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id.as_str());
    }

    #[test]
    fn registry_install_duplicate_rejected() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.dup");
        state.registry.install(&index, manifest.clone()).unwrap();
        let result = state.registry.install(&index, manifest);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_PLUGIN_EXISTS);
    }

    #[test]
    fn registry_admin_enable_disable() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.admin");
        state.registry.install(&index, manifest).unwrap();

        // 启用
        let enable = cmd_registry_admin(&state, "p.admin", RegistryAdminOp::Enable);
        assert!(enable.is_ok());

        // 禁用
        let disable = cmd_registry_admin(&state, "p.admin", RegistryAdminOp::Disable);
        assert!(disable.is_ok());
    }

    #[test]
    fn registry_install_then_lifecycle() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.lifecycle");
        state.registry.install(&index, manifest).unwrap();

        // 启用
        let enable = cmd_registry_admin(&state, "p.lifecycle", RegistryAdminOp::Enable);
        assert!(enable.is_ok());
        let outcome = enable.unwrap();
        assert_eq!(outcome.to, tauron_host::lifecycle::State::Enabled);

        // 通过 lifecycle_report 上报 Attach
        let attach = cmd_lifecycle_report(
            &state,
            "plugin-p.lifecycle",
            None,
            Event::Attach,
        );
        assert!(attach.is_ok());
        let outcome = attach.unwrap();
        assert_eq!(outcome.to, tauron_host::lifecycle::State::Running);
    }

    #[test]
    fn registry_call_begin_and_end() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.call");
        state.registry.install(&index, manifest).unwrap();
        cmd_registry_admin(&state, "p.call", RegistryAdminOp::Enable).unwrap();
        cmd_lifecycle_report(&state, "plugin-p.call", None, Event::Attach).unwrap();

        // 发起调用
        let call = cmd_plugin_call(
            &state,
            "plugin-p.call",
            None,
            "test_method",
            serde_json::json!({"x": 1}),
        );
        assert!(call.is_ok());
        let pending = call.unwrap();
        assert!(!pending.call_id.is_empty());
        assert_eq!(pending.seq, 1);

        // 结束调用
        let end = cmd_call_end(&state, &pending.call_id);
        assert!(end.is_ok());

        // 再次结束 → 应报错
        let end2 = cmd_call_end(&state, &pending.call_id);
        assert!(end2.is_err());
        assert_eq!(end2.unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn registry_gc_expired() {
        let config = RegistryConfig {
            max_plugins: 8,
            max_active_identities: 8,
            max_pending_calls: 10,
            pending_ttl: std::time::Duration::from_nanos(1),
            plugin_filter: None,
        };
        let state = CommandState::with_config(config);
        let index = empty_index();
        let manifest = test_manifest("p.gc");
        state.registry.install(&index, manifest).unwrap();
        cmd_registry_admin(&state, "p.gc", RegistryAdminOp::Enable).unwrap();
        cmd_lifecycle_report(&state, "plugin-p.gc", None, Event::Attach).unwrap();

        // 发起多个调用
        for _ in 0..5 {
            cmd_plugin_call(&state, "plugin-p.gc", None, "gc_test", serde_json::Value::Null)
                .unwrap();
        }

        // 等待 TTL 过期
        std::thread::sleep(std::time::Duration::from_millis(5));

        // GC 应回收全部过期条目
        let gc_count = state.registry.gc_expired();
        assert_eq!(gc_count, 5, "应回收 5 个过期条目");
    }

    // ── 窗口管理命令测试（P1-1）──

    /// 主窗 label（与 `tauri.conf.json` 的窗口 label 约定一致：缺省主窗 = `main`）。
    const MAIN: &str = "main";

    #[test]
    fn cmd_window_minimize_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_minimize(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_maximize_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_maximize(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_restore_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_restore(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_close_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_close(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_quit_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_quit(&state).is_ok());
    }

    // ── 壳扩展命令测试（P2 断链修复）──

    #[test]
    fn cmd_window_set_position_records_rect() {
        let state = CommandState::new();
        cmd_window_set_position(&state, MAIN, 120, 80).unwrap();
        let rect = state.shell_ext.lock().window_rect;
        assert_eq!((rect.0, rect.1), (120, 80));
    }

    #[test]
    fn cmd_window_set_size_records_rect() {
        let state = CommandState::new();
        cmd_window_set_position(&state, MAIN, 10, 20).unwrap();
        cmd_window_set_size(&state, MAIN, 1280, 720).unwrap();
        let rect = state.shell_ext.lock().window_rect;
        assert_eq!(rect, (10, 20, 1280, 720));
    }

    #[test]
    fn cmd_clipboard_roundtrip() {
        let state = CommandState::new();
        assert_eq!(cmd_clipboard_read(&state).unwrap(), "");
        cmd_clipboard_write(&state, "hello tauron".to_string()).unwrap();
        assert_eq!(cmd_clipboard_read(&state).unwrap(), "hello tauron");
    }

    #[test]
    fn cmd_deep_link_register_records_protocol() {
        let state = CommandState::new();
        assert!(state.shell_ext.lock().deep_link_protocol.is_none());
        cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
        assert_eq!(
            state.shell_ext.lock().deep_link_protocol.as_deref(),
            Some("tauron")
        );
    }

    #[test]
    fn deep_link_delivered_reaches_subscriber_queue() {
        // 全链路：注册协议 → 前端订阅 topic → OS 回调投递 → drain 取帧
        let state = CommandState::new();
        cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
        cmd_events_subscribe(&state, "p1", "w1", "deep-link").unwrap();

        deep_link_delivered(&state, "tauron://plugin/open?x=1").unwrap();

        let frames = cmd_events_drain(&state, "p1", "event").unwrap();
        assert_eq!(frames.len(), 1, "deep-link 帧必须抵达订阅者队列");
        let payload = &frames[0].payload;
        assert_eq!(payload["url"], serde_json::json!("tauron://plugin/open?x=1"));
        assert_eq!(payload["protocol"], serde_json::json!("tauron"));
    }

    #[test]
    fn cmd_market_download_and_install_track_state() {
        let state = CommandState::new();
        let dl = cmd_market_download(&state, Some("2.0.0")).unwrap();
        assert!(dl.ok);
        // R8：模拟这件事是**线字段**，前端据此判断"这不是真实下载"。
        assert!(dl.simulated);
        assert_eq!(dl.version.as_deref(), Some("2.0.0"));
        assert!(dl.reason.as_deref().is_some_and(|r| r.contains("没有")));
        assert_eq!(
            serde_json::to_value(&dl).unwrap(),
            serde_json::json!({
                "ok": true,
                "simulated": true,
                "version": "2.0.0",
                "reason": dl.reason.clone(),
            }),
            "线形字段名/数量必须与 TS 同步（camelCase、无额外字段）"
        );
        assert_eq!(
            state.shell_ext.lock().update_state.as_deref(),
            Some("downloaded:2.0.0")
        );
        let inst = cmd_market_install(&state, Some("2.0.0")).unwrap();
        assert!(inst.ok && inst.simulated);
        assert_eq!(
            serde_json::to_value(&inst).unwrap()["simulated"],
            serde_json::json!(true)
        );
        assert_eq!(
            state.shell_ext.lock().update_state.as_deref(),
            Some("installed:2.0.0")
        );
        // 缺省 version 时线形仍是同一形状（version = null），不是换一种形状。
        let none = cmd_market_download(&state, None).unwrap();
        assert_eq!(none.version, None);
        assert_eq!(
            serde_json::to_value(&none).unwrap()["version"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn cmd_dialog_commands_return_cancelled_defaults() {
        let state = CommandState::new();
        // 无原生 UI：open/save 返回 None（取消），confirm 返回 false。
        assert_eq!(cmd_dialog_open(&state, false, false).unwrap(), None);
        assert_eq!(cmd_dialog_save(&state, None).unwrap(), None);
        assert!(!cmd_dialog_confirm(&state, "t", "m").unwrap());
        assert!(cmd_dialog_message(&state, "t", "m", None).is_ok());
        assert!(cmd_dialog_message(&state, "t", "m", Some("error")).is_ok());
    }

    #[test]
    fn cmd_dialog_message_rejects_kind_outside_the_closed_vocabulary() {
        let state = CommandState::new();
        let err = cmd_dialog_message(&state, "t", "m", Some("question"))
            .expect_err("表外 kind 必须拒绝（R8 之前它被整个忽略）");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        // 闭集内的三个值 + 缺省都必须通过。
        for kind in [None, Some("info"), Some("warning"), Some("error")] {
            assert!(cmd_dialog_message(&state, "t", "m", kind).is_ok(), "{kind:?}");
        }
    }

    #[test]
    fn cmd_deep_link_register_rejects_empty_protocol() {
        let state = CommandState::new();
        let err = cmd_deep_link_register(&state, "  ".to_string())
            .expect_err("空协议不是可注册的协议名");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(state.shell_ext.lock().deep_link_protocol.is_none());
    }

    /// 造出「已声明 topic + 已订阅 + 有队列」的插件状态。
    fn seed_bus(state: &CommandState, id: &str, topic: &str) {
        state
            .bus
            .lock()
            .declare_topics(id, &[EventDecl { topic: topic.to_string(), public: true }])
            .unwrap();
        state.bus.lock().subscribe(id, &format!("plugin-{id}"), topic).unwrap();
        state.bus.lock().publish(
            id,
            topic,
            serde_json::Value::Null,
            tauron_host::eventbus::ChannelKind::Event,
        );
    }

    /// 卸载必须回收事件总线残留（§8-3 零悬挂）。
    ///
    /// 订阅/反向索引/队列/声明都以插件 id 为键，注册表条目移除后再无引用者：
    /// 不回收就会留到进程结束，卸载→重装循环持续累积。
    #[test]
    fn uninstall_disposes_event_bus_residue() {
        let state = CommandState::new();
        let topic = "plugin:com.a:x";
        seed_bus(&state, "com.a", topic);
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 前置：残留确实存在（否则本用例没有判别力）。
        {
            let bus = state.bus.lock();
            assert!(bus.has_hanging_subscriptions("com.a"), "前置：应有订阅");
            assert!(bus.has_hanging_queues("com.a"), "前置：应有队列");
            assert!(bus.topic_meta(topic).is_some(), "前置：应有声明");
        }

        let out = cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();
        assert!(!out.illegal, "Installed → Uninstalled 应为合法迁移");

        let bus = state.bus.lock();
        assert!(!bus.has_hanging_subscriptions("com.a"), "卸载后不得残留订阅");
        assert!(!bus.has_hanging_queues("com.a"), "卸载后不得残留队列");
        assert!(bus.topic_meta(topic).is_none(), "卸载后 topic 声明应被回收");
    }

    #[test]
    fn uninstall_recycles_every_per_plugin_side_state() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 三个旁路状态先种上数据（否则本用例没有判别力）。
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "A")]), Some("com.a")).unwrap();
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "B")]), Some("com.b")).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        cmd_contributes_register(
            &state,
            "com.a",
            ContributeEntry {
                plugin_id: "com.a".to_string(),
                kind: "menu".to_string(),
                id: "a.menu".to_string(),
                label: "A".to_string(),
            },
        )
        .unwrap();
        state.recovery.lock().register_plugin("com.a");
        // 多选择器分组登记（以订阅者为键）——卸载必须连它一起回收。
        state.subscription_groups.lock().insert(
            "grp:a".to_string(),
            GroupSubscription {
                subscriber: "com.a".into(),
                tokens: vec!["t.a1".into(), "t.a2".into()],
            },
        );
        state.subscription_groups.lock().insert(
            "grp:b".to_string(),
            GroupSubscription {
                subscriber: "com.b".into(),
                tokens: vec!["t.b1".into()],
            },
        );

        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();

        // 分组登记：被卸载插件的回收，其它订阅者的保留。
        {
            let groups = state.subscription_groups.lock();
            assert!(
                !groups.contains_key("grp:a"),
                "卸载必须回收该插件的分组登记（否则死条目累积）"
            );
            assert!(groups.contains_key("grp:b"), "不得连坐回收其它订阅者");
        }

        // i18n：被卸载插件的命名空间清空，其他插件的文案不受影响。
        assert_eq!(
            cmd_i18n_t(&state, "plugin:com.a.oc.title").unwrap(),
            "plugin:com.a.oc.title"
        );
        assert_eq!(cmd_i18n_t(&state, "plugin:com.b.oc.title").unwrap(), "B");

        // 贡献：被卸载插件的入口全部移除。
        assert!(cmd_contributes_list(&state, Some("menu")).unwrap()
            .iter()
            .all(|e| e.plugin_id != "com.a"));

        // 恢复引擎：不再被登记（它的状态表是持久化的，残留会跨重启）。
        assert!(state.recovery.lock().plugin_state("com.a").is_none());
    }

    /// 禁用**不得**回收总线残留：只有卸载/清除才回收。
    ///
    /// 禁用后插件仍可重新启用，订阅与声明必须原样保留。
    #[test]
    fn disable_keeps_event_bus_residue() {
        let state = CommandState::new();
        let topic = "plugin:com.a:y";
        seed_bus(&state, "com.a", topic);
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Disable).unwrap();

        let bus = state.bus.lock();
        assert!(bus.has_hanging_subscriptions("com.a"), "禁用不应回收订阅");
        assert!(bus.topic_meta(topic).is_some(), "禁用不应回收声明");
    }

    // ────────────────────────────────────────────────────────────
    // P0-2：进程插件运行时（tauron-proc 激活）
    //
    // 这些用例一律走 **fake 启动面**：测试里绝不真起 sidecar 进程（CI 上没有
    // sidecar 二进制；真起进程还会带进时序、残留进程与平台差异，把用例变 flaky）。
    // 被验证的是执行器**语义**——拒绝路径、租约绑定、幂等、崩溃判定与投递；
    // "真进程真的能起来"由 `tauron_proc::CommandSpawner` 自己的用例覆盖。
    // ────────────────────────────────────────────────────────────

    /// 记录型 + 可控存活的 fake 启动面。
    ///
    /// 同时提供测试需要的三件能力：记录「启动面被调用几次 / 用什么配置」、
    /// 记录「终止面按哪个 pid 被调用」，以及凭空制造崩溃（把 pid 标记为已死）
    /// 与启动失败。
    #[derive(Default)]
    struct FakeSpawner {
        calls: Mutex<Vec<SpawnConfig>>,
        killed: Mutex<Vec<u32>>,
        alive: Mutex<std::collections::HashSet<u32>>,
        next_pid: std::sync::atomic::AtomicU32,
        fail_with: Mutex<Option<ProcError>>,
        fail_kill_with: Mutex<Option<String>>,
    }

    impl FakeSpawner {
        fn call_count(&self) -> usize {
            self.calls.lock().len()
        }

        fn last_cfg(&self) -> SpawnConfig {
            self.calls
                .lock()
                .last()
                .cloned()
                .expect("启动面未被调用（本用例要求它必须被调用过）")
        }

        /// 终止面收到的 pid 序列（按调用顺序）。
        fn killed(&self) -> Vec<u32> {
            self.killed.lock().clone()
        }

        /// 制造一次崩溃：此后 `is_alive(pid)` 恒为 false。
        fn crash(&self, pid: u32) {
            self.alive.lock().remove(&pid);
        }

        /// 让下一次 `spawn` 失败。
        fn fail_next_spawn_with(&self, e: ProcError) {
            *self.fail_with.lock() = Some(e);
        }

        /// 让所有 `kill` 都失败（模拟权限不足 / 杀不掉）。
        fn fail_kill_with(&self, msg: &str) {
            *self.fail_kill_with.lock() = Some(msg.to_string());
        }
    }

    impl ProcSpawner for FakeSpawner {
        fn spawn(&self, cfg: &SpawnConfig) -> ProcResult<SpawnedProc> {
            self.calls.lock().push(cfg.clone());
            if let Some(e) = self.fail_with.lock().take() {
                return Err(e);
            }
            let pid = self
                .next_pid
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1000;
            self.alive.lock().insert(pid);
            Ok(SpawnedProc { pid })
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.lock().contains(&pid)
        }

        fn kill(&self, pid: u32) -> ProcResult<tauron_proc::KillOutcome> {
            self.killed.lock().push(pid);
            if let Some(msg) = self.fail_kill_with.lock().clone() {
                return Err(ProcError::ProcessTerminated(msg));
            }
            // 已崩溃的 pid 如实按"早就没了"上报（与 `CommandSpawner` 同语义）；
            // 存活的 pid 按"真杀了"上报。
            let was_alive = self.alive.lock().remove(&pid);
            Ok(if was_alive {
                tauron_proc::KillOutcome::Terminated
            } else {
                tauron_proc::KillOutcome::AlreadyGone
            })
        }
    }

    fn process_manifest(id: &str, sidecar: Option<&str>) -> PluginManifest {
        let mut m = test_manifest(id);
        m.plugin_type = PluginType::Process;
        m.entry = tauron_host::manifest::EntrySpec {
            js: None,
            sidecar: sidecar.map(|s| s.to_string()),
            wasm: None,
            ui: None,
        };
        m
    }

    /// 验签材料齐全的 profile（缺任何一项都会被 §4.7 的 spawn 前校验挡住）。
    fn valid_profile() -> RuntimeSpawnProfile {
        RuntimeSpawnProfile {
            binary_path: None,
            args: vec!["--serve".to_string()],
            env: std::collections::HashMap::new(),
            signature: RuntimeBinarySignature {
                algorithm: "ed25519".to_string(),
                signature: "sig".to_string(),
                signer_id: "signer-001".to_string(),
            },
            binary_hash: "a".repeat(64),
            // 必须是**宿主当前 ABI 契约**（`cmd_runtime_spawn` 会比对，见
            // `runtime_spawn_rejects_abi_mismatch_before_starting_anything`）——
            // 不是随便填的占位串，填错会被 `E_ABI_MISMATCH` 挡在启动面之前。
            abi: RuntimeAbiFingerprint {
                rust_version: tauron_proc::SIDECAR_ABI_RUST_VERSION.to_string(),
                interface_hash: tauron_proc::SIDECAR_ABI_INTERFACE_HASH.to_string(),
            },
        }
    }

    /// 装好一个 process 插件，并返回带 fake 启动面的状态。
    fn process_state(id: &str, sidecar: Option<&str>) -> (CommandState, Arc<FakeSpawner>) {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        state
            .registry
            .install(&empty_index(), process_manifest(id, sidecar))
            .unwrap();
        (state, fake)
    }

    /// 「非 Process 插件被拒」：结构化失败码 + **启动面一次都不许被调用** + 无租约。
    #[test]
    fn runtime_spawn_rejects_non_process_plugin_without_starting_anything() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        // test_manifest 是 js 型。
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let err = cmd_runtime_spawn(&state, "com.a", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME);
        assert!(err.message.contains("process"), "message 必须指出正确类型：{}", err.message);
        assert_eq!(fake.call_count(), 0, "类型不匹配时绝不调用启动面");
        assert_eq!(state.registry.runtime_len(), 0, "拒绝路径不得留下租约");
    }

    /// ABI 契约不匹配 → `E_ABI_MISMATCH`（**独立于** `E_INSTALL_FAILED`），且
    /// **启动面一次都不许被调用**：不合格的载荷连"是否已有租约"都不该被它探测到。
    ///
    /// 本用例是 `tauron_proc::validate_abi` 的**生产调用点**的证据——它此前只有
    /// 测试调用，导致 `E_ABI_MISMATCH` 这个线协议码**永不产生**（孤儿码），
    /// 而 TS 侧 `shell-client.ts` 的文档却在承诺它。
    #[test]
    fn runtime_spawn_rejects_abi_mismatch_before_starting_anything() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");

        // 1) 接口哈希维度不符 → E_ABI_MISMATCH，启动面零调用、无租约。
        let mut bad_iface = valid_profile();
        bad_iface.abi.interface_hash = "tauron-sidecar-rpc/0".to_string();
        let err = cmd_runtime_spawn(&state, "com.proc", &bad_iface).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_ABI_MISMATCH);
        assert!(!err.retryable, "ABI 不匹配不可重试——重试还是同一个不匹配");
        assert_eq!(fake.call_count(), 0, "ABI 不合格绝不调用启动面");
        assert_eq!(state.registry.runtime_len(), 0, "拒绝路径不得留下租约");

        // 2) 协议版本维度同样受校验（两个维度都查，不是只查一个）。
        let mut bad_ver = valid_profile();
        bad_ver.abi.rust_version = "tauron-proc-abi/0".to_string();
        assert_eq!(
            cmd_runtime_spawn(&state, "com.proc", &bad_ver).unwrap_err().code,
            ErrorCode::E_ABI_MISMATCH
        );
        assert_eq!(fake.call_count(), 0, "第二个维度被拒时启动面仍为零调用");

        // 3) 契约值正确（`valid_profile()` 即契约值）→ 放行，证明拒因确为 ABI。
        assert!(cmd_runtime_spawn(&state, "com.proc", &valid_profile()).is_ok());
        assert_eq!(fake.call_count(), 1, "契约正确才真正拉起 sidecar");
    }

    #[test]
    fn runtime_spawn_unknown_plugin_is_e_unknown_plugin() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        let err = cmd_runtime_spawn(&state, "com.ghost", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
        assert_eq!(fake.call_count(), 0);
        assert_eq!(state.registry.runtime_len(), 0);
    }

    /// 正路：真经过启动面、缺省二进制取 manifest 的 `entry.sidecar`、lease 绑 pid。
    #[test]
    fn runtime_spawn_starts_sidecar_and_binds_lease_to_pid() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        // 可用性门（P0-2）：只有 ENABLED / RUNNING 才允许有 sidecar。
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();

        assert_eq!(fake.call_count(), 1, "必须真的经过注入的启动面");
        let cfg = fake.last_cfg();
        assert_eq!(cfg.binary_path, "sidecar.exe", "缺省二进制 = manifest 的 entry.sidecar");
        assert_eq!(cfg.args, vec!["--serve".to_string()]);
        assert_eq!(handle.pid, 1000);

        let id = PluginId::new("com.proc").unwrap();
        assert_eq!(
            state.registry.runtime_handle_of(&id),
            Some(handle.clone()),
            "lease ↔ pid 必须登记进注册表"
        );

        let health = cmd_runtime_health(&state, &handle.lease).unwrap();
        assert_eq!(health.pid, handle.pid);
        assert!(health.alive, "fake 未标记死亡 → 必须报存活");
        assert_eq!(health.crashes, 0);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(
            health.reap,
            ReapStats::default(),
            "health 里的 reap 是全局回收留痕，此刻还没有任何回收动作"
        );
    }

    /// **可用性门**：装好但没启用的插件不许起进程（`INSTALLED` 不是可用状态）。
    ///
    /// 这条门同时是 `ERRORED_USER_CONFIRM` 的人工闸门（"不 active"是同一条规则）。
    /// 顺序也在这里钉住：**先 enable，再 spawn**——不是为了让老用例过而放宽门。
    #[test]
    fn installed_plugin_cannot_spawn_until_it_is_enabled() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed
        );

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(!err.retryable, "不可用状态需要人去启用，不是可重试错误");
        assert!(
            err.message.contains("INSTALLED") && err.message.contains("不可用"),
            "message 必须说清当前状态与下一步：{}",
            err.message
        );
        assert_eq!(fake.call_count(), 0, "被门拦下时一次都不许起进程");
        assert_eq!(state.registry.runtime_len(), 0, "拒绝路径不得留下租约");

        // 既有启用路径（不新造入口）之后必须放行。
        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Enable).unwrap();
        assert_eq!(out.to, tauron_host::lifecycle::State::Enabled);
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(fake.call_count(), 1);
        assert_eq!(handle.pid, 1000);
    }

    #[test]
    fn runtime_spawn_profile_can_override_binary_path() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let mut p = valid_profile();
        p.binary_path = Some("C:\\plugins\\other.exe".to_string());
        cmd_runtime_spawn(&state, "com.proc", &p).unwrap();
        assert_eq!(fake.last_cfg().binary_path, "C:\\plugins\\other.exe");
    }

    /// 重复 spawn 只有**一种**行为：返回既有 lease，且绝不启动第二个进程。
    #[test]
    fn runtime_spawn_is_idempotent_per_plugin() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let first = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(second, first, "重复 spawn 必须原样返回既有 lease");
        assert_eq!(fake.call_count(), 1, "绝不启动第二个进程（否则会产生孤儿 PID）");
        assert_eq!(state.registry.runtime_len(), 1, "同一插件至多一条租约");
    }

    /// 未验签的载荷不得到达启动面（§4.7：spawn 前校验签名与哈希）。
    #[test]
    fn runtime_spawn_rejects_unverified_profile_before_touching_spawner() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let mut p = valid_profile();
        p.binary_hash = "too-short".to_string();
        let err = cmd_runtime_spawn(&state, "com.proc", &p).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert_eq!(fake.call_count(), 0, "未验签的载荷不得到达启动面");
        assert_eq!(state.registry.runtime_len(), 0);
    }

    /// process 插件**装不进来**：manifest 校验要求非空 `entry.sidecar`。
    ///
    /// 这正是 `cmd_runtime_spawn` 里"profile 与 manifest 都没给二进制路径"那条兜底
    /// 分支在实践中**不可达**的原因。它保留为防御性检查（不 panic、不猜路径），
    /// 本用例锁住它的前提——哪天 manifest 放宽了，这条分支就会变成真路径。
    #[test]
    fn process_plugin_without_sidecar_cannot_even_be_installed() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        let err = state
            .registry
            .install(&empty_index(), process_manifest("com.proc", None))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("entry.sidecar"), "message={}", err.message);
        assert_eq!(fake.call_count(), 0, "装不进来的插件不可能走到启动面");
        assert_eq!(state.registry.runtime_len(), 0);
    }

    /// 「fake spawner 未被调用时不得产生 lease」的另一半：启动**失败**同样不得产生租约。
    #[test]
    fn runtime_spawn_failure_never_creates_a_lease() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        fake.fail_next_spawn_with(ProcError::SpawnFailed("exec failed".to_string()));
        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED);
        assert!(!err.retryable, "确定性的启动失败不得被标成可重试");
        assert_eq!(state.registry.runtime_len(), 0, "启动失败绝不能铸出租约");
        assert!(
            state
                .registry
                .runtime_handle_of(&PluginId::new("com.proc").unwrap())
                .is_none()
        );
    }

    /// 未知 / 失效 lease → `E_LEASE_EXPIRED`（**不是** `E_CALL_NOT_FOUND`）。
    #[test]
    fn runtime_health_unknown_lease_is_lease_expired_not_call_not_found() {
        let (state, _fake) = process_state("com.proc", Some("sidecar.exe"));
        let err = cmd_runtime_health(&state, "no-such-lease").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_LEASE_EXPIRED);
        assert_ne!(
            err.code,
            ErrorCode::E_CALL_NOT_FOUND,
            "租约边界必须与 pending call 边界分开"
        );
    }

    /// 崩溃：投递 `RuntimeCrash`（状态机真的吃了它）+ `consecutiveFailures` +1
    /// + `CrashTracker` 记一次，且**重复轮询不得重复计数**。
    #[test]
    fn runtime_crash_delivers_event_and_bumps_consecutive_failures_once() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        // 先让状态可用（可用性门），`RUNNING` 由 spawn 自己推上去。
        enabled_process_plugin(&state, "com.proc");
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Enabled);

        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        // `RuntimeCrash` 的合法源态是 ENABLED/RUNNING；spawn 真起了进程 → 状态必须已到 RUNNING。
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Running);
        fake.crash(handle.pid);

        let health = cmd_runtime_health(&state, &handle.lease).unwrap();
        assert!(!health.alive, "被标记死亡的 pid 必须报不存活");
        assert_eq!(health.crashes, 1, "崩溃必须记进 CrashTracker");
        assert_eq!(
            health.consecutive_failures, 1,
            "consecutiveFailures 必须 +1（复用恢复引擎的既有计数）"
        );

        // 事件不是投进虚空：RUNNING → ERRORED_RETRYABLE，并消耗既有重试预算。
        let entry = state.registry.find(&id).unwrap();
        assert_eq!(
            entry.state.state,
            LifecycleState::ErroredRetryable,
            "RuntimeCrash 必须真的驱动状态机"
        );
        assert_eq!(entry.state.retry_count, 1);

        // 轮询式探测的关键不变量：同一次死亡只记一次。
        let again = cmd_runtime_health(&state, &handle.lease).unwrap();
        assert!(!again.alive);
        assert_eq!(again.crashes, 1, "重复 health 不得把一次死亡放大成两次");
        assert_eq!(again.consecutive_failures, 1);
        assert_eq!(state.registry.find(&id).unwrap().state.retry_count, 1);

        // 跨通道一致：恢复引擎的线形视图同样看得见这次失败。
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["counter"]["consecutiveFailures"], 1);
    }

    /// 卸载必须回收租约：否则同名插件重装时旧 lease 会变成孤儿条目。
    #[test]
    fn runtime_lease_is_recycled_with_the_plugin_entry() {
        let (state, _fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();

        assert_eq!(state.registry.runtime_len(), 0, "卸载必须回收租约");
        assert_eq!(
            cmd_runtime_health(&state, &handle.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED
        );
    }

    /// 崩溃预算耗尽 → `E_PLUGIN_DISABLED` 而非 panic 码；启动失败类错误都不是可重试。
    #[test]
    fn proc_errors_map_to_non_retryable_structured_codes() {
        assert_eq!(
            proc_error_to_host(ProcError::InvalidSpawnConfig("x".to_string())).code,
            ErrorCode::E_INVALID_MANIFEST
        );
        assert_eq!(
            proc_error_to_host(ProcError::HashMismatch).code,
            ErrorCode::E_INSTALL_FAILED
        );
        // ABI 不匹配**独立成码**（不并进 E_INSTALL_FAILED）：它是版本兼容问题，
        // 前端据此提示"升级插件/宿主"而不是"重装"。
        assert_eq!(
            proc_error_to_host(ProcError::AbiMismatch {
                expected: "tauron-proc-abi/1".to_string(),
                actual: "tauron-proc-abi/0".to_string(),
            })
            .code,
            ErrorCode::E_ABI_MISMATCH
        );
        assert_eq!(
            proc_error_to_host(ProcError::CrashLimitExceeded { plugin_id: "p".to_string() }).code,
            ErrorCode::E_PLUGIN_DISABLED
        );
        for e in [
            ProcError::SpawnFailed("x".to_string()),
            ProcError::Timeout("x".to_string()),
            ProcError::HeartbeatLost("x".to_string()),
        ] {
            let mapped = proc_error_to_host(e);
            assert_ne!(
                mapped.code,
                ErrorCode::E_HOST_PANIC,
                "启动失败不是 panic——谎报 panic 会被前端当可重试错误无限重试"
            );
            assert!(!mapped.retryable);
        }
    }

    /// 线形契约：TS 侧 camelCase 必须能反序列化；必填项缺失必须报错（不得静默成空串）。
    #[test]
    fn runtime_spawn_profile_uses_camel_case_wire_shape() {
        let v = serde_json::json!({
            "binaryPath": "sidecar.exe",
            "args": ["--serve"],
            "env": { "RUST_LOG": "info" },
            "signature": { "algorithm": "ed25519", "signature": "sig", "signerId": "signer-001" },
            "binaryHash": "a".repeat(64),
            "abi": { "rustVersion": "1.98.0", "interfaceHash": "iface" }
        });
        let p: RuntimeSpawnProfile = serde_json::from_value(v).expect("TS 线形必须可反序列化");
        assert_eq!(p.binary_path.as_deref(), Some("sidecar.exe"));
        assert_eq!(p.signature.signer_id, "signer-001");
        assert_eq!(p.abi.rust_version, "1.98.0");
        assert_eq!(p.env.get("RUST_LOG").map(String::as_str), Some("info"));

        // 缺 signature/abi → 反序列化必须失败（不能悄悄用空串过校验以外的路）。
        let missing = serde_json::json!({ "binaryHash": "a".repeat(64) });
        assert!(serde_json::from_value::<RuntimeSpawnProfile>(missing).is_err());
    }

    #[test]
    fn runtime_health_serializes_camel_case() {
        let h = RuntimeHealth {
            alive: false,
            pid: 7,
            crashes: 2,
            consecutive_failures: 1,
            reap: ReapStats {
                attempts: 3,
                terminated: 1,
                already_gone: 1,
                failures: 1,
                last_error: Some("拒绝访问".to_string()),
            },
        };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "alive": false,
                "pid": 7,
                "crashes": 2,
                "consecutiveFailures": 1,
                "reap": {
                    "attempts": 3,
                    "terminated": 1,
                    "alreadyGone": 1,
                    "failures": 1,
                    "lastError": "拒绝访问"
                }
            })
        );
        // 嵌套字段名也必须是既有 camelCase 约定（否则 TS 侧 `alreadyGone` 读成 undefined，
        // 留痕又变回"只有日志"）。
        assert!(v["reap"].get("alreadyGone").is_some());
        assert!(v["reap"].get("lastError").is_some());
    }

    // ────────────────────────────────────────────────────────────
    // 缺口二：崩溃预算超限必须真的拒绝启动（`is_crash_exceeded` 不得是孤儿逻辑）
    // ────────────────────────────────────────────────────────────

    /// 把插件推进到**可用**状态（`ENABLED`）——既有启用路径，不新造入口。
    ///
    /// 刻意**不**顺手投 `ATTACH`：进程插件的 `RUNNING` 必须由"真的起了新进程"
    /// 这一步推上去（见 `attach_after_spawn`），测试里伪造一步就测不出那条断链。
    fn enabled_process_plugin(state: &CommandState, id: &str) {
        cmd_registry_admin(state, id, RegistryAdminOp::Enable).unwrap();
    }

    /// 启动 → 崩溃 → 查健康，返回本次崩溃前铸造的句柄。
    fn spawn_then_crash(state: &CommandState, fake: &FakeSpawner, id: &str) -> RuntimeHandle {
        let handle = cmd_runtime_spawn(state, id, &valid_profile()).unwrap();
        fake.crash(handle.pid);
        let health = cmd_runtime_health(state, &handle.lease).unwrap();
        assert!(!health.alive);
        handle
    }

    /// 把崩溃窗口推过上限（`> max_crashes`），同时让插件**保持可用**。
    ///
    /// 第一次崩溃走真实投递链（health → `RuntimeCrash` → 恢复引擎 → 状态机），
    /// 证明窗口计数确实由 health 驱动；其余直接推 `CrashTracker`（**唯一**计数来源，
    /// 不是自造数字）。这么分两步是为了绕开一条无关交互：恢复引擎的
    /// `consecutiveFailures ≥ SAFEMODE_THRESHOLD` 会把插件打成
    /// `DISABLED + disabledBySafemode`，那样测到的就变成"状态不可用"而不是"窗口超限"。
    /// 崩溃后先让插件自报恢复（既有迁移 `ERRORED_RETRYABLE + RETRY_OK → ENABLED`），
    /// 于是调用本函数后插件**可用**且窗口超限——正是预算门唯一该管的局面。
    fn exhaust_crash_window(state: &CommandState, fake: &FakeSpawner, id: &str) {
        let pid = PluginId::new(id).unwrap();
        let handle = spawn_then_crash(state, fake, id);
        assert_eq!(handle.pid, 1000);
        state.registry.report_event(&pid, Event::RetryOk).unwrap();
        while !state.proc_runtime.is_crash_exceeded(id) {
            state.proc_runtime.record_crash(id);
        }
        assert!(
            state.registry.find(&pid).unwrap().is_active(),
            "插件必须仍可用，否则本用例测到的是可用性门而不是预算门"
        );
    }

    /// **边界**：窗口内崩溃数**未**超过 `max_crashes` 时，重试必须被放行
    /// （`tauron_proc::CrashTracker` 的定义：`max_crashes` 是"允许的崩溃次数"，
    /// 超限的判定是 `> max_crashes`）。这条用例锁住"我们没有另造一套更严的阈值"。
    #[test]
    fn crash_window_below_the_limit_does_not_block_a_retry() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");

        // 真实崩溃一次：窗口 1，且状态机离开可用态（崩溃的既有后果）。
        let first = spawn_then_crash(&state, &fake, "com.proc");
        assert_eq!(state.proc_runtime.crash_count("com.proc"), 1);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::ErroredRetryable,
            "崩溃后不再可用——重试前必须先让状态重新可用（P0-2 可用性门）"
        );
        // 插件自报恢复（既有迁移，带 ResetCounters），窗口不动。
        state.registry.report_event(&id, Event::RetryOk).unwrap();

        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile())
            .expect("窗口未超限 + 状态可用 → 重试必须放行");
        assert_ne!(second.lease, first.lease);
        assert_eq!(second.pid, 1001, "重试必须起**新**进程");
        assert_eq!(fake.call_count(), 2);

        // 窗口推到上限（3 = `CrashLimit::default().max_crashes`）：仍未超限。
        while state.proc_runtime.crash_count("com.proc") < 3 {
            state.proc_runtime.record_crash("com.proc");
        }
        assert!(
            !state.proc_runtime.is_crash_exceeded("com.proc"),
            "3 次 == 上限：上限的判定是 > max_crashes，第 4 次才算超"
        );
    }

    /// **超限**：窗口内第 4 次崩溃后，即使插件**可用**也必须拒绝启动——
    /// 且**不调用启动面**、落到 `ERRORED_USER_CONFIRM`、错误码为既有的
    /// `E_PLUGIN_DISABLED`（不可重试）。
    ///
    /// 这条用例专门锁"预算门不依赖状态门"：状态是 ENABLED（可用性门放行），
    /// 拒绝只能来自 `CrashTracker::is_exceeded`——它是本仓唯一允许判定"崩溃超限"
    /// 的地方（`tauron-proc` 定稿的 3 次 / 5min），本命令不另造阈值。
    #[test]
    fn crash_window_exceeded_refuses_a_fresh_start_even_when_the_plugin_is_available() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        exhaust_crash_window(&state, &fake, "com.proc");

        assert!(state.proc_runtime.is_crash_exceeded("com.proc"));
        assert_eq!(state.proc_runtime.crash_count("com.proc"), 4, "窗口内 4 次（> 上限 3）");
        assert_eq!(fake.call_count(), 1, "只有第一次崩溃前起过进程");

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::E_PLUGIN_DISABLED,
            "复用既有码：预算耗尽 = 插件当前不可用、需宿主显式放行"
        );
        assert!(!err.retryable, "要求人工介入的失败绝不能被标成可重试");
        assert_eq!(fake.call_count(), 1, "拒绝路径绝不调用启动面");
        assert!(
            state.registry.runtime_needs_restart(&id),
            "拒绝后不得凭空留下活租约（旧租约是崩溃租约，仍然存在）"
        );
        assert!(err.message.contains('4'), "message 必须带上真实崩溃次数：{}", err.message);
        assert!(
            err.message.contains("ERRORED_USER_CONFIRM"),
            "落点必须写进错误信息：{}",
            err.message
        );

        // 落点：状态机必须真的落到"需用户确认"（不是只改了错误信息）。
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredUserConfirm,
            "预算耗尽必须落到 ERRORED_USER_CONFIRM"
        );

        // 再点一次也不放行（幂等拒绝，不会因为重试而漂移）。
        let again = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(again.code, ErrorCode::E_PLUGIN_DISABLED);
        assert_eq!(fake.call_count(), 1);
    }

    /// **人工确认出口**：`host_registry_admin` 的 `Enable` 是既有的"用户显式放行"
    /// 入口（不新造）。它必须同时清零状态机预算与**进程侧崩溃窗口**——只清前者会
    /// 出现"用户已确认、状态机 Enabled、spawn 仍被 5 分钟窗口拒绝"的死结。
    ///
    /// 本用例同时钉住两条**既有交互**（都不是本命令引入的）：
    /// 1. 崩溃把恢复引擎的 `consecutiveFailures` 推过 `SAFEMODE_THRESHOLD`，于是
    ///    `Enable` 之后的对账又用 `SafemodeEnter` 把插件打成
    ///    `DISABLED + disabledBySafemode`（"安全模式优先于手动启用"）——因此这里
    ///    断言的是 `out.to == ENABLED`（本次迁移）与最终 `DISABLED`，而不是"最终
    ///    一定是 ENABLED"；
    /// 2. 因此"确认后能再起进程"的完整顺序是 **enable → 安全模式试验性启用
    ///    （`host_recover_trial_enable`）→ spawn**，这也正是 P0-2 可用性门要求的顺序。
    ///    本用例把三步都走一遍，证明它真的走得通（不是只写在文档里）。
    #[test]
    fn admin_enable_clears_the_crash_window_and_trial_enable_allows_spawn_again() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        exhaust_crash_window(&state, &fake, "com.proc");

        // 预算门先把插件落到"需用户确认"。
        assert_eq!(
            cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err().code,
            ErrorCode::E_PLUGIN_DISABLED
        );
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredUserConfirm
        );
        assert!(state.proc_runtime.is_crash_exceeded("com.proc"));

        // 用户在主窗显式启用（人工确认）。
        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Enable).unwrap();
        assert!(!out.illegal);
        assert_eq!(
            out.to,
            LifecycleState::Enabled,
            "ERRORED_USER_CONFIRM + ENABLE 必须回到 ENABLED（既有迁移）"
        );
        assert_eq!(
            state.proc_runtime.crash_count("com.proc"),
            0,
            "人工确认必须同时清零崩溃窗口（否则 spawn 仍被拒，用户点了没用）"
        );
        assert!(!state.proc_runtime.is_crash_exceeded("com.proc"));

        // 既有交互：安全模式优先于手动启用——但只在连续失败计数已过阈值时发生。
        // 本用例只走了 **1 次真实崩溃**（其余是直接推窗口计数，见 `exhaust_crash_window`），
        // 因此确认后不触发安全模式；安全模式那条腿由
        // `spawn_is_refused_while_disabled_by_safemode` 覆盖（那里真的崩了两次）。
        assert_eq!(
            state.recovery.lock().counter().consecutive_failures,
            1,
            "1 次真实崩溃 → 连续失败计数 1（< SAFEMODE_THRESHOLD）"
        );
        let after = state.registry.find(&id).unwrap();
        assert!(
            after.is_active(),
            "确认后插件必须处于可用状态（安全模式未介入）——这是「确认后能再 spawn」的前提"
        );
        assert!(!after.state.disabled_by_safemode);

        // 确认后必须真的能再起一个进程（窗口已清，不再被预算门拦下）。
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile())
            .expect("确认后必须能再次启动");
        assert_eq!(fake.call_count(), 2, "确认后必须能再次启动");
        assert_eq!(handle.pid, 1001, "必须是新的 pid");
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "新进程起好后状态必须到 RUNNING"
        );
    }

    /// 可用性门单独生效：状态机落到 `ERRORED_USER_CONFIRM`（人工闸门）而崩溃窗口
    /// **未**超限时，也必须拒绝启动——因为 `ERRORED_USER_CONFIRM` 不是可用状态。
    /// （人工确认闸门由可用性门覆盖，不再是独立分支。）
    #[test]
    fn user_confirm_state_refuses_spawn_via_the_availability_gate() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");

        // 一次崩溃 → ERRORED_RETRYABLE（崩溃窗口只有 1 次，远未超限）。
        spawn_then_crash(&state, &fake, "com.proc");
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredRetryable
        );
        // 走**既有**迁移落到"需用户确认"：ErroredRetryable + RETRY_EXHAUSTED。
        let out = state.registry.report_event(&id, Event::RetryExhausted).unwrap();
        assert!(!out.illegal);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredUserConfirm
        );
        assert!(
            !state.proc_runtime.is_crash_exceeded("com.proc"),
            "本用例的前提：崩溃窗口**没有**超限（拒绝只能来自可用性门）"
        );

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(!err.retryable);
        assert!(
            err.message.contains("ERRORED_USER_CONFIRM"),
            "message 必须点明当前状态：{}",
            err.message
        );
        assert_eq!(fake.call_count(), 1, "被门拦下时一次都不许再起进程");
    }

    /// 中间态同样被拒：崩溃后 `ERRORED_RETRYABLE` 不是可用状态（"还有重试预算"
    /// 不等于"现在可以起进程"）。
    #[test]
    fn errored_retryable_state_refuses_spawn_until_re_enabled() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        spawn_then_crash(&state, &fake, "com.proc");

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(err.message.contains("ERRORED_RETRYABLE"), "message={}", err.message);
        assert_eq!(fake.call_count(), 1);
        assert!(!state.proc_runtime.is_crash_exceeded("com.proc"), "窗口只有 1 次");

        // 既有路径重新可用后，重试被放行（这正是"有预算的自动重试"）。
        state.registry.report_event(&id, Event::RetryOk).unwrap();
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(handle.pid, 1001);
    }

    /// **不可用即无进程**（P0-2 单一规则）：运行中的插件被禁用时，它的 sidecar 必须
    /// 被终止——不能出现"注册表说禁用、进程还在跑"。
    #[test]
    fn disabling_a_running_plugin_terminates_its_sidecar() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "spawn 成功 → RUNNING；只有禁用一个**在运行**的插件才测得到「禁用即回收进程」"
        );
        assert!(fake.killed().is_empty(), "禁用之前不得终止");

        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Disable).unwrap();
        assert_eq!(out.to, LifecycleState::Disabled);

        assert_eq!(fake.killed(), vec![handle.pid], "禁用必须终止该插件仍活着的 sidecar");
        assert_eq!(state.registry.runtime_len(), 0, "租约必须一起回收");
        let s = state.registry.runtime_reap_stats();
        assert_eq!((s.attempts, s.terminated, s.failures), (1, 1, 0), "{s:?}");
        assert_eq!(
            cmd_runtime_health(&state, &handle.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED,
            "租约已随禁用回收"
        );

        // 禁用期间不能再起进程（可用性门）。
        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert_eq!(fake.call_count(), 1);
    }

    /// 安全模式禁用（`DISABLED + disabledBySafemode`）期间不得起进程：要让插件重新
    /// 可用，得走 `host_recover_trial_enable`（或用户确认后的 enable），**不是**放宽
    /// 这道门。
    #[test]
    fn spawn_is_refused_while_disabled_by_safemode() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        // 两次崩溃把恢复引擎推过 SAFEMODE_THRESHOLD。
        spawn_then_crash(&state, &fake, "com.proc");
        state.registry.report_event(&id, Event::RetryOk).unwrap();
        spawn_then_crash(&state, &fake, "com.proc");
        assert!(state.recovery.lock().counter().consecutive_failures >= 2);

        // 人工启用 → 对账（安全模式优先）→ DISABLED + 安全模式标志。
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Enable).unwrap();
        let entry = state.registry.find(&id).unwrap();
        assert_eq!(entry.state.state, tauron_host::lifecycle::State::Disabled);
        assert!(entry.state.disabled_by_safemode);

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(
            err.message.contains("DISABLED") && err.message.contains("host_recover_trial_enable"),
            "message 必须给出**正确**的下一步动作：{}",
            err.message
        );
        assert_eq!(fake.call_count(), 2, "安全模式期间一次都不许起进程");
        assert!(
            state.registry.runtime_needs_restart(&id),
            "崩溃租约仍在表里（留给 health 报账），但语义是「没有活进程」——这正是 needs_restart"
        );

        // 既有出口：安全模式内的**试验性启用**（`host_recover_trial_enable`，不新造入口）
        // → 状态回到可用 → 这时才允许 spawn。这就是"先 enable/trial-enable，再 spawn"。
        cmd_recover_trial_enable(&state, "com.proc").unwrap();
        assert!(
            state.registry.find(&id).unwrap().is_active(),
            "trial-enable 后插件必须处于可用状态（ENABLED/RUNNING）"
        );
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile())
            .expect("trial-enable 后必须能起进程");
        assert_eq!(fake.call_count(), 3);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Running,
            "新进程起好后状态必须到 RUNNING（进程插件没有 webview 可上报 ATTACH）"
        );
        assert!(handle.pid > 0);
    }

    // ────────────────────────────────────────────────────────────
    // 缺口一：租约回收必须真的终止 sidecar（否则卸载留下孤儿进程）
    // ────────────────────────────────────────────────────────────

    /// 卸载必须按租约里的 pid 调终止面，并留痕（`terminated` 计数）。
    #[test]
    fn uninstall_terminates_the_sidecar_and_traces_it() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert!(fake.killed().is_empty(), "未卸载前不得调终止面");

        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();

        assert_eq!(fake.killed(), vec![handle.pid], "必须终止租约绑定的那个 pid");
        assert_eq!(state.registry.runtime_len(), 0, "租约必须回收");
        let s = state.registry.runtime_reap_stats();
        assert_eq!(
            (s.attempts, s.terminated, s.failures),
            (1, 1, 0),
            "真杀必须计入 terminated：{s:?}"
        );
        assert_eq!(s.last_error, None);
    }

    /// 清除（Purge）走同一条回收路径。
    #[test]
    fn purge_also_terminates_the_sidecar() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Purge).unwrap();
        assert_eq!(fake.killed(), vec![handle.pid]);
        assert_eq!(state.registry.runtime_reap_stats().terminated, 1);
    }

    /// 终止失败（权限不足等）：**卸载必须成功**，但必须留痕（不得静默吞）——
    /// 而且留痕必须能从 IPC 看到（`host_runtime_health` 的 `reap` 字段），
    /// 否则"不静默吞"只等于写日志。
    #[test]
    fn kill_failure_does_not_fail_uninstall_but_is_traced() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        fake.fail_kill_with("拒绝访问（测试模拟权限不足）");

        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();
        assert!(!out.illegal, "卸载本身必须成功");
        assert_eq!(fake.killed(), vec![handle.pid], "终止面仍必须被调用（不能因为怕失败就不调）");
        assert_eq!(state.registry.runtime_len(), 0, "租约必须消失，否则重装会撞上旧租约");
        assert!(state.registry.find(&PluginId::new("com.proc").unwrap()).is_none());

        let s = state.registry.runtime_reap_stats();
        assert_eq!((s.attempts, s.terminated, s.failures), (1, 0, 1), "{s:?}");
        assert!(
            s.last_error.as_deref().unwrap_or("").contains("权限不足"),
            "失败原因必须可查：{:?}",
            s.last_error
        );

        // 同一宿主内、**另一条**租约的 health 也必须看到这次失败（`reap` 是全局
        // 留痕）：跨 IPC 可见 = 宿主 UI 能把它显示出来，而不是只写日志。
        state
            .registry
            .install(&empty_index(), process_manifest("com.other", Some("sidecar2.exe")))
            .unwrap();
        enabled_process_plugin(&state, "com.other");
        let other = cmd_runtime_spawn(&state, "com.other", &valid_profile()).unwrap();
        let health = cmd_runtime_health(&state, &other.lease).unwrap();
        assert_eq!((health.reap.attempts, health.reap.failures), (1, 1));
        assert!(
            health.reap.last_error.as_deref().unwrap_or("").contains("权限不足"),
            "跨 IPC 的留痕必须带原因：{:?}",
            health.reap.last_error
        );
        assert_ne!(health.reap, ReapStats::default(), "留痕必须真的跨 IPC 可见");
    }

    /// 崩溃后重试（换新租约）时，旧 pid 也必须先被终止——不允许"直接覆盖表项"。
    #[test]
    fn retry_after_crash_terminates_the_previous_pid_before_replacing_the_lease() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        let first = spawn_then_crash(&state, &fake, "com.proc");
        assert!(fake.killed().is_empty(), "换新之前不得终止");

        // 崩溃后状态离开可用态：重试前必须先让它重新可用（P0-2 可用性门）。
        state.registry.report_event(&id, Event::RetryOk).unwrap();

        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(fake.killed(), vec![first.pid], "旧 pid 必须先终止再换新");
        assert_ne!(second.lease, first.lease);
        assert_eq!(state.registry.runtime_len(), 1);
        // 崩溃租约的进程早已退出 → 如实记 `already_gone`，不混进失败计数。
        let s = state.registry.runtime_reap_stats();
        assert_eq!((s.attempts, s.terminated, s.already_gone, s.failures), (1, 0, 1, 0), "{s:?}");
    }

    /// 崩溃租约是"需要重启"，活租约不是——`host_runtime_health` 的幂等语义全靠它。
    #[test]
    fn live_lease_does_not_need_restart_but_crashed_lease_does() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        assert!(state.registry.runtime_needs_restart(&id), "没租约 = 需要启动");

        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert!(!state.registry.runtime_needs_restart(&id), "活租约不需要重启");
        fake.crash(handle.pid);
        cmd_runtime_health(&state, &handle.lease).unwrap();
        assert!(state.registry.runtime_needs_restart(&id), "崩溃后需要重启");
    }

    // ────────────────────────────────────────────────────────────
    // 状态机断链：spawn 成功必须把状态推到 RUNNING（进程插件没有 webview 可上报）
    // ────────────────────────────────────────────────────────────

    /// 全新 spawn 成功后状态必须 `ENABLED → RUNNING`（断言**注册表状态**，
    /// 不是"投过事件"）；幂等重复 spawn **不**投第二次 `ATTACH`。
    ///
    /// 判别力来自 `illegal_transitions`：`RUNNING + ATTACH` 在迁移表里没有规则，
    /// 所以"多投一次"一定会被状态机计成非法迁移——计数为 0 就证明真的没投。
    #[test]
    fn a_new_process_moves_the_plugin_from_enabled_to_running() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Enabled);

        let first = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "起了新进程就必须把状态推到 RUNNING（进程插件没有 webview 上报 ATTACH）"
        );
        assert_eq!(
            state.registry.find(&id).unwrap().state.illegal_transitions,
            0,
            "ENABLED + ATTACH 是合法迁移，不得产生非法迁移"
        );

        // 幂等重复 spawn：不得再投 ATTACH（否则会是一次非法迁移，计数会 +1）。
        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(second, first, "幂等返回既有租约");
        assert_eq!(fake.call_count(), 1, "绝不启动第二个进程");
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "状态不得因为重复调用而漂移"
        );
        assert_eq!(
            state.registry.find(&id).unwrap().state.illegal_transitions,
            0,
            "幂等返回不得再投 ATTACH（多投会是一条非法迁移）"
        );
    }

    /// 并发卸载窗口：进程起好后条目才消失 → **回收刚铸的租约**，绝不把孤儿句柄交出去。
    ///
    /// 真并发没法确定性复现，这里手工构造那个中间态（租约已铸、条目已消失），直接验证
    /// 补偿分支本身——它存在的唯一意义就是"孤儿进程不允许被交回调用方"。
    #[test]
    fn a_spawn_that_loses_its_entry_reclaims_the_fresh_lease() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();
        assert!(state.registry.find(&id).is_none(), "条目已消失 = 并发卸载窗口");

        // 造出"启动发生在卸载之后"的中间态：租约在表里，条目却已不存在。
        let (orphan, started) = state.registry.runtime_ensure_lease(&id, || Ok(9999)).unwrap();
        assert!(started);
        assert_eq!(state.registry.runtime_len(), 1);

        let err = attach_after_spawn(&state, &id).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
        assert!(err.message.contains("已回收刚铸的租约"), "message={}", err.message);
        assert_eq!(state.registry.runtime_len(), 0, "孤儿租约必须被回收");
        assert_eq!(
            *fake.killed().last().unwrap(),
            orphan.pid,
            "回收必须真的终止那个刚起的进程（killed={:?}）",
            fake.killed()
        );
    }

    /// 无可迁移路径时不伪造事件：状态已在 `RUNNING` 时 `ATTACH` 没有规则——
    /// 保持原状（目标态已达成），非法迁移由状态机计数（可查，不静默）。
    #[test]
    fn a_redundant_attach_is_counted_but_never_faked() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        let first = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();

        // 造出「状态 RUNNING + 租约已崩」这个组合：插件自报恢复并自己 ATTACH 过，
        // 随后宿主才发现旧租约已崩、于是换了新进程——此时 ATTACH 无规则可走。
        fake.crash(first.pid);
        cmd_runtime_health(&state, &first.lease).unwrap();
        state.registry.report_event(&id, Event::RetryOk).unwrap();
        state.registry.report_event(&id, Event::Attach).unwrap();
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Running);
        assert!(state.registry.runtime_needs_restart(&id), "旧租约已崩 → 需要新进程");

        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_ne!(second.pid, first.pid, "必须真的起了新进程");
        assert_eq!(fake.call_count(), 2);

        let entry = state.registry.find(&id).unwrap();
        assert_eq!(
            entry.state.state,
            LifecycleState::Running,
            "RUNNING + ATTACH 无规则 → 保持原状（已在目标态），不伪造别的状态"
        );
        assert_eq!(
            entry.state.illegal_transitions, 1,
            "非法迁移必须被状态机计数（可查），而不是被静默吞掉"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // R8 §1：Sink 抽象的验收标准——「换一个 sink 实现，命令结果就变」
    //
    // 这一节回答的是本次抽象**唯一**要回答的问题：平台部分能不能被替换，
    // 替换之后命令的行为是不是真的跟着变。凡是"注入假实现后命令仍然给出旧结果"
    // 的地方，都是抽象没接上。
    // ──────────────────────────────────────────────────────────────────────
    mod sink_tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// 用假 sink 装配一份状态（**这就是"可在测试里注入"的落地点**）。
        ///
        /// 三个字段都是 `SubstrateState` 的 `pub` 字段，在 `Arc` 化之前替换；
        /// 生产侧的同一个位置在 `tauri.rs::command_state_with_dir_and_config`。
        fn state_with_sinks(
            window: Arc<dyn WindowSink>,
            dialog: Arc<dyn DialogSink>,
            deep_link: Arc<dyn DeepLinkSink>,
        ) -> PluginRuntimeState {
            let mut substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
            substrate.window_sink = window;
            substrate.dialog_sink = dialog;
            substrate.deep_link_sink = deep_link;
            PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
        }

        fn noop_only(window: Arc<dyn WindowSink>) -> PluginRuntimeState {
            state_with_sinks(
                window,
                Arc::new(NoopDialogSink),
                Arc::new(NoopDeepLinkSink),
            )
        }

        /// 记录型窗口假实现：证明命令**真的**转调了 sink（而不是自己吞掉）。
        #[derive(Default)]
        struct RecordingWindowSink {
            ops: Mutex<Vec<WindowOpRecord>>,
            /// 构造出的窗口规格（[`WindowSink::create`] 的入参逐字段留痕）。
            specs: Mutex<Vec<WindowCreateSpec>>,
            /// `relaunch` / `create` 的返回值（真实宿主由平台结果决定）。
            relaunch_ok: bool,
            create_ok: bool,
        }

        impl RecordingWindowSink {
            fn with_results(relaunch_ok: bool, create_ok: bool) -> Self {
                Self {
                    relaunch_ok,
                    create_ok,
                    ..Default::default()
                }
            }

            fn push(&self, op: &'static str, label: Option<&str>, detail: String) {
                self.ops.lock().push(WindowOpRecord {
                    op,
                    label: label.map(str::to_string),
                    detail,
                });
            }

            fn op_names(&self) -> Vec<&'static str> {
                self.ops.lock().iter().map(|r| r.op).collect()
            }

            fn specs(&self) -> Vec<WindowCreateSpec> {
                self.specs.lock().clone()
            }
        }

        impl WindowSink for RecordingWindowSink {
            fn minimize(&self, label: &str) -> HostResult<()> {
                self.push("minimize", Some(label), String::new());
                Ok(())
            }

            fn maximize(&self, label: &str) -> HostResult<()> {
                self.push("maximize", Some(label), String::new());
                Ok(())
            }

            fn restore(&self, label: &str) -> HostResult<()> {
                self.push("restore", Some(label), String::new());
                Ok(())
            }

            fn close(&self, label: &str) -> HostResult<()> {
                self.push("close", Some(label), String::new());
                Ok(())
            }

            fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()> {
                self.push("set_position", Some(label), format!("x={x},y={y}"));
                Ok(())
            }

            fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()> {
                self.push("set_size", Some(label), format!("width={width},height={height}"));
                Ok(())
            }

            fn quit(&self) -> HostResult<()> {
                self.push("quit", None, String::new());
                Ok(())
            }

            fn relaunch(&self) -> HostResult<bool> {
                self.push("relaunch", None, String::new());
                Ok(self.relaunch_ok)
            }

            fn create(&self, spec: &WindowCreateSpec) -> HostResult<bool> {
                self.push("create", Some(&spec.label), spec.url.clone());
                self.specs.lock().push(spec.clone());
                Ok(self.create_ok)
            }
        }

        /// 记录型对话框假实现。
        struct RecordingDialogSink {
            open: Option<String>,
            save: Option<String>,
            confirm: bool,
            messages: Mutex<Vec<(String, String, String)>>,
        }

        impl DialogSink for RecordingDialogSink {
            fn open_file(&self, _multiple: bool, _directory: bool) -> HostResult<Option<String>> {
                Ok(self.open.clone())
            }

            fn save_file(&self, _default_name: Option<&str>) -> HostResult<Option<String>> {
                Ok(self.save.clone())
            }

            fn message(&self, kind: &str, title: &str, body: &str) -> HostResult<()> {
                self.messages
                    .lock()
                    .push((kind.to_string(), title.to_string(), body.to_string()));
                Ok(())
            }

            fn confirm(&self, _title: &str, _body: &str) -> HostResult<bool> {
                Ok(self.confirm)
            }
        }

        /// 记录型深链接假实现。
        #[derive(Default)]
        struct RecordingDeepLinkSink {
            calls: Mutex<Vec<String>>,
            native: bool,
        }

        impl DeepLinkSink for RecordingDeepLinkSink {
            fn register(&self, protocol: &str) -> HostResult<()> {
                self.calls.lock().push(format!("register:{protocol}"));
                Ok(())
            }

            fn unregister(&self, protocol: &str) -> HostResult<()> {
                self.calls.lock().push(format!("unregister:{protocol}"));
                Ok(())
            }

            fn native_supported(&self) -> bool {
                self.native
            }
        }

        /// 顺序观察型窗口假实现：在 `relaunch` 被调用的**那一刻**读注册表标志。
        ///
        /// 注册表句柄在装配后注入（`OnceLock`）：sink 先于 `PluginRuntimeState`
        /// 存在，而 `with_substrate` 会自建注册表——只有事后注入才能观察到"命令真的
        /// 用的那份注册表"。
        #[derive(Default)]
        struct RelaunchOrderingSink {
            registry: std::sync::OnceLock<Arc<Registry>>,
            observed: Mutex<Option<bool>>,
            calls: AtomicUsize,
        }

        impl RelaunchOrderingSink {
            fn bind_registry(&self, registry: Arc<Registry>) {
                let _ = self.registry.set(registry);
            }

            fn observed_flag(&self) -> Option<bool> {
                *self.observed.lock()
            }

            fn calls(&self) -> usize {
                self.calls.load(Ordering::SeqCst)
            }
        }

        impl WindowSink for RelaunchOrderingSink {
            fn minimize(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn maximize(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn restore(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn close(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn set_position(&self, _label: &str, _x: i32, _y: i32) -> HostResult<()> {
                Ok(())
            }
            fn set_size(&self, _label: &str, _w: u32, _h: u32) -> HostResult<()> {
                Ok(())
            }
            fn quit(&self) -> HostResult<()> {
                Ok(())
            }

            fn relaunch(&self) -> HostResult<bool> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                // 观察点：此刻注册表里 `com.a` 的安全模式标志是什么？
                // true = 对账已经在 relaunch **之前**跑完了。
                let flag = self.registry.get().and_then(|r| {
                    r.list_all()
                        .into_iter()
                        .find(|p| p.id == "com.a")
                        .map(|p| p.disabled_by_safemode)
                });
                *self.observed.lock() = flag;
                Ok(true)
            }

            fn create(&self, _spec: &WindowCreateSpec) -> HostResult<bool> {
                Ok(true)
            }
        }

        /// 轮 11 第三批：深链接注册是**应用级**操作（写 `shell_ext` + 声明 topic，且会
        /// 注销上一个协议），所以仅主窗；越权时 **sink 一个字节都不许被碰**——否则
        /// "拒绝"仍然会留下副作用（旧协议的 OS 关联被摘掉）。
        #[test]
        fn deep_link_registration_is_main_window_only_and_never_touches_the_sink_when_denied() {
            let sink = Arc::new(RecordingDeepLinkSink::default());
            let state = state_with_sinks(
                Arc::new(MemoryWindowSink::new()),
                Arc::new(NoopDialogSink),
                sink.clone(),
            );

            // 对照基线：主窗注册 → sink 真的被调用。
            cmd_deep_link_register_as(&Caller::MainWindow, &state, "tauron".to_string()).unwrap();
            assert_eq!(sink.calls.lock().clone(), vec!["register:tauron"]);

            // 插件接管 → 拒绝；sink 停在上一行（没有 register:evil，也没有 unregister:tauron）。
            let err = cmd_deep_link_register_as(&plugin_caller("p.a"), &state, "evil".to_string())
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
            assert_eq!(
                sink.calls.lock().clone(),
                vec!["register:tauron"],
                "拒绝路径不得碰 sink（连旧值的 unregister 都不许发生）"
            );
            assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("tauron"));

            // 主窗换协议 → 恰好是"先注销旧值、再注册新值"（证明拒绝不是把命令整体关死）。
            cmd_deep_link_register_as(&Caller::MainWindow, &state, "other".to_string()).unwrap();
            assert_eq!(
                sink.calls.lock().clone(),
                vec!["register:tauron", "unregister:tauron", "register:other"]
            );
        }

        /// `tauron.rs` 的平台操作搬到 sink 之后，命令包装器必须真的调它。
        #[test]
        fn swapping_the_window_sink_changes_the_command_result() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            let label = "plugin-com.a";

            cmd_window_minimize(&state, label).unwrap();
            cmd_window_maximize(&state, label).unwrap();
            cmd_window_restore(&state, label).unwrap();
            cmd_window_close(&state, label).unwrap();
            cmd_window_set_position(&state, label, 7, 9).unwrap();
            cmd_window_set_size(&state, label, 800, 600).unwrap();
            cmd_window_quit(&state).unwrap();
            // 真实宿主才会返回 true；假实现返回 true 时命令必须如实转达。
            let relaunch = cmd_window_relaunch_as(&Caller::MainWindow, &state).unwrap();

            assert_eq!(
                sink.op_names(),
                vec![
                    "minimize",
                    "maximize",
                    "restore",
                    "close",
                    "set_position",
                    "set_size",
                    "quit",
                    "relaunch",
                ],
                "窗口命令必须逐条转调 sink（顺序 = 调用顺序）"
            );
            assert_eq!(
                sink.ops
                    .lock()
                    .iter()
                    .filter(|r| r.label.as_deref() == Some(label))
                    .count(),
                6,
                "带目标的六条操作必须把宿主侧的 label 原样传给 sink"
            );
            assert!(
                relaunch.relaunch_requested,
                "sink 说重启已请求 → 命令必须如实回传 true"
            );

            // 反向对照：**换回缺省（进程内）sink，同一命令给出不同结果**。
            let degraded = CommandState::new();
            let out = cmd_window_relaunch_as(&Caller::MainWindow, &degraded).unwrap();
            assert!(
                !out.relaunch_requested,
                "缺省 MemoryWindowSink 没有重启原语，必须如实回 false"
            );
            assert!(
                out.reason.is_some(),
                "降级必须带原因，而不是一个无解释的 false"
            );
        }

        /// 注入返回 `Some("x")` 的假对话框实现 → `cmd_dialog_open` 必须回 `Some("x")`，
        /// 而不是恒 `None`。这条就是"换 sink 即换结果"在对话框面的形态。
        #[test]
        fn swapping_the_dialog_sink_changes_the_command_result() {
            let dialog = Arc::new(RecordingDialogSink {
                open: Some("/tmp/picked.png".to_string()),
                save: Some("/tmp/out.txt".to_string()),
                confirm: true,
                messages: Mutex::new(Vec::new()),
            });
            let state = state_with_sinks(
                Arc::new(MemoryWindowSink::new()),
                dialog.clone(),
                Arc::new(NoopDeepLinkSink),
            );

            assert_eq!(
                cmd_dialog_open(&state, false, false).unwrap().as_deref(),
                Some("/tmp/picked.png")
            );
            assert_eq!(
                cmd_dialog_save(&state, Some("a.txt")).unwrap().as_deref(),
                Some("/tmp/out.txt")
            );
            assert!(cmd_dialog_confirm(&state, "t", "m").unwrap());
            cmd_dialog_message(&state, "t", "m", Some("warning")).unwrap();
            assert_eq!(
                dialog.messages.lock().clone(),
                vec![("warning".to_string(), "t".to_string(), "m".to_string())],
                "kind/title/body 必须原样到达 sink（kind 已过闭集校验）"
            );

            // 反向对照：缺省 NoopDialogSink 对同一入参返回取消。
            let degraded = CommandState::new();
            assert_eq!(cmd_dialog_open(&degraded, false, false).unwrap(), None);
            assert!(!cmd_dialog_confirm(&degraded, "t", "m").unwrap());
        }

        /// **顺序不变量**：`host_window_relaunch` 必须先对账、后重启。
        ///
        /// 反序会把「注册表标志未与引擎判定对齐」的状态带进下一次启动，而阶段计数是
        /// 跨进程持久化的 → 下一轮仍按旧判定走 → 重启循环。这里用"relaunch 被调用
        /// 那一刻的注册表标志"作为顺序证据。
        #[test]
        fn window_relaunch_reconciles_before_requesting_restart() {
            let sink = Arc::new(RelaunchOrderingSink::default());
            let state = noop_only(sink.clone());
            sink.bind_registry(state.registry.clone());
            state
                .registry
                .install(&empty_index(), test_manifest("com.a"))
                .unwrap();

            // 直接在恢复引擎上造出「安全模式 + 非必需插件应被禁用」的判定，
            // 但**不**经过 `cmd_recover_report`（它会自己对账）——否则注册表标志
            // 早已写回，本用例就没有判别力了。
            {
                let mut engine = state.recovery.lock();
                engine.register_plugin("com.a");
                engine.record_boot_failure(None);
                engine.record_boot_failure(None);
            }
            assert_eq!(
                cmd_recover_boot(&state).unwrap()["phase"],
                "safemode",
                "前置：引擎判定必须是安全模式"
            );
            assert!(
                !find_summary(&state, "com.a").unwrap().disabled_by_safemode,
                "前置：注册表标志此时尚未对账 —— 这正是「重启前必须对账」的场景"
            );

            let out = cmd_window_relaunch_as(&Caller::MainWindow, &state).unwrap();
            assert_eq!(out.reconcile.scanned, 1);
            assert_eq!(
                out.reconcile.entered, 1,
                "对账必须真的把安全模式判定补发到注册表（不是空跑）"
            );
            assert!(out.relaunch_requested);
            assert_eq!(
                sink.observed_flag(),
                Some(true),
                "relaunch 被调用时注册表标志必须已经写上 —— 顺序反了这里会是 false"
            );
            assert_eq!(sink.calls(), 1, "重启只请求一次");

            // 判别力对照：同样的前置状态，只调 sink（跳过对账）→ 观察点读到 false。
            // 没有这一条，上面的断言无法区分「顺序正确」与「观察点恒为 true」。
            let control_sink = Arc::new(RelaunchOrderingSink::default());
            let control = noop_only(control_sink.clone());
            control_sink.bind_registry(control.registry.clone());
            control
                .registry
                .install(&empty_index(), test_manifest("com.a"))
                .unwrap();
            {
                let mut engine = control.recovery.lock();
                engine.register_plugin("com.a");
                engine.record_boot_failure(None);
                engine.record_boot_failure(None);
            }
            assert_eq!(
                cmd_recover_boot(&control).unwrap()["phase"],
                "safemode",
                "对照前置：同一份安全模式判定"
            );
            // 只重启、不对账：这正是"顺序写反"会造成的局面。
            control.window_sink.relaunch().unwrap();
            assert_eq!(
                control_sink.observed_flag(),
                Some(false),
                "对照组必须读到未对账的 false —— 证明观察点真的有判别力"
            );
        }

        /// 重启是应用级动作：插件主体一律拒绝，且**拒绝路径不产生任何副作用**。
        #[test]
        fn window_relaunch_is_main_window_only_and_leaves_no_side_effect() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());

            let err = cmd_window_relaunch_as(&Caller::Plugin("com.a".to_string()), &state)
                .expect_err("插件主体不得触发应用重启");
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
            assert!(
                sink.op_names().is_empty(),
                "拒绝路径不得调用 sink（判定必须在转调之前）"
            );

            // 畸形 label 构造不出主体（`Caller::from_label` 就拒了）——这是同一道门。
            assert_eq!(
                Caller::from_label("plugin-not a valid id").unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
        }

        /// 缺省装配（非 Tauri 宿主）必须如实说"没有重启"，而不是假装重启成功。
        #[test]
        fn builtin_sinks_are_honest_about_what_they_do_not_do() {
            let state = CommandState::new();
            let out = cmd_window_relaunch_as(&Caller::MainWindow, &state).unwrap();
            assert!(!out.relaunch_requested);
            assert!(out.reason.as_deref().is_some_and(|r| r.contains("没有")));
            assert_eq!(out.reconcile, PhaseReconcileOutcome::default());

            // 三个内建降级实现的自我描述逐条钉住。
            let window = MemoryWindowSink::new();
            let spec = WindowCreateSpec {
                label: "plugin-com.a".to_string(),
                plugin_id: "com.a".to_string(),
                url: "ui/panel.html".to_string(),
                title: "A".to_string(),
                width: 100,
                height: 100,
            };
            assert!(!window.relaunch().unwrap(), "进程内 sink 没有重启原语");
            assert!(!window.create(&spec).unwrap(), "进程内 sink 不创建窗口");
            assert!(window.recorded("create") && window.recorded("relaunch"), "但要留痕");
            assert!(!NoopDeepLinkSink.native_supported(), "无 OS 级深链接注册");
            assert!(!NoopDialogSink.confirm("t", "m").unwrap());
        }

        /// `host_window_create`：label 由核心铸造、URL 来自 manifest、缺省值固定。
        #[test]
        fn window_create_mints_plugin_label_and_resolves_the_manifest() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            state
                .registry
                .install(&empty_index(), manifest_with_ui("com.a", "面板 A", "ui/panel.html"))
                .unwrap();

            let out = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .unwrap();

            assert_eq!(out.label, "plugin-com.a", "label 恒为 plugin-<id>（身份载体）");
            assert_eq!(out.plugin_id, "com.a");
            assert!(out.created, "sink 说创建成功 → 如实回传");
            assert!(out.reason.is_none());

            let specs = sink.specs();
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].label, "plugin-com.a");
            assert_eq!(specs[0].url, "ui/panel.html", "URL 只能来自 manifest.entry.ui");
            assert_eq!(specs[0].title, "面板 A", "标题缺省 = manifest 的 name");
            assert_eq!(specs[0].width, DEFAULT_WINDOW_WIDTH);
            assert_eq!(specs[0].height, DEFAULT_WINDOW_HEIGHT);

            // 入参可覆盖标题与尺寸（线参数的最小面）。
            cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    title: Some("自定义".to_string()),
                    width: Some(640),
                    height: Some(480),
                },
            )
            .unwrap();
            let specs = sink.specs();
            assert_eq!(specs[1].title, "自定义");
            assert_eq!((specs[1].width, specs[1].height), (640, 480));
        }

        /// 不存在的插件 id：拒绝，且**不触碰 sink**（不创建半态窗口）。
        #[test]
        fn window_create_rejects_unknown_plugin_before_touching_the_sink() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            let err = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.ghost".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .expect_err("未注册插件不得开窗");
            assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
            assert!(sink.specs().is_empty(), "拒绝路径不得创建任何窗口");
        }

        /// 未声明 `entry.ui` 的插件：拒绝（不编造默认 URL），同样不触碰 sink。
        #[test]
        fn window_create_refuses_plugins_without_a_ui_entry() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            state
                .registry
                .install(&empty_index(), test_manifest("com.a"))
                .unwrap();
            let err = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .expect_err("没有 UI 入口就没有可加载的页面");
            assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
            assert!(sink.specs().is_empty());
        }

        /// 开窗是宿主管理面动作：插件主体一律拒绝。
        #[test]
        fn window_create_is_main_window_only() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            state
                .registry
                .install(&empty_index(), manifest_with_ui("com.a", "A", "ui/a.html"))
                .unwrap();
            let err = cmd_window_create_as(
                &Caller::Plugin("com.a".to_string()),
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .expect_err("插件不得为自己/别人开窗");
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
            assert!(sink.specs().is_empty());
        }

        /// 缺省装配下 `create` 返回降级结果（`created: false` + 原因），不假装成功。
        #[test]
        fn window_create_degrades_honestly_without_a_native_sink() {
            let state = CommandState::new();
            state
                .registry
                .install(&empty_index(), manifest_with_ui("com.a", "A", "ui/a.html"))
                .unwrap();
            let out = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .unwrap();
            assert!(!out.created);
            assert_eq!(out.label, "plugin-com.a", "label 仍然铸造出来（清理钩子按它回收）");
            assert!(
                out.reason
                    .as_deref()
                    .is_some_and(|r| r.contains("没有") && r.contains("创建任何窗口")),
                "降级必须写明没有创建窗口：{:?}",
                out.reason
            );
        }

        /// 深链接：sink 被真实驱动；**换协议时先注销旧值**。
        #[test]
        fn deep_link_register_drives_the_sink_and_releases_the_previous_protocol() {
            let sink = Arc::new(RecordingDeepLinkSink {
                native: false,
                ..Default::default()
            });
            let state = state_with_sinks(
                Arc::new(MemoryWindowSink::new()),
                Arc::new(NoopDialogSink),
                sink.clone(),
            );

            cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
            cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
            cmd_deep_link_register(&state, "other".to_string()).unwrap();

            assert_eq!(
                sink.calls.lock().clone(),
                vec![
                    "register:tauron".to_string(),
                    "register:tauron".to_string(),
                    // 换协议：先注销旧值，再注册新值（旧关联不得留在系统里）。
                    "unregister:tauron".to_string(),
                    "register:other".to_string(),
                ],
                "同值重复注册不注销；换值必须先注销"
            );
            assert_eq!(
                state.shell_ext.lock().deep_link_protocol.as_deref(),
                Some("other")
            );
            // 降级事实必须可机读：本仓没有说话算数的 OS 级注册。
            assert!(!sink.native_supported());
        }

        /// 带 UI 入口的 manifest（窗口创建需要 `entry.ui`）。
        fn manifest_with_ui(id: &str, name: &str, ui: &str) -> PluginManifest {
            let mut manifest = test_manifest(id);
            manifest.name = name.to_string();
            manifest.entry.ui = Some(ui.to_string());
            manifest
        }
    }
}
