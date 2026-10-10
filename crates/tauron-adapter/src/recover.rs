// tauron-adapter · 恢复域命令族（T-7 逐字节纯搬移 · 十五片）。
//
// 本模块由 crate 根 lib.rs 的「恢复引擎」段整体纯搬移而来（旧 lib 7036–7448 行，一段连续 413 行），
// 未改动任何一行代码——函数体、注释、doc、cfg 全部逐字节保真，仅把归属文件从 lib.rs 换到 recover.rs。
// 内容：私有 helper `recovery_boot_payload`（恢复引擎线形态读数，boot/report/trial 三条命令共用，仅此族
// 使用故随族私有不外泄）+ 六条 `pub fn`：cmd_recover_boot / cmd_recover_boot_as / cmd_recover_report /
// cmd_recover_report_as / cmd_recover_trial_enable / cmd_recover_trial_enable_as。
//
// 刻意留在 lib.rs 的是被其它域共用的边界/对账助手：`reconcile_recovery_phase`（registry/admin、runtime
// 也调用）、`persist_recovery_engine`（runtime spawn 也调用）、以及 subsystem 边界 `reconcile_events_boundary` /
// `reconcile_registry_boundary`。本族命令经 `use super::*;` 原样可见 crate 根的这些私助手与 `guard` /
// `require_main_window` / `PluginRuntimeState` / `SubstrateState` / `Caller` / `HostResult` / `ErrorCode` /
// `Event` 等类型，零 `pub(crate)` 放宽、纯可见性搬移。

use super::*;

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
    let required_plugins: Vec<String> = engine.required_plugins().iter().cloned().collect();

    let store = state.recovery_store.lock();
    let persistence = serde_json::json!({
        "enabled": store.is_enabled(),
        "dir": store.dir().map(|p| p.to_string_lossy().into_owned()),
        "bootInFlight": store.in_flight,
        "lastError": store.last_error.clone(),
    });
    // 崩溃后诊断上下文：`RecoveryStore` 从标记文件读回来的**上一轮**关键事件
    // 摘要（含本次 `load` 识别到的那次崩溃）。锁定在 store 锁内物化。
    let last_context: Vec<serde_json::Value> =
        store.last_context.iter().map(boot_context_entry_wire).collect();

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
/// 恢复引擎由三处驱动，均在本文件中：
/// - [`CommandState::with_adapter_config`]：启动时载入持久化标记并做崩溃检测；
/// - [`cmd_recover_report`]：应用上报启动结果（驱动信号）；
/// - [`cmd_recover_trial_enable`]：安全模式内逐个试验性启用插件。
///
/// 阶段判定落到插件侧的唯一途径是 [`reconcile_recovery_phase`]：它把引擎的判定
/// 补发成 `SafemodeEnter` / `SafemodeExit`，由注册表状态机写入
/// `disabled_by_safemode`——那才是 `<oc-plugin-manager>` 角标读取的值。
///
/// A91（轮 47）：本命令同时是故障子系统的**确定性修复时刻**（主窗在恢复流里调用
/// 它即触发）——事件总线会话态清零（订阅/审批/队列；topic 声明保留）、注册表修不了
/// 则如实隔离。修复成败不改变本命令的读取语义；结果落在各自边界状态里
/// （`host_resource_stats` 的 `faults` 块与后续被拒命令的错误文案都看得到）。
pub fn cmd_recover_boot(state: &SubstrateState) -> HostResult<serde_json::Value> {
    guard("recover_boot", || {
        let _ = reconcile_events_boundary(state);
        let _ = reconcile_registry_boundary(state);
        Ok(recovery_boot_payload(state))
    })?
}

/// `host_recover_boot` 的**带身份判定**版本（轮 12）。
///
/// 返回的是**应用级**恢复态势（是否进安全模式、失败计数、阶段判定）。插件读它 =
/// 侦察宿主当前是否处于降级运行态（并可据此挑时机），和 `host_production_doctor`
/// 被定为 privileged 的理由同一类：部署/运行状态情报面。启动时对账与
/// `cmd_recover_trial_enable` 的驱动全在主窗侧，插件侧没有合法读取场景。
pub fn cmd_recover_boot_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_recover_boot")?;
    cmd_recover_boot(state)
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
        result["outcome"] = serde_json::json!(if is_success { "success" } else { "failure" });
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

        // Recovery safety policy takes precedence over action de-duplication. Once a plugin has
        // exhausted its one-shot trial budget, a replay in the same incident must remain a hard
        // E_PLUGIN_DISABLED rejection rather than being hidden as a de-duplicated success.
        if state.recovery.lock().counter().trial_exhausted(id.as_str()) {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_DISABLED,
                format!("插件 `{id}` 已试验失败，回落 disabled-by-safemode，不可再次试启"),
            ));
        }

        // 必须先取出结果再分支：`match state.recovery.lock().trial_enable(..)` 会让
        // 临时 guard 被临时值生命周期延长规则持有到整个 match 结束，于是错误分支里
        // 再次 `state.recovery.lock()` 就是自死锁（`parking_lot` 不重入，表现为
        // 空转而非挂起）。
        // V4 A92: real RecoveryExecutor wiring. The incident sequence comes from the latest
        // persisted failure context, so duplicate user clicks in one incident share a key while
        // a later independent incident gets a new sequence.
        let (action, should_execute) = {
            let mut engine = state.recovery.lock();
            let incident_seq =
                engine.last_context().map(|entry| entry.ts).unwrap_or_else(recovery::now_ms);
            let action = RecoveryAction {
                plugin_id: id.as_str().to_string(),
                action_kind: "trial-enable".to_string(),
                idempotency_key: RecoveryAction::idempotency_key(
                    id.as_str(),
                    "trial-enable",
                    incident_seq,
                ),
            };
            let should_execute = engine.execute_action(&action).map_err(|e| {
                HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("恢复动作正在执行或不可开始：{e}"),
                )
            })?;
            (action, should_execute)
        };

        if !should_execute {
            let payload = recovery_boot_payload(state);
            let mut result = payload.clone();
            result["pluginId"] = serde_json::json!(id.as_str());
            result["engineAction"] = serde_json::json!("trialEnable:deduplicated");
            result["phaseReconcile"] = serde_json::to_value(phase_reconcile).unwrap_or_default();
            return Ok(result);
        }

        let previous_plugin_state = state.recovery.lock().plugin_state(id.as_str());
        let trial = state.recovery.lock().trial_enable(id.as_str());
        match trial {
            Ok(()) => {}
            Err(tauron_recovery::RecoveryError::RestrictedInSafemode) => {
                state.recovery.lock().abort_recovery();
                let phase = state.recovery.lock().decide_boot_phase().as_str();
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("插件 `{id}` 只能在安全模式下试验性启用；当前阶段是 `{phase}`"),
                ));
            }
            Err(tauron_recovery::RecoveryError::TrialExhausted(pid)) => {
                state.recovery.lock().abort_recovery();
                return Err(HostError::new(
                    ErrorCode::E_PLUGIN_DISABLED,
                    format!("插件 `{pid}` 已试验失败，回落 disabled-by-safemode，不可再次试启"),
                ));
            }
            Err(e) => {
                state.recovery.lock().abort_recovery();
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("试验性启用失败：{e}"),
                ));
            }
        }

        // 试启成功后让注册表状态机跟着走 D28 试启迁移（§4.14：引擎负责判，
        // 状态机负责执行）。外部副作用成功后才写 EffectRecord + commit action；
        // 拒绝则 abort，允许同一 incident 的下一次显式请求安全重放。
        // Apply the external registry effect. A TransitionOutcome with `illegal=true` is a
        // rejection even though report_event itself returned Ok; treating it as committed would
        // persist an EffectRecord for an effect that never happened.
        let registry_effect: HostResult<(String, bool)> =
            if let Some(out) = state.registry.find(&id).map(|e| e.state.state) {
                use tauron_host::lifecycle::State;
                if out == State::Enabled || out == State::Running {
                    // Desired external state already exists; the recovery action is satisfied,
                    // but this invocation did not create a new external effect.
                    Ok(("alreadyEnabled".to_string(), false))
                } else {
                    match state.registry.report_event(&id, Event::TrialEnable) {
                        Ok(o) if !o.illegal => Ok((format!("trialEnable:{}", o.to.as_str()), true)),
                        Ok(o) => Err(HostError::new(
                            ErrorCode::E_STATE_INVALID_TRANSITION,
                            format!(
                                "恢复试启被注册表状态机拒绝：{} -> {}",
                                o.from.as_str(),
                                o.to.as_str()
                            ),
                        )),
                        Err(error) => Err(error),
                    }
                }
            } else {
                // Engine-only recovery remains supported for a plugin that is not yet in the
                // runtime registry. There is no external effect to record in that case.
                Ok(("notInRegistry".to_string(), false))
            };

        let (engine_action, external_effect_applied) = match registry_effect {
            Ok(result) => result,
            Err(error) => {
                {
                    let mut engine = state.recovery.lock();
                    engine.rollback_trial_enable_if_unchanged(id.as_str(), previous_plugin_state);
                    engine.abort_recovery();
                }
                persist_recovery_engine(state);
                return Err(error);
            }
        };

        {
            let mut engine = state.recovery.lock();
            if external_effect_applied {
                engine.record_effect(EffectRecord {
                    effect_id: format!("registry:{}:{}", id.as_str(), action.idempotency_key),
                    action_key: action.idempotency_key.clone(),
                    ts: recovery::now_ms(),
                });
            }
            engine.complete_recovery();
        }

        // Persist the action/effect ledger together with the recovery engine. This keeps
        // deduplication valid across process restart, not only inside one in-memory session.
        persist_recovery_engine(state);

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
