// T-7 拆分：从 `lib.rs` 逐字节纯搬移的资源诊断命令族。
//
// 两个命令都只读 `PluginRuntimeState`（经 `state.registry` / `state.bus` /
// `state.notify_store` / 四个 `*_fault` 字段），不点名任何随族搬迁的结构体；
// `host_resource_stats` 的读数是「随读回收 + 逐资源表取数」，第二事实源闸门
// （wire-gate V7 §9）钉死读数必须来自注册表本身——搬移不改这条线形。
//
// `use super::*;` 令搬来的命令体原样调用 crate-root 私有助手（guard、
// require_main_window）与顶层 use 导入；两命令全为 pub，按原名再导出，保
// `crate::cmd_resource_stats(_as)`（tauri.rs 接线与内联测试裸名）解析不变。

use super::*;

/// 主窗资源快照：展示每种有界资源的总占用/上限与逐插件占用。
/// 插件明细取自对应资源表本身，不维护第二份计数状态。
pub fn cmd_resource_stats(state: &PluginRuntimeState) -> HostResult<serde_json::Value> {
    guard("resource_stats", || {
        // 诊断读 = 回收点：过期 pending / 调用图边 / 准入令牌此前只在 pending
        // 触顶（begin_call）时被顺带回收；主窗若不轮询诊断，N 笔未结束的调用
        // 会占用内存直到重启。这里随读回收一次，快照也因此与回收事实一致。
        state.registry.gc_expired();
        // V7 §7/§9 的驱动腿：终止失败的 pid 在这里被再试一轮（有界，见
        // `RuntimeTable::retry_pending_reaps`）。刻意与 `gc_expired` 同址——
        // 主窗轮询诊断本来就是这个宿主的"心跳"，重试没有驱动方等于没有重试。
        state.registry.runtime_retry_pending_reaps();
        let plugins = state.registry.list_all();
        let plugin_ids: Vec<String> = plugins.iter().map(|plugin| plugin.id.to_string()).collect();
        let pending_used = state.registry.pending_len();
        let pending_limit = state.registry.config().max_pending_calls;
        let streams_used = state.registry.stream_active_total();
        // V7 §9 leak gate：代际台账读数（有界性必须能被宿主 UI 查到，不能只活在 Rust 测试里）。
        let generations = state.registry.runtime_generation_stats();
        // V7 §9 leak gate：回收留痕（含重试队列与终态证据）同样上线。
        let reap = state.registry.runtime_reap_stats();

        let subscriptions = state.bus.lock();
        let subscriptions_used = subscriptions.subscription_total();
        let subscription_usage: std::collections::BTreeMap<String, usize> = plugin_ids
            .iter()
            .map(|id| (id.clone(), subscriptions.subscription_count(id)))
            .collect();
        drop(subscriptions);

        let notifications = state.notify_store.lock();
        let notification_usage = notifications.group_counts();
        let notification_capacity = notifications.capacity();
        let notification_plugin_capacity = notifications.plugin_capacity();
        let notification_used = notifications.len();
        let notification_evictions = notifications.eviction_counts();
        let notification_evictions_total = notifications.eviction_total();
        let notification_counts: std::collections::BTreeMap<String, usize> = notification_usage
            .into_iter()
            .filter_map(|(key, count)| {
                key.strip_prefix("plugin:").map(|id| (id.to_string(), count))
            })
            .collect();
        drop(notifications);

        let plugin_rows: Vec<serde_json::Value> = plugin_ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "pluginId": id,
                    "pendingCalls": state.registry.pending_for(id),
                    "streams": state.registry.stream_active_for(id),
                    "subscriptions": subscription_usage.get(id).copied().unwrap_or_default(),
                    "notifications": notification_counts.get(id).copied().unwrap_or_default(),
                    "notificationEvictions": notification_evictions.get(id).copied().unwrap_or_default(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "global": {
                "pendingCalls": { "used": pending_used, "limit": pending_limit },
                "streams": { "used": streams_used, "limit": tauron_host::stream::MAX_STREAMS },
                "subscriptions": {
                    "used": subscriptions_used,
                    "limit": tauron_host::eventbus::MAX_SUBSCRIPTIONS
                },
                "notifications": {
                    "used": notification_used,
                    "limit": notification_capacity,
                    "perPluginLimit": notification_plugin_capacity,
                    "evictedTotal": notification_evictions_total
                },
                "generations": generations,
                "reap": reap
            },
            "perPlugin": {
                "pendingCallsLimit": tauron_host::registry::MAX_PENDING_PER_PLUGIN,
                "streamsLimit": tauron_host::stream::MAX_STREAMS_PER_PLUGIN,
                "subscriptionsLimit": tauron_host::eventbus::MAX_SUBSCRIPTIONS_PER_PLUGIN,
                "plugins": plugin_rows
            },
            "faults": {
                "settings": {
                    "state": state.settings_fault.lock().state(),
                    "generation": state.settings_fault.lock().generation(),
                    "lastFault": state.settings_fault.lock().last_fault().cloned()
                },
                // A91（轮 47）：三个子系统边界的可观测面。主窗据此判「有没有子域
                // 在拒绝服务、修复尝试得出了什么结论」，而不是等下一次命令报错。
                "events": {
                    "state": state.events_fault.lock().state(),
                    "generation": state.events_fault.lock().generation(),
                    "lastFault": state.events_fault.lock().last_fault().cloned()
                },
                "approval": {
                    "state": state.review_fault.lock().state(),
                    "generation": state.review_fault.lock().generation(),
                    "lastFault": state.review_fault.lock().last_fault().cloned()
                },
                "registry": {
                    "state": state.registry_fault.lock().state(),
                    "generation": state.registry_fault.lock().generation(),
                    "lastFault": state.registry_fault.lock().last_fault().cloned()
                }
            }
        }))
    })?
}

/// 资源诊断数据只允许主窗读取，避免暴露其他插件的活动量。
pub fn cmd_resource_stats_as(
    caller: &Caller,
    state: &PluginRuntimeState,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_resource_stats")?;
    cmd_resource_stats(state)
}
