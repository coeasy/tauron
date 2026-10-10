// T-7 拆分：从 `lib.rs` 逐字节纯搬移的通知域命令族（写端 notify + 读端 notifications_list/read）。
//
// 八项连号搬移：`cmd_notify` / `cmd_notify_as`（写端，署名必须是自己）、
// `visible_notifications`（读端可见性过滤）/ `notifications_list_payload`（**私有助手**，快照构造，
// 仅被同族 list/list_as 调用，故不外泄）、`cmd_notifications_list(_as)`、`cmd_notifications_read(_as)`。
// `NotificationRecord` 兼容结构体与 `require_self_plugin_scope` 判定函数分属其它域、仍留 `lib.rs`。
//
// `use super::*;` 令搬来的命令体原样调用 crate-root 私助手（`guard`、`require_self_plugin_scope`）
// 与顶层 use（`NotifyStore`/`DispatchSink`/`MAX_NOTIFY_*` 等）；全部 pub 项按原名再导出，保
// `crate::cmd_notify*`（tauri.rs 接线）与内联测试裸名解析不变。

use super::*;

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
        if title.len() > MAX_NOTIFY_TITLE_BYTES {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "通知标题超出字节预算（{} > {MAX_NOTIFY_TITLE_BYTES}），拒绝入队",
                    title.len()
                ),
            ));
        }
        if body.len() > MAX_NOTIFY_BODY_BYTES {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "通知正文超出字节预算（{} > {MAX_NOTIFY_BODY_BYTES}），拒绝入队",
                    body.len()
                ),
            ));
        }
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
pub fn visible_notifications<'a>(caller: &Caller, store: &'a NotifyStore) -> Vec<&'a NotifyEntry> {
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
        let plugin_usage: std::collections::BTreeMap<String, usize> = store
            .group_counts()
            .into_iter()
            .filter_map(|(group, count)| {
                let plugin_id = group.strip_prefix("plugin:")?;
                match mine {
                    None => Some((plugin_id.to_string(), count)),
                    Some(caller_plugin) if caller_plugin == plugin_id => {
                        Some((plugin_id.to_string(), count))
                    }
                    Some(_) => None,
                }
            })
            .collect();
        Ok(serde_json::json!({
            "unread": unread,
            "total": total,
            "items": items,
            "dispatchLog": dispatch_log,
            "capacity": store.capacity(),
            "pluginCapacity": store.plugin_capacity(),
            "pluginUsage": plugin_usage,
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
            let owner = state.notify_store.lock().get(nid).map(|e| e.plugin_id.clone());
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
