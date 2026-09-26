//! 进程插件运行时租约表（P0-2）。
//!
//! 归属：[`crate::registry::Registry`] 的**同一个域锁**之内。`runtime` 表与
//! `pending` / `streams` 同域，锁顺序 `pending → streams → runtime`，
//! 三者互不嵌套持有（见 `registry.rs` 顶部的锁序文档）。
//!
//! **为什么租约表在宿主核心里，而不在进程 host（`tauron-proc`）里**：
//! 租约是**插件身份**的运行时句柄——`plugin_id ↔ lease ↔ pid` 一一绑定，
//! 生命周期与插件注册表同源（插件卸载必须能回收它的租约）。`tauron-proc` 只
//! 提供"启动 / 探测存活"的能力，它不认识插件、也不该认识注册表，因此它既
//! 不持有这张表，也不依赖本 crate。
//!
//! **不变量**
//! - 一个插件至多一条租约（重复 spawn 幂等返回既有租约，见 `Registry::runtime_ensure_lease`）；
//! - 一条租约对应且仅对应一个 pid；租约失效（进程被回收 / 宿主重启）后
//!   任何按 lease 的查询都返回 `E_LEASE_EXPIRED`，不会"猜一个 pid 顶上去"；
//! - 同一次死亡只记一次崩溃（`crash_recorded` 闸门），因为崩溃检测是**轮询式**的；
//! - **租约离开这张表的唯一途径是 [`RuntimeTable::terminate`]**：卸载、清除、
//!   崩溃后换新租约都必须先真的终止旧 pid。只把表项删掉 = 留下一个没人认领、
//!   没人探测、没人回收的操作系统进程（孤儿）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 终止能力的抽象（**宿主核心不认识进程**：`tauron-host` 不依赖 `tauron-proc`）。
///
/// 为什么要在宿主侧定义这个 trait，而不是让 `Registry` 直接调 `tauron-proc`：
/// 租约表是**通用**的运行时句柄表（js/wasm 执行器以后也会用同一张表），核心
/// 只需要"把某个 pid 停掉"这一件事。把 `tauron-proc` 拉进核心会让「核心 → 进程
/// host」变成编译期依赖，将来 wasm 宿主也得连带拖上进程管理。
///
/// 与 `PluginFlagSink` 同形：**由装配层注入**（`crates/tauron-adapter` 把它接到
/// `ProcSpawner::kill`），缺省不注入——不注入时终止一律按失败留痕（见
/// [`ReapStats::last_error`]），绝不假装杀过。
pub trait LeaseReaper: Send + Sync {
    /// 终止 `pid`。
    ///
    /// - `Ok(`[`ReapOutcome::Terminated`]`)`：本次真的停掉了进程；
    /// - `Ok(`[`ReapOutcome::AlreadyGone`]`)`：进程此前已退出（目标已达成）；
    /// - `Err(msg)`：**无法确认**进程已终止（权限不足 / 句柄失效 / 无终止能力）。
    ///   上层据此留痕，但**不得**因此让卸载失败。
    fn kill(&self, pid: u32) -> Result<ReapOutcome, String>;
}

/// 一次终止动作的结果（宿主侧，与 `tauron_proc::KillOutcome` 同形而**不同源**）。
///
/// 不带 serde：它**不跨 IPC**（宿主内部的一次动作结果），留在宿主侧即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapOutcome {
    /// 本次真的终止了进程。
    Terminated,
    /// 进程此前已经退出：没有杀任何东西，但"没有存活进程"已成立。
    AlreadyGone,
}

/// 租约回收的留痕（"终止失败不得让卸载失败，但不得静默吞"）。
///
/// 三类分开计数：`terminated`（真杀）、`already_gone`（早已退出，常态）、
/// `failures`（**杀不掉**——这才是要看的信号），外加最后一次失败原因。
///
/// **是全局计数**（宿主进程生命周期内、所有插件累计），不是某一条租约或某一个
/// 插件的统计：它以 `host_runtime_health` 的 `reap` 字段跨 IPC（见
/// `tauron-adapter` 的 `RuntimeHealth::reap`），消费方切勿当成"本条租约"的数字。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReapStats {
    /// 发起过多少次终止（每次租约回收/换新各一次）。
    pub attempts: u32,
    /// 真正杀掉进程的次数。
    pub terminated: u32,
    /// 进程此前已退出的次数。
    pub already_gone: u32,
    /// **无法确认已终止**的次数（未注入终止器 / 权限不足 / 句柄失效）。
    pub failures: u32,
    /// 最后一次失败原因（成功不清空历史失败计数，只更新原因）。
    pub last_error: Option<String>,
}

/// 一次成功启动的租约句柄（跨 IPC）。
///
/// `lease` 由宿主铸造（与 pending call 的 `callId`、流句柄的 `streamId` 同规矩）：
/// 前端自报租约就能指向别人的进程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHandle {
    /// 操作系统进程号。
    pub pid: u32,
    /// 租约 token。
    pub lease: String,
}

/// 租约条目（宿主内部，不跨 IPC——`Instant` 不可序列化，也不该外泄）。
#[derive(Debug, Clone)]
pub struct RuntimeLease {
    /// 租约 token。
    pub lease: String,
    /// 归属插件 id。
    pub plugin_id: String,
    /// 操作系统进程号。
    pub pid: u32,
    /// 登记时刻（诊断用）。
    pub started_at: Instant,
    /// 这次死亡是否**已经被记录过**。
    ///
    /// 崩溃检测是轮询式的：进程死后的每一次 `host_runtime_health` 都会看到
    /// "不存活"。没有这个闸门，重复轮询会把同一次死亡反复计成新崩溃——计数器、
    /// 崩溃事件、`consecutiveFailures` 全部被放大到失真（而且越勤轮询越失真）。
    pub crash_recorded: bool,
}

/// 运行时租约表。
#[derive(Default)]
pub struct RuntimeTable {
    /// lease → 条目。
    leases: HashMap<String, RuntimeLease>,
    /// plugin_id → lease（"一个插件至多一条租约"的索引）。
    by_plugin: HashMap<String, String>,
    /// 终止能力（装配层注入；未注入 = 无法终止，按失败留痕）。
    reaper: Option<Arc<dyn LeaseReaper>>,
    /// 回收留痕。
    reap: ReapStats,
}

impl std::fmt::Debug for RuntimeTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeTable")
            .field("leases", &self.leases.len())
            .field("reaper", &self.reaper.is_some())
            .field("reap", &self.reap)
            .finish()
    }
}

impl RuntimeTable {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入终止能力（装配层调用；可覆盖，便于测试换 fake）。
    pub fn set_reaper(&mut self, reaper: Arc<dyn LeaseReaper>) {
        self.reaper = Some(reaper);
    }

    /// 回收留痕快照。
    pub fn reap_stats(&self) -> ReapStats {
        self.reap.clone()
    }

    /// 表内租约数（诊断/测试）。
    pub fn len(&self) -> usize {
        self.leases.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    /// 某插件当前的租约句柄（**不论是否已崩溃**）。
    pub fn handle_of_plugin(&self, plugin_id: &str) -> Option<RuntimeHandle> {
        let lease = self.by_plugin.get(plugin_id)?;
        let e = self.leases.get(lease)?;
        Some(RuntimeHandle { pid: e.pid, lease: e.lease.clone() })
    }

    /// 某插件**仍在跑**的租约句柄（已标崩溃的租约不算）。
    pub fn live_handle_of_plugin(&self, plugin_id: &str) -> Option<RuntimeHandle> {
        let h = self.handle_of_plugin(plugin_id)?;
        match self.leases.get(&h.lease) {
            Some(e) if !e.crash_recorded => Some(h),
            _ => None,
        }
    }

    /// 该插件是否需要（重新）启动进程：没有租约，或者租约对应的进程已经崩过。
    ///
    /// 崩溃后的租约**不再**被 `runtime_ensure_lease` 当成"已有"（否则崩溃重试永远
    /// 起不来新进程，重试预算形同虚设）；`cmd_runtime_spawn` 用它决定要不要过
    /// 崩溃预算这道门。
    pub fn needs_restart(&self, plugin_id: &str) -> bool {
        self.live_handle_of_plugin(plugin_id).is_none()
    }

    /// 按 lease 取条目。
    pub fn get(&self, lease: &str) -> Option<RuntimeLease> {
        self.leases.get(lease).cloned()
    }

    /// 登记一条租约并铸 lease token。
    ///
    /// 若该插件已有租约（崩溃后的换新就是这条路径），**先终止旧 pid 再换**：
    /// 直接把表项覆盖掉会让旧进程成为孤儿（没人探测、卸载也回收不掉）。
    pub fn register(&mut self, plugin_id: &str, pid: u32) -> RuntimeHandle {
        if self.by_plugin.contains_key(plugin_id) {
            self.remove_plugin(plugin_id);
        }
        let lease = Uuid::new_v4().to_string();
        self.by_plugin.insert(plugin_id.to_string(), lease.clone());
        self.leases.insert(
            lease.clone(),
            RuntimeLease {
                lease: lease.clone(),
                plugin_id: plugin_id.to_string(),
                pid,
                started_at: Instant::now(),
                crash_recorded: false,
            },
        );
        RuntimeHandle { pid, lease }
    }

    /// 标记该租约的进程已崩溃。
    ///
    /// - `Some(true)`：**本次**首次标记（调用方据此计一次崩溃、投一次 `RuntimeCrash`）；
    /// - `Some(false)`：之前已标记过（重复轮询，不得重复计数）；
    /// - `None`：租约不存在（调用方报 `E_LEASE_EXPIRED`）。
    pub fn mark_crashed(&mut self, lease: &str) -> Option<bool> {
        let e = self.leases.get_mut(lease)?;
        if e.crash_recorded {
            Some(false)
        } else {
            e.crash_recorded = true;
            Some(true)
        }
    }

    /// 回收某插件的租约（卸载 / 清除）：**先终止进程，再摘表项**。
    ///
    /// 返回被回收的句柄（供调用方/测试核对 pid）。终止失败不影响回收本身——
    /// 租约必须消失（否则重装同名插件会撞上旧租约），失败只留痕。
    pub fn remove_plugin(&mut self, plugin_id: &str) -> Option<RuntimeHandle> {
        let lease = self.by_plugin.remove(plugin_id)?;
        let e = self.leases.remove(&lease)?;
        self.terminate(e.pid);
        Some(RuntimeHandle { pid: e.pid, lease: e.lease })
    }

    /// 回收**仍在跑**的租约：已标崩溃的租约**不动**（进程早已死透，租约要留给
    /// `host_runtime_health` 把这次崩溃报出去——在那里摘掉租约，调用方只会拿到
    /// `E_LEASE_EXPIRED`，崩溃事实连同计数一起消失）。
    ///
    /// 用途：状态落到"不可用"（`ENABLED`/`RUNNING` 之外）时保证不留活进程。
    pub fn remove_if_live(&mut self, plugin_id: &str) -> Option<RuntimeHandle> {
        let h = self.handle_of_plugin(plugin_id)?;
        if self.leases.get(&h.lease)?.crash_recorded {
            return None;
        }
        self.remove_plugin(plugin_id)
    }

    /// 终止一个 pid 并留痕。**不返回错误**：终止失败绝不能让卸载失败，
    /// 但一定会进 [`ReapStats`]（可查），不静默吞。
    ///
    /// 未注入 [`LeaseReaper`] 时按**失败**记账，而不是按"什么都没发生"跳过：
    /// 宿主配置漏注入会表现为 `failures` 持续增长，而不是看起来一切正常。
    fn terminate(&mut self, pid: u32) {
        self.reap.attempts += 1;
        let Some(reaper) = self.reaper.clone() else {
            self.reap.failures += 1;
            self.reap.last_error =
                Some(format!("未注入 LeaseReaper：pid {pid} 未被终止（可能是孤儿进程）"));
            return;
        };
        match reaper.kill(pid) {
            Ok(ReapOutcome::Terminated) => self.reap.terminated += 1,
            Ok(ReapOutcome::AlreadyGone) => self.reap.already_gone += 1,
            Err(msg) => {
                self.reap.failures += 1;
                self.reap.last_error = Some(format!("终止 pid {pid} 失败：{msg}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn register_binds_one_lease_to_one_pid() {
        let mut t = RuntimeTable::new();
        let h = t.register("com.example.proc", 4242);
        assert_eq!(h.pid, 4242);
        assert!(!h.lease.is_empty());
        assert_eq!(t.len(), 1);
        assert_eq!(t.handle_of_plugin("com.example.proc"), Some(h.clone()));
        assert_eq!(t.get(&h.lease).unwrap().pid, 4242);
        // 未知租约必须取不到（而不是猜一个）。
        assert!(t.get("no-such-lease").is_none());
        assert!(t.handle_of_plugin("com.example.other").is_none());
    }

    #[test]
    fn crash_is_recorded_at_most_once_per_lease() {
        let mut t = RuntimeTable::new();
        let h = t.register("com.example.proc", 1);
        assert_eq!(t.mark_crashed(&h.lease), Some(true), "首次必须是 true");
        assert_eq!(t.mark_crashed(&h.lease), Some(false), "重复轮询不得再计数");
        assert_eq!(t.mark_crashed(&h.lease), Some(false));
        assert_eq!(t.mark_crashed("ghost"), None);
    }

    #[test]
    fn remove_plugin_frees_the_index() {
        let mut t = RuntimeTable::new();
        let h = t.register("com.example.proc", 7);
        assert_eq!(t.remove_plugin("com.example.proc"), Some(h.clone()));
        assert!(t.is_empty());
        assert!(t.get(&h.lease).is_none(), "回收后按 lease 必须查不到");
        // 回收后可重新登记（不同租约 → 不同 pid 绑定）。
        let again = t.register("com.example.proc", 8);
        assert_ne!(again.lease, h.lease);
    }

    /// 记录终止调用的 fake（只记 pid，不起进程）。
    #[derive(Default)]
    struct RecordingReaper {
        killed: Mutex<Vec<u32>>,
        fail_with: Option<String>,
        already_gone: bool,
    }

    impl RecordingReaper {
        fn killed(&self) -> Vec<u32> {
            self.killed.lock().clone()
        }
    }

    impl LeaseReaper for RecordingReaper {
        fn kill(&self, pid: u32) -> Result<ReapOutcome, String> {
            self.killed.lock().push(pid);
            if let Some(msg) = &self.fail_with {
                return Err(msg.clone());
            }
            if self.already_gone {
                return Ok(ReapOutcome::AlreadyGone);
            }
            Ok(ReapOutcome::Terminated)
        }
    }

    /// 回收租约**必须真的终止 pid**（只删表项 = 孤儿进程），且 pid 要对得上。
    #[test]
    fn removing_a_lease_terminates_the_pid_and_traces_it() {
        let mut t = RuntimeTable::new();
        let reaper = Arc::new(RecordingReaper::default());
        t.set_reaper(reaper.clone());
        let h = t.register("com.example.proc", 4242);

        assert_eq!(t.remove_plugin("com.example.proc"), Some(h));
        assert_eq!(reaper.killed(), vec![4242], "必须按租约里的 pid 终止");
        let s = t.reap_stats();
        assert_eq!((s.attempts, s.terminated, s.already_gone, s.failures), (1, 1, 0, 0));
        assert_eq!(s.last_error, None);
    }

    /// 换新租约（崩溃后重试）也要先终止旧 pid——不允许"直接覆盖"。
    #[test]
    fn re_registering_terminates_the_previous_pid() {
        let mut t = RuntimeTable::new();
        let reaper = Arc::new(RecordingReaper::default());
        t.set_reaper(reaper.clone());
        let first = t.register("com.example.proc", 11);
        assert!(t.mark_crashed(&first.lease).unwrap());
        assert!(t.needs_restart("com.example.proc"), "崩溃租约必须视为需要重启");
        assert!(t.live_handle_of_plugin("com.example.proc").is_none());

        let second = t.register("com.example.proc", 22);
        assert_eq!(reaper.killed(), vec![11], "旧 pid 必须先被终止");
        assert_eq!(t.len(), 1, "换新后仍然只有一个租约");
        assert!(!t.needs_restart("com.example.proc"), "新租约是活的");
        assert_eq!(t.live_handle_of_plugin("com.example.proc"), Some(second));
        assert!(t.get(&first.lease).is_none(), "旧租约必须失效");
    }

    /// 终止失败：**不影响回收**，但必须留痕（不得静默吞）。
    #[test]
    fn kill_failure_is_traced_but_does_not_block_reclamation() {
        let mut t = RuntimeTable::new();
        t.set_reaper(Arc::new(RecordingReaper {
            fail_with: Some("权限不足".into()),
            ..Default::default()
        }));
        t.register("com.example.proc", 9);
        assert!(t.remove_plugin("com.example.proc").is_some(), "回收本身必须成功");
        assert!(t.is_empty(), "租约必须消失，否则重装会撞上旧租约");
        let s = t.reap_stats();
        assert_eq!((s.attempts, s.failures), (1, 1));
        assert!(
            s.last_error.as_deref().unwrap_or("").contains("权限不足"),
            "失败原因必须可查：{:?}",
            s.last_error
        );
    }

    /// 进程早已退出：如实记 `already_gone`，**不**混进失败计数。
    #[test]
    fn already_gone_is_not_counted_as_failure() {
        let mut t = RuntimeTable::new();
        t.set_reaper(Arc::new(RecordingReaper { already_gone: true, ..Default::default() }));
        t.register("com.example.proc", 3);
        t.remove_plugin("com.example.proc");
        let s = t.reap_stats();
        assert_eq!((s.attempts, s.already_gone, s.failures), (1, 1, 0));
        assert_eq!(s.last_error, None);
    }

    /// **漏注入终止器**必须表现为失败留痕，而不是"看起来一切正常"。
    #[test]
    fn missing_reaper_is_recorded_as_failure() {
        let mut t = RuntimeTable::new();
        t.register("com.example.proc", 5);
        t.remove_plugin("com.example.proc");
        let s = t.reap_stats();
        assert_eq!((s.attempts, s.failures), (1, 1));
        assert!(
            s.last_error.as_deref().unwrap_or("").contains("未注入"),
            "漏注入必须能查出来：{:?}",
            s.last_error
        );
    }
}
