//! 通知环形缓冲容量的**判定与生效**（轮 57）。
//!
//! 这里只有一个事实：设置键 `notifications.capacity` 是什么意思、以及怎么让它生效。
//! 它单独成模块而不是留在 `lib.rs`，理由与 `menu_routes.rs` 同类——`lib.rs` 的条目预算
//! 已接近上限（见 `contracts/adapter-domain-ownership.json`），新增能力默认开新域文件；
//! 而这组逻辑恰好是**纯判定 + 一个生效动作**，没有命令调度、没有 `_as` 身份判定、也没有
//! 账本写入点需要跟调用方同读一个文件。
//!
//! 分工写清楚，别处不得再抄：
//!
//! - **判定**：[`parse_notify_capacity`]（值 → 容量）是唯一解释这个键的地方，宿主别处
//!   都不许再出现一份范围检查或字面量；
//! - **校验入口**：按键写走 [`parse_notify_capacity`]（`cmd_settings_set` 先判后写），
//!   整份文档写走 [`validate_notify_capacity_document`]（`host_settings_adopt_legacy`），
//!   迁移后的事实走 [`notify_capacity_from_settings`]（`cmd_settings_migrate` 落盘前）——
//!   三个入口共用同一个判定，没有第二套范围；
//! - **生效**：[`apply_notify_capacity`] 是 `NotifyStore::trim_to` 在本仓库唯一的消费者
//!   （另一个调用点在装配期，那时 `NotifyStore` 还没进 `SubstrateState`）；每条把设置文档
//!   成功落盘的命令路径在落盘后各调一次（set / adopt_legacy / migrate）；
//! - **默认**：[`NOTIFICATIONS_CAPACITY_DEFAULT`] 同时是「键缺席」和「磁盘上是坏值」时
//!   用的那一个数，装配处只从这里取值。

use tauron_settings::SettingsStore;

use crate::{settings_path, settings_to_host_error, SubstrateState, HOST_SETTINGS_NAMESPACE};
use tauron_host::{ErrorCode, HostError, HostResult};

/// 通知环形缓冲容量的设置键（轮 57 接线）。
///
/// 键归主窗命名空间——插件写宿主键由 [`crate::require_settings_key_scope`] 拒。
pub(crate) const NOTIFICATIONS_CAPACITY_KEY: &str = "notifications.capacity";

/// 可配置上界。`NotifyStore::new` 只拒 0、**没有上界**，所以一次误写就能把宿主变成
/// 准无界缓冲；上界只在这里补一份，别处不得再抄。
pub(crate) const NOTIFICATIONS_CAPACITY_MAX: usize = 4096;

/// 内建默认容量：键缺席（或磁盘上是非法值）时环形缓冲用的就是这个数。
/// 装配处只允许从这里取值，别再抄一个字面量——「默认」必须和「缺席」是同一个事实。
pub(crate) const NOTIFICATIONS_CAPACITY_DEFAULT: usize = 512;

fn invalid_notify_capacity(detail: &str) -> HostError {
    HostError::new(
        ErrorCode::E_INVALID_MANIFEST,
        format!(
            "{NOTIFICATIONS_CAPACITY_KEY} {detail}（合法范围 1..={NOTIFICATIONS_CAPACITY_MAX}）"
        ),
    )
}

/// 把一个设置值解释成通知容量：必须是正整数且不超上界。
pub(crate) fn parse_notify_capacity(value: &serde_json::Value) -> HostResult<usize> {
    let raw = value
        .as_i64()
        .ok_or_else(|| invalid_notify_capacity(&format!("必须是整数（收到 {value}）")))?;
    if raw < 1 {
        return Err(invalid_notify_capacity("必须为正数"));
    }
    let capacity =
        usize::try_from(raw).map_err(|_| invalid_notify_capacity("超出本机可表示范围"))?;
    if capacity > NOTIFICATIONS_CAPACITY_MAX {
        return Err(invalid_notify_capacity("超过上界"));
    }
    Ok(capacity)
}

/// 整份文档写入路径的按键校验：文档里出现了这个键，就必须按**同一个判定**解释。
///
/// 为什么需要它（轮 59）：`host_settings_set` 自轮 57 起先判后写，但两条**整份文档**写路径
/// （接手旧版文档、schema 迁移）绕过了它——同一个键两套标准，坏值能经旧文档进磁盘。
///
/// 查两种拼法而不是只查一种：v1 文档的键是**裸键**（`notifications.capacity`），v2 层的键是
/// [`crate::settings_path`] 转义出来的**转义键**，`migrate_host_settings_v1_to_v2` 做的正是
/// 逐键改名。两种形态都是同一个键，判定仍然只有 [`parse_notify_capacity`] 一处。
pub(crate) fn validate_notify_capacity_document(doc: &serde_json::Value) -> HostResult<()> {
    for spelling in [NOTIFICATIONS_CAPACITY_KEY, settings_path(NOTIFICATIONS_CAPACITY_KEY).as_str()]
    {
        if let Some(value) = doc.get(spelling) {
            parse_notify_capacity(value)?;
        }
    }
    Ok(())
}

/// 从设置存储读容量事实。键缺席（或显式 `null`）＝**不表态**，返回 `None`，
/// 由调用方保留内建默认——缺键不是错误，读路径不校验 schema。
pub(crate) fn notify_capacity_from_settings(store: &SettingsStore) -> HostResult<Option<usize>> {
    let path = settings_path(NOTIFICATIONS_CAPACITY_KEY);
    match store.get_key(HOST_SETTINGS_NAMESPACE, &path).map_err(settings_to_host_error)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => parse_notify_capacity(&value).map(Some),
    }
}

/// 落盘／载入之后的生效点：把已经确认过的容量交给 `NotifyStore`。
///
/// `trim_to` 一个动作承担两件事——收缩（驱逐最旧、更新 `evictions`/`unread` 记账）
/// 与放大（只改上限），所以宿主对「用户把通知容量调小」不再有第二条实现路径。
/// 两把锁**串行**取（先 `settings` 读出、出临界区，再 `notify_store`），绝不跨落盘持有。
pub(crate) fn apply_notify_capacity(state: &SubstrateState) -> HostResult<Option<usize>> {
    let capacity = notify_capacity_from_settings(&state.settings.lock())?;
    if let Some(capacity) = capacity {
        state.notify_store.lock().trim_to(capacity).map_err(|error| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("通知容量 {capacity} 无法生效：{error:?}"),
            )
        })?;
    }
    Ok(capacity)
}
