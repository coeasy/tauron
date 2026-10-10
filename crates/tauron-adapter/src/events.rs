// T-7 拆分：从 `lib.rs` 逐字节纯搬移的事件总线（events）命令族。
//
// 八条 `cmd_events_*`（publish / publish_with_causation / subscribe / unsubscribe /
// approve_as / revoke_as / approvals_as / drain）+ 审批事实结构 `EventApproval`。源侧
// 非连续——段一与段二之间原隔着 `cmd_production_doctor_as` 及其私助手 `production_doctor_report`
// （二十一片起已搬入 `doctor.rs`）与共享订阅回收助手 `prune_subscription_groups`（仍分属
// runtime/registry 域、留在 `lib.rs`），故两段各取原样、以空行拼接——本片
// 搬移 publish…approvals_as 连续块与紧随其后的 drain 两段。命令体调用的
// `run_events_boundary`（私助手、仍被 lib 侧定义与台账 needle 钉住）、`admin_gate` /
// `require_main_window` 均 crate-root 项，随 `use super::*;` 原样可见；返回类型
// `PublishResult`/`SubscribeOutcome`/`Frame`/`ChannelKind` 亦由 crate-root 提供、不随本族搬。
// 全部按原名再导出，保 `crate::cmd_events_*` 与 `crate::EventApproval`（tauri.rs 接线
// 与 lib.rs 内联测试裸名）解析不变。

use super::*;

/// `host_events_publish`：发布事件（self 档）。
///
/// 使用可靠语义（`publish_request`）：溢出返回结构化错误。
pub fn cmd_events_publish(
    state: &SubstrateState,
    publisher: &str,
    topic: &str,
    payload: serde_json::Value,
) -> HostResult<PublishResult> {
    cmd_events_publish_with_causation(state, publisher, topic, payload, None)
}

/// V4 A78 event publication with an optional parent causation context.
pub fn cmd_events_publish_with_causation(
    state: &SubstrateState,
    publisher: &str,
    topic: &str,
    payload: serde_json::Value,
    causation: Option<&tauron_host::EventCausation>,
) -> HostResult<PublishResult> {
    run_events_boundary(state, "events_publish", || {
        let bus = state.bus.lock();
        bus.publish_request_with_causation(publisher, topic, payload, causation)
    })
}

/// `host_events_subscribe`：订阅事件（self 档）。
pub fn cmd_events_subscribe(
    state: &SubstrateState,
    subscriber: &str,
    window: &str,
    topic: &str,
) -> HostResult<SubscribeOutcome> {
    run_events_boundary(state, "events_subscribe", || {
        let bus = state.bus.lock();
        bus.subscribe(subscriber, window, topic)
    })
}

/// `host_events_unsubscribe`：退订事件（self 档）。
pub fn cmd_events_unsubscribe(state: &SubstrateState, token: &str) -> HostResult<()> {
    run_events_boundary(state, "events_unsubscribe", || {
        let bus = state.bus.lock();
        bus.unsubscribe(token)
    })
}

/// 一条 EventBus 私有 topic 审批事实。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventApproval {
    pub subscriber: String,
    pub topic: String,
}

/// 主窗批准插件订阅私有 topic。
pub fn cmd_events_approve_as(
    caller: &Caller,
    state: &SubstrateState,
    subscriber: &str,
    topic: &str,
) -> HostResult<()> {
    admin_gate(state, caller, "host_events_approve")?;
    if subscriber.trim().is_empty() || topic.trim().is_empty() {
        return Err(HostError::new(ErrorCode::E_AUTH_DENIED, "subscriber 与 topic 均不可为空"));
    }
    // `approve` 自身的拒绝（未声明 topic / 审批表满）原样上线——闸门只把 panic
    // 转成 `E_HOST_PANIC`，不吞业务错误。
    run_events_boundary(state, "events_approve", || state.bus.lock().approve(subscriber, topic))
}

/// 主窗撤销插件私有 topic 审批。幂等；返回是否真实删除。
///
/// A81：撤销即失效——该 `(subscriber, topic)` 的既有订阅一并退订、队列里该 topic 的
/// 待取帧一并作废（详见 `tauron_host::eventbus::EventBus::revoke`）。
pub fn cmd_events_revoke_as(
    caller: &Caller,
    state: &SubstrateState,
    subscriber: &str,
    topic: &str,
) -> HostResult<bool> {
    admin_gate(state, caller, "host_events_revoke")?;
    run_events_boundary(state, "events_revoke", || Ok(state.bus.lock().revoke(subscriber, topic)))
}

/// 主窗读取全部 EventBus 审批事实。
pub fn cmd_events_approvals_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<Vec<EventApproval>> {
    require_main_window(caller, "host_events_approvals")?;
    run_events_boundary(state, "events_approvals", || {
        Ok(state
            .bus
            .lock()
            .approvals()
            .into_iter()
            .map(|(subscriber, topic)| EventApproval { subscriber, topic })
            .collect())
    })
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
    run_events_boundary(state, "events_drain", || {
        let bus = state.bus.lock();
        bus.drain(subscriber, parsed_kind)
    })
}
