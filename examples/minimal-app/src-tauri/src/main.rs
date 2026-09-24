// tauron 集成示例 —— Tauri 主进程入口。
//
// 注册方式说明（Tauri v2 ACL 语义，见 tauron-adapter 文档）：
// - 本示例采用 **应用层 root 注册**（`invoke_handler(tauron_generate_handler![])`）：
//   命令以裸名暴露（`host_window_minimize`…），前端 `TauriBackend` 配
//   `commandPrefix: ''`；`capabilities/default.json` 授予 `core:default`
//   （Tauri v2 的规则是「不匹配任何 capability 的 webview 完全没有 IPC 访问」，
//   所以这个文件不是可选项）。
// - 生产客户端可用 `.plugin(tauron_adapter::tauri::init())`（`plugin:tauron|*`
//   路由），必须同步为 `tauron` 插件配置 capability 授予所需命令；`crates/tauron-adapter`
//   目前**没有** `permissions/` 定义，那条路由在启用能力检查时缺权限条目
//   （部署配置缺口，见 docs/architecture/app-layer-wire.md §1 告警）。
//
// 两种形态（编译期可选，`--features substrate-only`）：
// - **默认（全量）**：底座 + 插件运行时（54 条命令 + 装配器宏消费者）。
// - **substrate-only（验收标准 1 的证据）**：只 `manage(SubstrateState)` +
//   `tauron_substrate_handler![]`（38 条底座命令），**不**建 `PluginRuntimeState`、
//   **不**注册装配器消费者。`cargo check --features substrate-only` 即证明
//   "harness 类宿主只要底座也能编译并功能完整（shell + i18n + notify + recovery +
//   settings）"——这是方案 §9-1 那条验收标准的可复核证据。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let builder = tauri::Builder::default();

    // ── 默认：底座 + 插件运行时 ──
    #[cfg(not(feature = "substrate-only"))]
    let builder = builder
        // CommandState 需在 setup 阶段 manage（窗口/剪贴板/更新状态载体）。
        // state_init 同时打开恢复持久化（app_config_dir）——§4.14 崩溃检测的
        // 前提；前端必须每轮启动上报一次 host_recover_report，见 src/main.ts。
        .plugin(tauron_adapter::tauri::state_init())
        // 54 条 host_* 命令：root 注册（裸名调用，不依赖插件 ACL capability）
        .invoke_handler(tauron_adapter::tauron_generate_handler![])
        // R8 §2：装配器宏的**真实消费者**（见下方 app_commands 模块）。
        .plugin(app_commands::init())
        // 窗口销毁 → 回收该插件的订阅/队列与 pending 调用（§8-3 零悬挂订阅）。
        // `tauri::plugin::Builder` 没有窗口事件钩子，只有 App Builder 有，
        // 因此这条回收必须由宿主在这里接线，装配方不接就是泄漏。
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                use tauri::Manager;
                let state = window.state::<tauron_adapter::CommandState>();
                tauron_adapter::tauri::cleanup_closed_window(&state, window.label());
            }
        });

    // ── substrate-only：只有底座状态与底座命令族 ──
    #[cfg(feature = "substrate-only")]
    let builder = builder
        .setup(|app| {
            use tauri::Manager;
            // 底座状态自己 manage（不建插件注册表、不建 contributes 账本）。
            // 按生产形状传数据目录：不给的话崩溃检测只有内存态，进程一退计数即失、
            // 安全模式永不触发（见 docs/integration/incremental-adoption.md §1.2）。
            let config = tauron_adapter::AdapterConfig {
                recovery_data_dir: app.path().app_config_dir().ok(),
                ..tauron_adapter::AdapterConfig::default()
            };
            app.manage(tauron_adapter::SubstrateState::with_adapter_config(&config));
            Ok(())
        })
        .invoke_handler(tauron_adapter::tauron_substrate_handler![]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauron minimal app");
}

// ── R8 §2：装配器宏的真实消费者（仅全量形态）────────────────────────────
//
// 这个模块扮演"第三方 Tauri 插件"：它有自己的命令，想用宿主的能力面，但**不想**
// （也不能）自己重写那套包装——结构化错误过桥、panic 守卫、参数注入。用它来说明
// `tauri_plugin_as_host_command!` 解决的问题：
//
//   Rust 侧只有**一条**出路能让 JS 的 `invoke()` 不挂死：`#[tauri::command]` 包装里
//   必须带 panic 守卫（Tauri 不捕获 panic），错误必须序列化成宿主约定的
//   `{ code, message, retryable }` 对象（字符串转储会丢 `message`，前端只能正则反抠）。
//   这两件事的实现在 `tauron-adapter` 里是私有的（`to_tauri_err` 私有、
//   `host_command` 需要正确的调用姿态），第三方**抄不到**，抄错了**也不报错**——
//   只会在运行期表现为"前端卡死"或"错误信息丢失"。本宏把这条出路固化成一条展开式：
//   插件作者只写业务体（返回 `HostResult<T>`），包装由宏生成。
//
// 为什么消费者必须**真的存在**：宏的展开只有被 `generate_handler!` 收下时才被编译。
// 没有消费者的宏，语法错误与签名不匹配会一直藏到别人第一次使用时。本文件随
// `cargo check`（examples/minimal-app/src-tauri）一起编译，等于把宏钉进示例构建。
//
// substrate-only 形态下本模块整体不编译（那个宿主不跑插件运行时，"第三方插件命令"
// 在它上面没有落点）；全量形态（battery 默认跑的形态）才编译它。
#[cfg(not(feature = "substrate-only"))]
mod app_commands {
    use tauri::{State, WebviewWindow};
    use tauron_adapter::{Caller, HostResult, SubstrateState};

    /// 本插件命令的业务体：只写这一半，返回 `HostResult<T>`。
    ///
    /// 身份**从 `window.label()` 解析**（不信任任何前端入参）——与宿主命令同一口径：
    /// 包装器只负责把 Tauri 注入的 `window` 递进来，判定由 `Caller::from_label` 做。
    fn stats_body(
        state: State<'_, SubstrateState>,
        window: WebviewWindow,
        note: Option<String>,
    ) -> HostResult<serde_json::Value> {
        let caller = Caller::from_label(window.label())?;
        let i18n = tauron_adapter::cmd_i18n_stats(&state)?;
        Ok(serde_json::json!({
            "label": window.label(),
            "caller": match caller {
                Caller::MainWindow => "main".to_string(),
                Caller::Plugin(id) => id,
            },
            "note": note,
            "i18n": i18n,
        }))
    }

    // 装配：一条声明 = 一条宿主形态命令（包装由宏生成，`my_stats` 可直接进 handler）。
    tauron_adapter::tauri_plugin_as_host_command! {
        plugin = "my-plugin",
        cmd = pub my_stats(
            state: State<'_, SubstrateState>,
            window: WebviewWindow,
            note: Option<String>
        ) -> serde_json::Value,
        handler = stats_body,
    }

    /// 插件初始化：命令注册在**本插件自己的** invoke_handler 上
    /// （路由 `plugin:my-plugin|my_stats`），仍走宿主的 origin 门。
    pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
        tauri::plugin::Builder::new("my-plugin")
            .invoke_handler(tauron_adapter::tauri::origin_gated_handler(
                tauri::generate_handler![my_stats],
            ))
            .build()
    }
}

// ── 框架层（第三方客户端只需信封协议时）──────────────────────────────────
// plugin_invoke / plugin_cancel / plugin_emit 三条命令的同类 root 注册：
//
//     .invoke_handler(tauron_shell::tauron_generate_handler![])
//
// 与 tauron-core `createTauriBackend()`（裸命令名）直接匹配。
