// T-7 二十二片：进程运行时 spawn/health 命令族——4 条 `pub fn cmd_runtime_*` 从
// `lib.rs` 逐字节纯搬移到本模块。文件中部刻意**不搬**的同族私助手
// （`attach_after_spawn` / `refuse_exhausted_crash_budget` / `runtime_health_report` /
// `deliver_runtime_crash` / `proc_error_to_host`）以及跨域共用助手
// （`persist_recovery_engine`）留在 `lib.rs`——它们被 lib 内联测试按裸名直接调用
// （`runtime_health_report`@test、`proc_error_to_host`@test、`attach_after_spawn`@test），
// 或还被留在 lib 的助手调用；本模块经下面的 `use super::*;` 看到 crate 根的这些**私有**
// 项（子模块可见祖先私有项），故无需任何 `pub(crate)` 放宽。搬走的 spawn 命令体仍引用
// crate 根的 `admin_gate`（host_runtime_spawn 在 AUDITED_ADMIN_COMMANDS 内），wire-gate 的
// 审计咽喉点扫描源已并集本模块以复原 routed 覆盖面。
use super::*;

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
        if state.deployment_mode == tauron_host::DeploymentMode::Production
            && !matches!(
                state.proc_runtime.sandbox_descriptor().enforcement,
                tauron_proc::ProcessSandboxEnforcement::Hard
            )
        {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                "production runtime spawn requires hard ProcessSandboxProvider enforcement",
            ));
        }

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

        let mut cfg = profile.to_spawn_config(&binary_path);
        // 运维注入的环境变量在**这里**落地（`ClientConfig.env_overrides` 的落点）：
        // 放在 `to_spawn_config` 之后、校验与启动之前，因此它既参与 §4.7 的启动前
        // 校验，也确实到达子进程（`CommandSpawner` 的 `.envs()`）。
        apply_host_env_overrides(&mut cfg.env, &state.plugin_env_overrides);
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
        if state.registry.runtime_needs_restart(&id)
            && state.proc_runtime.is_crash_exceeded(plugin_id)
        {
            return Err(refuse_exhausted_crash_budget(state, &id, plugin_id));
        }

        // `started` 是**锁内**判定出来的"本次真的起了新进程"（幂等返回既有活租约
        // 时为 false），据此决定要不要把状态推到 RUNNING——见 `attach_after_spawn`。
        let (handle, started) = state.registry.runtime_ensure_lease(&id, || {
            state.proc_runtime.spawner().spawn(&cfg).map(|p| p.pid).map_err(proc_error_to_host)
        })?;
        if started {
            attach_after_spawn(state, &id)?;
            // 0.4-A1：真起了新进程就注册 stdout 帧接收器，闭合「宿主 → sidecar →
            // 回帧 → 结算」链路。sidecar 回帧（含 `callId`）经读线程路由到这里，
            // 由 `ProcessFrameSinkImpl` 调 `settle_call`。幂等：重复注册只是覆盖。
            // 返回 `false` = 进程在读线程侧已 EOF（注册被拒绝以防泄漏表条目）：
            // 此时 stdin 登记同样已被清掉，后续 `write_frame` 会以 `NotFound`
            // 如实失败——无需在此造错误，死亡探测路径会回收租约。立即退出的
            // sidecar 走这条竞态属正常，不视为错误。
            let registered = state.proc_runtime.spawner().register_frame_sink(
                handle.pid,
                Arc::new(crate::process_delivery::ProcessFrameSinkImpl::new(
                    state.registry.clone(),
                )),
            );
            let _ = registered; // 迟到 EOF：死亡探测兜底，见上注释
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
    admin_gate(&state.substrate, caller, "host_runtime_spawn")?;
    cmd_runtime_spawn(state, plugin_id, profile)
}
pub fn cmd_runtime_health(state: &PluginRuntimeState, lease: &str) -> HostResult<RuntimeHealth> {
    guard("runtime_health", || {
        let entry = state.registry.runtime_lease(lease)?;
        let status = state.proc_runtime.spawner().status(entry.pid);
        let alive = matches!(status, tauron_proc::ProcessStatus::Alive);

        // Unknown is a degraded diagnostic state, not a crash: never consume crash budget unless
        // the process is positively observed Exited.
        if matches!(status, tauron_proc::ProcessStatus::Exited)
            && state.registry.runtime_mark_crashed(lease)?
        {
            state.proc_runtime.record_crash(&entry.plugin_id);
            deliver_runtime_crash(state, &entry.plugin_id);
        }

        let crashes = state.proc_runtime.crash_count(&entry.plugin_id);
        let lifecycle = PluginId::new(&entry.plugin_id)
            .ok()
            .and_then(|id| state.registry.find(&id))
            .map(|plugin| plugin.state.state);
        let health = runtime_health_report(status, lifecycle, crashes);

        Ok(RuntimeHealth {
            alive,
            status,
            pid: entry.pid,
            crashes,
            consecutive_failures: state.recovery.lock().counter().consecutive_failures,
            reap: state.registry.runtime_reap_stats(),
            health,
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
