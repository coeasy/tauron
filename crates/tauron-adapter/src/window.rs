// T-7 拆分：从 `lib.rs` 逐字节纯搬移的窗口管理命令族。
//
// `use super::*;` 让子模块看到 crate 根的全部项——含私有助手（guard、
// require_main_window、unsupported_body 等）与顶层 use 导入，因此搬来的
// 命令体可原样调用；`validate_plugin_ui_entry` 是本族私有助手，不外泄。

use super::*;

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

/// `host_window_quit` 的**带身份判定**版本（轮 12）。
///
/// # 为什么这条必须有判定
///
/// 它关的不是调用方自己的窗口，而是**整个应用**（`TauriWindowSink::quit` 走
/// `app.exit(0)`）：插件窗调一次就把宿主连同所有邻居插件一起关掉，是现成的 DoS
/// 开关。同族的 `host_window_relaunch`（重启）在轮 11 已判为仅主窗——quit 比
/// relaunch 更彻底，没有理由反而不判。其余窗口命令（close/minimize/maximize/
/// restore/set_position/set_size）传的是 `window.label()`（调用方**自己**的窗口），
/// 天然按主体隔离，所以不需要判定。
pub fn cmd_window_quit_as(caller: &Caller, state: &SubstrateState) -> HostResult<()> {
    require_main_window(caller, "host_window_quit")?;
    cmd_window_quit(state)
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
pub fn cmd_window_relaunch_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<WindowRelaunchOutcome> {
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
        validate_plugin_ui_entry(&url)?;

        let spec = WindowCreateSpec {
            // 身份 label 的**唯一**铸造点（与 `PluginIdentity::new` 同一约定）。
            label: format!("plugin-{id}"),
            plugin_id: id.as_str().to_string(),
            url,
            title: req.title.clone().unwrap_or_else(|| entry.manifest.name.clone()),
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

/// `host_window_set_position`：移动调用方窗口到 `(x, y)`（逻辑像素/DIP）。
///
/// **两件事，都要做**（R8）：
/// 1. 写 `shell_ext.window_rect` —— 进程内的几何账本（**无生产读取方**；
///    持久化与恢复在前端 `window-state.ts`，见该字段的接入状态说明）；
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

/// `entry.ui` is consumed by Tauri as an application asset path when no on-disk
/// plugin installation root is configured. Keep it a portable relative path on
/// every host OS; in particular, reject Windows separators and drive prefixes
/// even when validation runs on Unix.
fn validate_plugin_ui_entry(raw: &str) -> HostResult<()> {
    let invalid = raw.trim().is_empty()
        || raw.starts_with('/')
        || raw.starts_with('\\')
        || raw.contains('\\')
        || raw.contains(':')
        || raw.split('/').any(|part| part.is_empty() || part == "." || part == "..");
    if invalid {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "插件 entry.ui 必须是安全的相对路径".to_string(),
        ));
    }
    Ok(())
}
