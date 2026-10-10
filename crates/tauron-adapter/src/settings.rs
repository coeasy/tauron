// 本模块是 Tauron 宿主适配器设置命令族的按原名**逐字节纯搬移**（T-7 十二→十三片）。
//
// 搬运动机：把 `lib.rs` 里一整段设置命令族（`host_settings_*` 4 条 pub + 1 条私助手 +
// `cmd_settings_*` 8 条命令：读端 3 条（get/get_as 主窗与带身份版本 + wire 转调核心）
// 写端 2 条（set/set_as）+ 接手迁移 3 条（adopt_legacy/adopt_legacy_as/migrate/migrate_as）
// ——注意迁移 4 条：migrate 与 migrate_as 独立）从 lib.rs 顶格迁出，lib.rs 通过文件尾
// `pub use settings::{…}` 按原名再导出，保 `crate::cmd_settings_*` 由 tauri.rs 接线、
// `crate::host_settings_*` 由 lib.rs 同族 caller 解析、以及 lib.rs 内联测试裸名调用**全部**不变。
//
// 与 `notify.rs` 片不同，本片**只搬命令段（Block B）**：设置域的基础设施（`HOST_SETTINGS_*`
// 常量、`atomic_write_settings_file`/`persist_settings_doc`/`load_settings_doc` 等私助手、
// `run_settings_boundary` 边界闸、`settings_to_host_error` 等错误映射、以及被 `notify_capacity.rs`
// 以 `crate::settings_to_host_error`/`crate::settings_path` 路径调用的两个跨文件消费者）都留在
// `lib.rs` 里，与恢复引擎 / 事件审批注册表三个子系统的边界助手（`subsystem_fault_to_host_error`/
// `run_subsystem_boundary`/`run_events_boundary`/`run_review_boundary`/`run_registry_boundary`/
// `begin_subsystem_reconcile`/`reconcile_*_boundary`）保持同文件邻近——那些边界助手非设置族，
// 若一并搬走会造成 lib.rs 内跨模块引用面膨胀。
//
// 因此本片是**纯搬移，零语义变更**：可见性、函数体、doc 注释、模块归属之外的一切均逐字保留；
// `use super::*;` 让搬来的命令看到 crate 根私助手（`settings_to_host_error` /
// `persist_settings_doc` / `stage_settings_rollback_image` / `load_settings_rollback_image` /
// `atomic_write_settings_file` / `clear_settings_rollback_image` / `commit_settings_change` /
// `install_host_settings_schema` / `host_settings_schema` / `migrate_host_settings_v1_to_v2` /
// `run_settings_boundary` / `reconcile_settings_boundary` / `settings_to_host_error` /
// `settings_fault_to_host_error` / `settings_rollback_path` / `settings_write_lock` /
// `require_settings_key_scope` / `plugin_settings_namespace` / `settings_path` / `settings_wire_key` /
// `HOST_SETTINGS_NAMESPACE` / `HOST_SETTINGS_CHANGED_TOPIC` / `HOST_SETTINGS_SCHEMA_V1` /
// `HOST_SETTINGS_SCHEMA_V2` / `HOST_SETTINGS_FILE` / `HOST_SETTINGS_ROLLBACK_FILE` 等），
// 无需 `pub(crate)` 扩权，也无需改任何调用点。

use super::*;

/// **接手一份旧版（v1）宿主设置文档**（R7-2：settings 可迁移）。
///
/// 用途：宿主从磁盘读到的旧版配置走这里进 Store。写入用户层并把**数据版本**
/// 标成 [`HOST_SETTINGS_SCHEMA_V1`]——不标注的话 [`host_settings_migrate`]
/// 无从知道起点，只能拒绝迁移（宁可不迁，也不猜）。
///
/// `doc` 必须是对象（键 = 设置键，值 = 设置值）；否则返回 `E_INVALID_MANIFEST`，
/// 不静默退化成空文档。
///
/// **轮 59 起的按键语义校验**：文档里出现 [`NOTIFICATIONS_CAPACITY_KEY`] 时，值必须先过
/// [`validate_notify_capacity_document`]（判定仍是 `parse_notify_capacity` 那一处），非法
/// 文档在**碰 Store 之前**就被拒——内存与磁盘都保持原样。轮 57 只在 `host_settings_set`
/// 上加了先判后写，这条整份接手路径因此是坏值的门。
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
    validate_notify_capacity_document(&doc)?;
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
fn host_settings_migrate_transaction(state: &SubstrateState) -> HostResult<MigrationReceipt> {
    state
        .settings
        .lock()
        .migrate_transactional(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V2)
        .map_err(settings_to_host_error)
}

pub fn host_settings_migrate(state: &SubstrateState) -> HostResult<usize> {
    Ok(host_settings_migrate_transaction(state)?.steps())
}

/// 当前宿主设置的数据版本（**宿主嵌入方的 Rust 诊断读口**；`None` = 既无数据也无标注）。
///
/// 线上没有对应命令：插件侧能看到的最接近事实是镜像帧里的 `revision`，
/// 版本本身要靠 `host_settings_migrate` 的返回步数推断。
pub fn host_settings_data_version(state: &SubstrateState) -> Option<String> {
    state.settings.lock().data_version(HOST_SETTINGS_NAMESPACE).map(str::to_string)
}

/// Monotonic revision of successfully committed host-setting writes.
///
/// Rust embedding diagnostic read-out — there is **no** wire command for it;
/// wire consumers observe the same counter through the `revision` field of
/// [`HOST_SETTINGS_CHANGED_TOPIC`] frames.
pub fn host_settings_revision(state: &SubstrateState) -> u64 {
    state.settings.lock().revision()
}

/// `host_settings_get`：读取设置。
///
/// **线形不变**（前端契约）：入参 `key: string`，返回任意 JSON；未写过的键
/// 返回 `Null`（不是报错）。读路径不校验 schema——缺键不是错误。
pub fn cmd_settings_get(state: &SubstrateState, key: &str) -> HostResult<serde_json::Value> {
    run_settings_boundary(state, "settings_get", || {
        let path = settings_path(key);
        let store = state.settings.lock();
        let value =
            store.get_key(HOST_SETTINGS_NAMESPACE, &path).map_err(settings_to_host_error)?;
        Ok(value.unwrap_or(serde_json::Value::Null))
    })
}

/// `host_settings_set`：写入设置。
///
/// **线形不变**（前端契约）：入参 `key: string, value: any`，返回 `()`。
/// 写入**经 [`SettingsStore`]**（不再是裸 `HashMap`）：键会被编码成合法点路径，
/// 值要过命名空间 schema，落盘形态由 Store 的四层结构决定。
///
/// **轮 57 起的唯一带语义的键**：[`NOTIFICATIONS_CAPACITY_KEY`]
/// （`notifications.capacity`，主窗专属——插件写宿主键由 [`require_settings_key_scope`] 拒）。
/// 合法性由 [`parse_notify_capacity`] 一处判定，范围 `1..=NOTIFICATIONS_CAPACITY_MAX`，
/// **在落盘之前**判：非法值返回 `E_INVALID_MANIFEST`，设置文档与环形缓冲都保持原样
/// （没有“撤销键”的命令面入口，所以 `null` 也拒；要回内建默认 [`NOTIFICATIONS_CAPACITY_DEFAULT`]
/// 就显式把默认值写回来）。
/// 落盘并镜像提交成功后由 [`apply_notify_capacity`] 立即生效（收缩=驱逐最旧并同步
/// `unread`/分组记账，放大=只抬上限）；`host_notifications_list` 的 `capacity` 字段
/// 报的就是**生效后的真值**。宿主启动时会再读一次磁盘（见装配处的
/// `notify_capacity_from_settings`），重启不会回到内建默认；磁盘上若有越界写入的坏值
/// （轮 59 起两条整份写路径都按同一判定先判后写，剩下的真实来路是**用户手改文件**）
/// **不拒绝启动**：留痕并沿用内建默认。
pub fn cmd_settings_set(
    state: &SubstrateState,
    key: &str,
    value: serde_json::Value,
) -> HostResult<()> {
    run_settings_boundary(state, "settings_set", || {
        if key.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "设置键不能为空（调用方可能传了 undefined/null）".to_string(),
            ));
        }

        // 轮 57：**先判后写**。通知容量的合法性只由 [`parse_notify_capacity`] 一处说了算，
        // 非法值在写租约与落盘之前就被拒——磁盘上永远不会留下一个「装配期读回来会报错」
        // 的容量键，装配分支因此只用来兜「用户手改文件」这种越界写入。
        if key == NOTIFICATIONS_CAPACITY_KEY {
            parse_notify_capacity(&value)?;
        }

        // 单写者事务。两件事必须分清：**写租约故意横跨落盘**（否则另一个 writer 能插进
        // 「内存已改、磁盘未改」的半提交窗口，回滚时会把对方的写入一起吃掉）；被禁止的是
        // `settings` 互斥量进 `persist_settings_doc`——它在下面的块作用域里就出了临界区，
        // 落盘只消费一次性快照。
        let _write = state.settings_write_lock.lock();
        let path = settings_path(key);
        let (before, event) = {
            let mut store = state.settings.lock();
            let before = store.snapshot_all();
            let (_op, event) = store
                .set_deferred(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_NAMESPACE, &path, &value)
                .map_err(settings_to_host_error)?;
            (before, event)
        };

        if let Err(error) = persist_settings_doc(state) {
            state.settings.lock().restore(&before);
            return Err(error);
        }

        // Watchers only observe a revision after durable persistence succeeded.
        commit_settings_change(state, event);

        // 轮 57：容量键的**运行期生效点**——落盘与提交都成功了才动环形缓冲。
        // 放在 persist 之前会出现「缓冲已经缩了、磁盘写失败回滚」的不一致（下次启动又长回去）。
        // 这里取 `settings` 锁不会与落盘交叉：`_write` 是写租约，不是设置互斥量。
        if key == NOTIFICATIONS_CAPACITY_KEY {
            apply_notify_capacity(state)?;
        }
        Ok(())
    })
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
pub fn cmd_settings_adopt_legacy(state: &SubstrateState, doc: serde_json::Value) -> HostResult<()> {
    run_settings_boundary(state, "settings_adopt_legacy", || {
        let _write = state.settings_write_lock.lock();
        let before = state.settings.lock().snapshot_all();
        host_settings_adopt_legacy(state, doc)?;
        if let Err(error) = persist_settings_doc(state) {
            state.settings.lock().restore(&before);
            return Err(error);
        }
        // 轮 59：与 `cmd_settings_set` 同一姿态——**落盘成功了才生效**。整份接手一条
        // 裸键文档时这个读口拿不到容量（键的转义由迁移负责），此时它是空操作；
        // 文档已经用转义键时，容量立刻按磁盘事实生效，不等重启。
        apply_notify_capacity(state)?;
        Ok(())
    })
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
/// 轮 59 起「过不了校验」还包括**带语义的键**：迁移把 v1 裸键转成装配期读得到的转义键，
/// 这一步正是坏容量变成「可读事实」的时刻，所以落盘前按 [`parse_notify_capacity`] 同一
/// 判定解释一次，解释不了就回滚整个迁移；成功落盘后容量立即生效（不等重启）。
///
/// 与 [`cmd_settings_adopt_legacy`] 一样受 [`guard`] 保护——迁移要走 schema
/// 编译与用户数据改写，一旦 panic 必须是 `E_HOST_PANIC` 而不是把 panic  unwind
/// 穿过 IPC 边界。
pub fn cmd_settings_migrate(state: &SubstrateState) -> HostResult<usize> {
    reconcile_settings_boundary(state)?;
    run_settings_boundary(state, "settings_migrate", || {
        let _write = state.settings_write_lock.lock();
        // A101：镜像必须在迁移**之前**取——迁移完成后内存里已经没有迁移前的用户层了
        // （`MigrationReceipt.before` 就是同一份，但它是私有字段，且不覆盖其它命名空间）。
        let before = state.settings.lock().snapshot_all();
        let receipt = host_settings_migrate_transaction(state)?;
        let steps = receipt.steps();
        if receipt.changed() {
            // 轮 59：**迁移是坏值变成「可读事实」的那一步**——裸键 `notifications.capacity`
            // 转成转义键之后，装配期才读得到它。所以在写镜像与落盘之前先按同一个判定解释一次，
            // 非法就整体回滚（与下面两条失败分支同样全有或全无，磁盘一个字节都不改）。
            let capacity = notify_capacity_from_settings(&state.settings.lock());
            if let Err(error) = capacity {
                state.settings.lock().rollback_migration(receipt);
                return Err(error);
            }
            // 合同要求快照 → 先把迁移前的用户层落到磁盘，再动正式文档。**顺序不能反**：
            // 先写新文档再写镜像，中间掉电的结果是「已经迁了，且没有任何东西能回滚」。
            if receipt.contract().requires_snapshot {
                if let Err(error) = stage_settings_rollback_image(state, &before) {
                    state.settings.lock().rollback_migration(receipt);
                    return Err(error);
                }
            }
            if let Err(error) = persist_settings_doc(state) {
                state.settings.lock().rollback_migration(receipt);
                if let Some(path) = settings_rollback_path(state) {
                    clear_settings_rollback_image(&path);
                }
                return Err(error);
            }
            // 轮 59：迁移把裸键转成可读的转义键，落盘成功后容量按磁盘事实生效——
            // 与 `cmd_settings_set` 同一姿态（生效只发生在 durable commit 之后）。
            apply_notify_capacity(state)?;
        }
        Ok(steps)
    })
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
