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
//! - **终止失败不会把 pid 一起丢掉**：它进有界重试队列，由
//!   [`RuntimeTable::retry_pending_reaps`] 的调用方推进；到尝试上限即固化成
//!   [`TerminalReapRecord`]（V7 §7 的「kill retry/backoff + evidence」、§9 的
//!   「kill failure 不能只留日志」）。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::durable::{decode_durable, encode_durable, DurableEnvelope};
use crate::generation::{Generation, GenerationHandle, GenerationRegistry, GenerationStats};
use crate::liveness::{PidLiveness, PidProbe};

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
/// 失败还会带出**后续**：`pending` / `retries` / `recovered` / `terminal` / `overflow` /
/// `skipped_live_pid` / `terminal_records` 记录"重试有没有发生、有没有让位、最后固化成
/// 什么证据"（V7 §9 的「kill failure 必须有 retry/terminal evidence，不能只留日志」）。
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
    /// **当前还在等重试**的 pid 数（有界，上界 [`MAX_PENDING_REAPS`]）。
    ///
    /// 这是 V7 §9 leak gate 要的读数：kill 失败既不能只留日志，也不能变成
    /// 无限增长的待办队列。
    pub pending: usize,
    /// 由驱动推进的**重试**次数（不含首次尝试，因此 `attempts >= initial + retries`）。
    pub retries: u32,
    /// 重试后终于终止掉的条数（对应队列项出队且记 `terminated` / `already_gone`）。
    pub recovered: u32,
    /// 放弃重试、固化为终态证据的条数（见 [`ReapStats::terminal_records`]）。
    pub terminal: u32,
    /// 待重试队列已满、只能直接固化证据的次数（增长被拒 = 有界的证据）。
    pub overflow: u32,
    /// 重试**主动让位**的次数：该 pid 此刻已被某条在册租约持有。
    ///
    /// pid 会被操作系统回收重用。若插件卸载时"杀不掉"、随后同一个 pid 又被新进程用上，
    /// 拿着旧记录去打这个 pid 就是**杀一个宿主自己刚起的活进程**（`ProcSpawner` 的跟踪表
    /// 按 pid 索引，分不出"旧的死进程"与"重用同一号的新进程"）。让位不是失败也不是终止：
    /// 旧进程按定义已经不在了（号码已被新主人占用），所以这里只计数并固化一条证据。
    pub skipped_live_pid: u32,
    /// 重启扫描**销账**数：平台探测确认 pid 已不存在（上一轮宿主的终止失败已无对象）。
    pub sweep_resolved: u32,
    /// 重启扫描**存活**数：pid 仍在，但跨重启无法验明进程身份——**不盲杀**，留证待人查。
    pub sweep_survivors: u32,
    /// 重启扫描**无法判定**数：探测不可用/结论不可靠——同样不杀、留证。
    pub sweep_unknown: u32,
    /// 台账 durable 写盘失败次数（持久化腿不得静默：与 `last_error` 分开计数）。
    pub ledger_write_failures: u32,
    /// 终态证据（环形，最多 [`MAX_TERMINAL_REAPS`] 条，最旧的被覆盖）。
    ///
    /// 「kill failure 必须有 terminal evidence」的落点：这里带的是**能拿去查的
    /// 具体 pid / 插件 / 原因**，不是一句日志。
    pub terminal_records: Vec<TerminalReapRecord>,
}

/// 一条**不再重试**的回收证据（V7 §7「kill retry/backoff + evidence」的 evidence 腿）。
///
/// 两种来路：重试到顶（[`MAX_REAP_RETRY_ATTEMPTS`]）／队列满而直接固化，以及重试**让位**
/// 给在册租约（见 [`ReapStats::skipped_live_pid`]）——原因写在 `reason` 里，读的人不必猜。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReapRecord {
    /// 归属插件 id。租约此刻已经消失，但进程可能还活着——这正是必须留证据的原因。
    pub plugin_id: String,
    /// 终止失败的进程号。
    pub pid: u32,
    /// 一共尝试过几次（含首次）。
    pub attempts: u32,
    /// 最后一次失败原因。
    pub reason: String,
}

/// 队列里等待重试的条目（宿主内部，不跨 IPC）。
#[derive(Debug, Clone)]
struct PendingReap {
    plugin_id: String,
    pid: u32,
    /// 已经尝试过的次数（首次 `terminate` 记 1）。
    attempts: u32,
    reason: String,
}

/// 回收台账里条目的可序列化形态（[`PendingReap`] 的持久化镜像）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReapLedgerEntry {
    plugin_id: String,
    pid: u32,
    attempts: u32,
    reason: String,
}

impl From<&PendingReap> for ReapLedgerEntry {
    fn from(value: &PendingReap) -> Self {
        Self {
            plugin_id: value.plugin_id.clone(),
            pid: value.pid,
            attempts: value.attempts,
            reason: value.reason.clone(),
        }
    }
}

/// `reap-ledger.json` 的载荷：上一轮宿主可能留下又来不及重试的条目 + 终态证据环形册。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReapLedgerPayload {
    pending: Vec<ReapLedgerEntry>,
    terminal: Vec<TerminalReapRecord>,
}

/// durable 信封的 schema 标识（进校验和）。
const REAP_LEDGER_SCHEMA: &str = "tauron.reap-ledger/1";
/// 回收台账文件名（宿主数据目录内，与 `admin-audit.json` 同域）。
pub const REAP_LEDGER_FILE: &str = "reap-ledger.json";

/// 台账读取失败：撕裂/篡改/IO——装配期**拒绝启动**，不静默重置。
///
/// 与 `AdminAuditSink::open` 同规矩：一份读不出来的持久化台账不允许被悄悄当成空账。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReapLedgerError {
    /// 文件系统层失败（建目录/读文件）。
    #[error("reap ledger io failed: {0}")]
    Io(String),
    /// durable 校验失败（撕裂、篡改、schema 不符）。
    #[error("reap ledger failed integrity checks: {0}")]
    Integrity(String),
}

/// 一条失败回收最多**再试**几次（首试之外）。到顶即固化为终态证据。
pub const MAX_REAP_RETRY_ATTEMPTS: u32 = 3;
/// 待重试队列上界：超出即拒绝继续增长，直接固化为终态证据并计 `overflow`。
pub const MAX_PENDING_REAPS: usize = 64;
/// 终态证据环形台账上界。
pub const MAX_TERMINAL_REAPS: usize = 16;

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
    /// V4 A88/A90 active generation bound to this lease.
    pub generation: Generation,
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
    /// V4 A88 generation active when this runtime was registered.
    pub generation: Generation,
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
    /// V4 A88/A90 active-generation authority; runtime lease tokens are generation lease tokens.
    ///
    /// 与租约表一比一同生命周期：`register` 铸一条，`remove_plugin` 放一条并摘除跟踪。
    generations: GenerationRegistry,
    /// 终止能力（装配层注入；未注入 = 无法终止，按失败留痕）。
    reaper: Option<Arc<dyn LeaseReaper>>,
    /// 回收留痕。
    reap: ReapStats,
    /// 终止失败的 pid 等待重试的队列（上界 [`MAX_PENDING_REAPS`]）。
    ///
    /// **为什么需要它**：`tauron-proc::ProcSpawner::kill` 失败时把子进程句柄放回
    /// 自己的跟踪表并在错误里写「可重试」，而这一侧以前只是记一笔 `failures` 就
    /// 把租约摘掉——下层保留了可重试的句柄，上层却没有任何重试方，那句"可重试"
    /// 是断链。队列让它真的有下文：驱动每调一次推进一轮（见 [`Self::retry_pending_reaps`]）。
    pending_reaps: VecDeque<PendingReap>,
    /// 放弃重试的终态证据（环形，上界 [`MAX_TERMINAL_REAPS`]）。
    terminal_reaps: VecDeque<TerminalReapRecord>,
    /// 跨重启台账文件路径（装配期配置；`None` = 纯内存宿主，合法的无盘模式）。
    reap_ledger_path: Option<PathBuf>,
    /// 台账 durable generation（每次写盘 +1；读回续写）。
    reap_ledger_seq: u64,
    /// 台账是否有未落盘的变更（锁内只标脏/取快照，磁盘写在锁外）。
    reap_ledger_dirty: bool,
    /// 平台存活性探测（装配层经 [`RuntimeTable::set_pid_probe`] 注入）。
    ///
    /// 未注入时重启扫描一律按 [`PidLiveness::Unknown`] 留证——**不因此盲杀**。
    pid_probe: Option<Arc<dyn PidProbe>>,
    /// **上一轮宿主生命周期**留下的待重试条目（从台账读回，等待启动扫描）。
    ///
    /// 与 `pending_reaps` 刻意分池：这些条目**不**进 [`RuntimeTable::retry_pending_reaps`]
    /// 的杀进程腿——跨重启后租约表是空的，`pid_owner` 让位封口失效，拿着旧 pid 去打
    /// 就是盲杀（同号可能已被无关新进程复用）。它们只走 [`RuntimeTable::sweep_restart_reaps`]。
    restart_reaps: VecDeque<ReapLedgerEntry>,
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

    /// 注入平台存活性探测（装配层调用；测试可覆盖为确定性替身）。
    ///
    /// 探测只在 [`Self::sweep_restart_reaps`] 里被使用，且**只读**——未注入时
    /// 扫描按 `Unknown` 留证，绝不因此盲杀。
    pub fn set_pid_probe(&mut self, probe: Arc<dyn PidProbe>) {
        self.pid_probe = Some(probe);
    }

    /// 回收留痕。
    pub fn reap_stats(&self) -> ReapStats {
        let mut s = self.reap.clone();
        // 队列与证据是**状态**，不进 `self.reap`（那里只放累计计数），读数时合成快照。
        s.pending = self.pending_reaps.len();
        s.terminal_records = self.terminal_reaps.iter().cloned().collect();
        s
    }

    /// 代际台账读数（V7 §9 leak gate 的宿主侧出口）。
    ///
    /// `tracked_resources` 的上界是**在管插件数**：租约回收会连带摘除代际跟踪
    /// （见 [`Self::remove_plugin`]），因此反复安装/卸载不同插件不会把它推高。
    pub fn generation_stats(&self) -> GenerationStats {
        self.generations.stats()
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
        let e = self.get(lease)?;
        Some(RuntimeHandle { pid: e.pid, lease: e.lease.clone(), generation: e.generation })
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
        let entry = self.leases.get(lease)?.clone();
        let handle = GenerationHandle {
            resource: entry.plugin_id.clone(),
            generation: entry.generation,
            token: entry.lease.clone(),
        };
        self.generations.validate(&handle).ok()?;
        Some(entry)
    }

    /// Strong V4 A90 lookup: both opaque lease token and caller-observed generation must match.
    pub fn get_versioned(&self, lease: &str, generation: Generation) -> Option<RuntimeLease> {
        let entry = self.get(lease)?;
        (entry.generation == generation).then_some(entry)
    }

    /// 登记一条租约并铸 lease token。
    ///
    /// 若该插件已有租约（崩溃后的换新就是这条路径），**先终止旧 pid 再换**：
    /// 直接把表项覆盖掉会让旧进程成为孤儿（没人探测、卸载也回收不掉）。
    pub fn register(&mut self, plugin_id: &str, pid: u32) -> RuntimeHandle {
        if self.by_plugin.contains_key(plugin_id) {
            self.remove_plugin(plugin_id);
        }

        let generation = self.generations.activate(plugin_id);
        let generation_handle = self
            .generations
            .lease(plugin_id)
            .expect("runtime generation was activated immediately before lease");
        let lease = generation_handle.token;
        debug_assert_eq!(generation_handle.generation, generation);

        self.by_plugin.insert(plugin_id.to_string(), lease.clone());
        self.leases.insert(
            lease.clone(),
            RuntimeLease {
                lease: lease.clone(),
                plugin_id: plugin_id.to_string(),
                pid,
                generation,
                started_at: Instant::now(),
                crash_recorded: false,
            },
        );
        RuntimeHandle { pid, lease, generation }
    }

    /// 标记该租约的进程已崩溃。
    ///
    /// - `Some(true)`：**本次**首次标记（调用方据此计一次崩溃、投一次 `RuntimeCrash`）；
    /// - `Some(false)`：之前已标记过（重复轮询，不得重复计数）；
    /// - `None`：租约不存在（调用方报 `E_LEASE_EXPIRED`）。
    pub fn mark_crashed(&mut self, lease: &str) -> Option<bool> {
        self.get(lease)?;
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
    /// 租约必须消失（否则重装同名插件会撞上旧租约）。但失败**不止于留痕**：
    /// pid 会进 [`Self::pending_reaps`] 等驱动重试（V7 §7 / §9）。
    pub fn remove_plugin(&mut self, plugin_id: &str) -> Option<RuntimeHandle> {
        let lease = self.by_plugin.remove(plugin_id)?;
        let e = self.leases.remove(&lease)?;
        self.generations.release(&lease);
        // 代际跟踪随租约一起摘除：租约没了就没有可校验的对象，留着只会让台账按
        // "曾起过运行期的插件数"增长（V7 §9 leak gate，读数见 [`Self::generation_stats`]）。
        self.generations.forget(plugin_id);
        self.terminate(plugin_id, e.pid);
        Some(RuntimeHandle { pid: e.pid, lease: e.lease, generation: e.generation })
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
    fn terminate(&mut self, plugin_id: &str, pid: u32) {
        self.reap.attempts += 1;
        let failure = match self.reaper.clone() {
            None => {
                self.reap.failures += 1;
                let reason = format!("未注入 LeaseReaper：pid {pid} 未被终止（可能是孤儿进程）");
                self.reap.last_error = Some(reason.clone());
                Some(reason)
            }
            Some(reaper) => match reaper.kill(pid) {
                Ok(ReapOutcome::Terminated) => {
                    self.reap.terminated += 1;
                    None
                }
                Ok(ReapOutcome::AlreadyGone) => {
                    self.reap.already_gone += 1;
                    None
                }
                Err(msg) => {
                    self.reap.failures += 1;
                    let reason = format!("终止 pid {pid} 失败：{msg}");
                    self.reap.last_error = Some(reason.clone());
                    Some(reason)
                }
            },
        };
        if let Some(reason) = failure {
            self.enqueue_pending(plugin_id, pid, reason);
        }
    }

    /// 该 pid 是否仍是某条**在册租约**的进程号（含已标崩溃的：宁可让位，不可误杀）。
    ///
    /// 返回该租约的 token，只为把证据写得能拿去查。
    fn pid_owner(&self, pid: u32) -> Option<String> {
        self.leases.values().find(|e| e.pid == pid).map(|e| e.lease.clone())
    }

    /// 把一次失败的终止挂进重试队列；队列已满则**直接固化为终态证据**。
    ///
    /// 两条路都不许静默：入队 = 还有下文，固化 = 有 pid/插件/原因可查。
    fn enqueue_pending(&mut self, plugin_id: &str, pid: u32, reason: String) {
        if self.pending_reaps.iter().any(|p| p.pid == pid) {
            // 同一 pid 只保留一条待重试记录（重复入队会把队列撑到与失败次数同量级）。
            if let Some(p) = self.pending_reaps.iter_mut().find(|p| p.pid == pid) {
                p.reason = reason;
                self.reap_ledger_dirty = true;
            }
            return;
        }
        if self.pending_reaps.len() >= MAX_PENDING_REAPS {
            self.reap.overflow += 1;
            self.push_terminal(TerminalReapRecord {
                plugin_id: plugin_id.to_string(),
                pid,
                attempts: 1,
                reason: format!("待重试队列已满（{MAX_PENDING_REAPS}）：{reason}"),
            });
            return;
        }
        self.pending_reaps.push_back(PendingReap {
            plugin_id: plugin_id.to_string(),
            pid,
            attempts: 1,
            reason,
        });
        self.reap_ledger_dirty = true;
    }

    /// 固化一条终态证据（环形：超出 [`MAX_TERMINAL_REAPS`] 覆盖最旧的一条）。
    ///
    /// 证据是台账载荷的一部分，因此这条也标脏（调用方经
    /// [`Self::take_reap_ledger_write`] 在锁外落盘）。
    fn push_terminal(&mut self, record: TerminalReapRecord) {
        self.reap.terminal += 1;
        if self.terminal_reaps.len() >= MAX_TERMINAL_REAPS {
            self.terminal_reaps.pop_front();
        }
        self.terminal_reaps.push_back(record);
        self.reap_ledger_dirty = true;
    }

    /// 打开（或建目录）宿主数据目录下的跨重启台账，读回**上一轮宿主**留下的条目。
    ///
    /// **只读不写**：条目进 [`Self::restart_reaps`]（分池，不进杀进程重试腿），
    /// 等 [`Self::sweep_restart_reaps`] 定性；文件保持原样，直到扫描后由调用方经
    /// [`Self::take_reap_ledger_write`] 落盘——"读了还没扫就崩"不会把条目弄丢。
    /// 撕裂/篡改/IO 失败一律 `Err`（装配期拒绝启动，不静默重置成空账）。
    pub fn configure_reap_ledger(&mut self, dir: &Path) -> Result<(), ReapLedgerError> {
        std::fs::create_dir_all(dir).map_err(|e| ReapLedgerError::Io(e.to_string()))?;
        let path = dir.join(REAP_LEDGER_FILE);
        if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| ReapLedgerError::Io(e.to_string()))?;
            let envelope: DurableEnvelope<ReapLedgerPayload> =
                decode_durable(&bytes).map_err(|e| ReapLedgerError::Integrity(e.to_string()))?;
            self.reap_ledger_seq = envelope.generation;
            self.restart_reaps = envelope.payload.pending.into_iter().collect();
            // 上一轮的终态证据并回环形册：`terminal` 累计计数**不加**（那是宿主生命
            // 周期内的口径），读回的是**状态**，与 `terminal_records` 快照同语义。
            for record in envelope.payload.terminal {
                if self.terminal_reaps.len() >= MAX_TERMINAL_REAPS {
                    self.terminal_reaps.pop_front();
                }
                self.terminal_reaps.push_back(record);
            }
        }
        self.reap_ledger_path = Some(path);
        Ok(())
    }

    /// 启动扫描：把**上一轮宿主**留下的待重试条目逐条定性为终态证据。
    ///
    /// **安全封口（轮 41 的核心）**：跨重启后租约表是空的，
    /// [`Self::retry_pending_reaps`] 的 `pid_owner` 让位封口失效——同一个 pid 可能
    /// 已被无关新进程复用，**任何**按 pid 的终止都可能是误杀。因此本扫描**只读**，只定性、不杀：
    /// - [`PidLiveness::Gone`]：确认已不存在 → 销账（计 `already_gone` + 终态证据）；
    /// - [`PidLiveness::Alive`]：进程仍在，但无法验明是不是本宿主的旧子进程 →
    ///   **不杀**、留证（宿主存活期内的正常路径已由 Job Object `KILL_ON_JOB_CLOSE` /
    ///   进程组 containment 覆盖，剩的是逃逸/失败残骸，交操作者按证据处置）；
    /// - [`PidLiveness::Unknown`]：探测未注入/不可用 → 同样不杀、留证。
    ///
    /// 空台账（首启或上一轮干净）也会标脏，让调用方落一份空账——"本轮扫描过"本身
    /// 是可查事实。返回销账条数。
    pub fn sweep_restart_reaps(&mut self) -> usize {
        let queue = std::mem::take(&mut self.restart_reaps);
        let mut resolved = 0usize;
        for entry in queue {
            let liveness = self
                .pid_probe
                .as_ref()
                .map(|probe| probe.probe(entry.pid))
                .unwrap_or(PidLiveness::Unknown);
            match liveness {
                PidLiveness::Gone => {
                    self.reap.already_gone += 1;
                    self.reap.sweep_resolved += 1;
                    resolved += 1;
                    self.push_terminal(TerminalReapRecord {
                        plugin_id: entry.plugin_id,
                        pid: entry.pid,
                        attempts: entry.attempts,
                        reason: format!(
                            "重启扫描销账：pid {} 经平台探测已不存在（上一轮宿主终止失败，现已无对象）",
                            entry.pid
                        ),
                    });
                }
                PidLiveness::Alive => {
                    self.reap.sweep_survivors += 1;
                    self.push_terminal(TerminalReapRecord {
                        plugin_id: entry.plugin_id,
                        pid: entry.pid,
                        attempts: entry.attempts,
                        reason: format!(
                            "重启扫描留证：pid {} 仍在运行；跨重启无法验明进程身份，刻意不盲杀——请按此证据人工处置",
                            entry.pid
                        ),
                    });
                }
                PidLiveness::Unknown => {
                    self.reap.sweep_unknown += 1;
                    self.push_terminal(TerminalReapRecord {
                        plugin_id: entry.plugin_id,
                        pid: entry.pid,
                        attempts: entry.attempts,
                        reason: format!(
                            "重启扫描留证：pid {} 存活性探测不可用（或哨兵形态），状态未定——不盲杀，留证待人查",
                            entry.pid
                        ),
                    });
                }
            }
        }
        // 无论有没有条目都标脏：调用方落一份"已扫描"的账（空账也落）。
        self.reap_ledger_dirty = true;
        resolved
    }

    /// 取一份待落盘的台账快照（锁内只做序列化；磁盘写由调用方在锁外做，
    /// 同 `AdminAuditSink::record` 的"编码在锁内、写盘在锁外"）。
    ///
    /// `None` = 没配置路径（无盘宿主）或没有脏变更。成功取快照即清脏并 +1
    /// generation；seal/encode 失败（纯内存步骤）计 `ledger_write_failures` 并返回
    /// `None`，脏位保留待下次尝试。
    pub fn take_reap_ledger_write(&mut self) -> Option<(PathBuf, Vec<u8>)> {
        if !self.reap_ledger_dirty {
            return None;
        }
        let path = self.reap_ledger_path.clone()?;
        let payload = ReapLedgerPayload {
            pending: self.pending_reaps.iter().map(ReapLedgerEntry::from).collect(),
            terminal: self.terminal_reaps.iter().cloned().collect(),
        };
        let seq = self.reap_ledger_seq + 1;
        match DurableEnvelope::seal(REAP_LEDGER_SCHEMA, seq, payload)
            .and_then(|envelope| encode_durable(&envelope))
        {
            Ok(bytes) => {
                self.reap_ledger_seq = seq;
                self.reap_ledger_dirty = false;
                Some((path, bytes))
            }
            Err(_) => {
                self.reap.ledger_write_failures += 1;
                None
            }
        }
    }

    /// 落盘腿报告写失败（磁盘写在锁外做，失败回锁内留痕——不得静默）。
    pub fn note_ledger_write_failure(&mut self) {
        self.reap.ledger_write_failures += 1;
    }

    /// 推进一轮**有界**重试：每个待重试 pid 至多再试一次。返回本轮真正终止掉的条数。
    ///
    /// **调用方（唯一的驱动腿）**：`cmd_resource_stats`——主窗轮询诊断即回收点，与
    /// `Registry::gc_expired` 同一条既定口径（见该函数注释）。刻意**不**挂在
    /// `host_runtime_health` 上：那条命令按 lease 轮询、频率由调用方决定，插件侧
    /// 高频轮询会变成"用轮询驱动杀进程"。
    ///
    /// 上界三重：队列长度 [`MAX_PENDING_REAPS`]、每条尝试次数
    /// [`MAX_REAP_RETRY_ATTEMPTS`]、证据条数 [`MAX_TERMINAL_REAPS`]。到顶即固化为
    /// [`TerminalReapRecord`] 并出队——绝不无限增长，也绝不只留一句日志。
    ///
    /// **还有一条安全封口**：每条重试在打下第二之前先查 [`Self::pid_owner`]——该 pid
    /// 若已被某条在册租约持有，就是"宿主现在认的活进程"，重试**让位**（计
    /// [`ReapStats::skipped_live_pid`] 并留证据）而不是误杀。pid 会被 OS 重用，而
    /// 终止器按 pid 查跟踪表，分不出旧进程与同号新进程。
    pub fn retry_pending_reaps(&mut self) -> usize {
        if self.pending_reaps.is_empty() {
            return 0;
        }
        // 队列非空 ⇒ 本轮必有条目出队/换新（attempts 变化），台账必脏。
        self.reap_ledger_dirty = true;
        let queue = std::mem::take(&mut self.pending_reaps);
        let mut recovered = 0usize;
        for mut entry in queue {
            // **先问"这个 pid 还有没有主人"**：有就说明它是宿主现在认的活进程（或至少是
            // 在册租约的 pid），拿着旧回收记录去打它等于误杀。旧进程同号复用后按定义
            // 已经不存在，因此这里出队、计数并留证据，不再打第二下。
            if let Some(owner) = self.pid_owner(entry.pid) {
                self.reap.skipped_live_pid += 1;
                self.push_terminal(TerminalReapRecord {
                    plugin_id: entry.plugin_id,
                    pid: entry.pid,
                    attempts: entry.attempts,
                    reason: format!(
                        "pid {} 现由在册租约 {owner} 持有：重试会误杀活进程，故让位出队（旧进程按定义已退出）",
                        entry.pid
                    ),
                });
                continue;
            }
            self.reap.retries += 1;
            self.reap.attempts += 1;
            entry.attempts += 1;
            let outcome = self.reaper.as_ref().map(|reaper| reaper.kill(entry.pid)).map_or_else(
                || Err(format!("仍未注入 LeaseReaper（累计第 {} 次尝试）", entry.attempts)),
                |res| match res {
                    Ok(outcome) => Ok(outcome),
                    Err(msg) => Err(format!("终止 pid {} 失败：{msg}", entry.pid)),
                },
            );
            match outcome {
                Ok(ReapOutcome::Terminated) => {
                    self.reap.terminated += 1;
                    self.reap.recovered += 1;
                    recovered += 1;
                }
                Ok(ReapOutcome::AlreadyGone) => {
                    self.reap.already_gone += 1;
                    self.reap.recovered += 1;
                    recovered += 1;
                }
                Err(reason) => {
                    self.reap.failures += 1;
                    self.reap.last_error = Some(reason.clone());
                    entry.reason = reason;
                    if entry.attempts >= MAX_REAP_RETRY_ATTEMPTS {
                        self.push_terminal(TerminalReapRecord {
                            plugin_id: entry.plugin_id,
                            pid: entry.pid,
                            attempts: entry.attempts,
                            reason: entry.reason,
                        });
                    } else {
                        self.pending_reaps.push_back(entry);
                    }
                }
            }
        }
        recovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

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
    fn runtime_generation_advances_and_old_handle_becomes_stale() {
        let mut t = RuntimeTable::new();
        let first = t.register("p", 10);
        assert_eq!(first.generation, Generation::INITIAL);
        assert!(t.get_versioned(&first.lease, first.generation).is_some());

        let second = t.register("p", 11);
        assert_eq!(second.generation, first.generation.next());
        assert_ne!(second.lease, first.lease);
        assert!(t.get(&first.lease).is_none());
        assert!(t.get_versioned(&second.lease, first.generation).is_none());
        assert!(t.get_versioned(&second.lease, second.generation).is_some());
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

    /// V7 §9 leak gate：反复起/停**不同**插件不得把代际台账推高，也不得复用被弃的号。
    #[test]
    fn generation_tracking_stays_bounded_under_plugin_churn() {
        const CHURN: u32 = 5_000;
        let mut t = RuntimeTable::new();
        t.set_reaper(Arc::new(RecordingReaper::default()));

        let abandoned = t.register("com.example.churn-0", 1);
        assert!(t.remove_plugin("com.example.churn-0").is_some());
        for i in 1..CHURN {
            let id = format!("com.example.churn-{i}");
            t.register(&id, i + 1);
            assert_eq!(t.generation_stats().tracked_resources, 1, "在管数只随在飞插件走");
            assert!(t.remove_plugin(&id).is_some());
        }

        let idle = t.register("com.example.idle", 9);
        let stats = t.generation_stats();
        assert_eq!(stats.tracked_resources, 1, "churn 过的 id 不得留下跟踪条目");
        assert_eq!(stats.live_leases, 1);
        assert_eq!(stats.generations_issued, CHURN as u64 + 1, "号源只增：发过多少代就记多少");
        assert_eq!(idle.generation, Generation(CHURN as u64 + 1));

        // 摘除跟踪不等于放过旧句柄：被弃租约查不到，重装同名插件必拿更大的号。
        assert!(t.get(&abandoned.lease).is_none());
        let revived = t.register("com.example.churn-0", 10);
        assert!(revived.generation > abandoned.generation, "代际号不得复用");
        assert!(t.get_versioned(&revived.lease, abandoned.generation).is_none());
        // 台账与租约表一比一：两条在飞 = 两条跟踪。
        assert_eq!(t.generation_stats().tracked_resources, t.len());
        assert_eq!(t.len(), 2);
    }

    /// 记录终止调用的 fake（只记 pid，不起进程）。
    #[derive(Default)]
    struct RecordingReaper {
        killed: Mutex<Vec<u32>>,
        fail_with: Option<String>,
        already_gone: bool,
        /// 前 N 次 `kill` 调用返回失败（模拟"句柄已保留，可重试"），之后转为成功。
        fail_first: AtomicU32,
    }

    impl RecordingReaper {
        fn killed(&self) -> Vec<u32> {
            self.killed.lock().clone()
        }
    }

    impl LeaseReaper for RecordingReaper {
        fn kill(&self, pid: u32) -> Result<ReapOutcome, String> {
            self.killed.lock().push(pid);
            if self.fail_first.load(Ordering::SeqCst) > 0 {
                self.fail_first.fetch_sub(1, Ordering::SeqCst);
                return Err("拒绝访问（句柄已保留，可重试）".into());
            }
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
        // 留痕之外还必须有人接着管：pid 进重试队列（V7 §7 的 retry 腿）。
        assert_eq!(s.pending, 1, "只留日志 = 断链，失败的 pid 必须排队待重试");
        assert_eq!(t.pending_reaps.len(), 1);
        assert_eq!(t.pending_reaps[0].pid, 9);
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

    /// `tauron-proc::kill` 失败时把子进程句柄放回跟踪表并在错误里写「可重试」——
    /// 这一侧必须真有人重试，否则那句承诺是断链（V7 §7 Process 行）。
    #[test]
    fn driver_retries_a_failed_reap_and_recovers_it() {
        let mut t = RuntimeTable::new();
        let reaper =
            Arc::new(RecordingReaper { fail_first: AtomicU32::new(1), ..Default::default() });
        t.set_reaper(reaper.clone());
        t.register("com.example.proc", 777);
        assert!(t.remove_plugin("com.example.proc").is_some());
        assert_eq!(
            (t.reap_stats().attempts, t.reap_stats().failures, t.reap_stats().pending),
            (1, 1, 1),
            "首试失败后必须排队待重试"
        );

        assert_eq!(t.retry_pending_reaps(), 1, "这一轮真的把 pid 停掉了");
        let s = t.reap_stats();
        assert_eq!(
            (s.attempts, s.retries, s.terminated, s.recovered, s.pending, s.terminal),
            (2, 1, 1, 1, 0, 0),
            "重试成功要同时体现在发号、出队与 recovered 上"
        );
        assert_eq!(reaper.killed(), vec![777, 777], "重试必须打在同一个 pid 上");
        assert!(s.terminal_records.is_empty(), "已经回收掉的不该留终态证据");
        // 队列空了以后驱动不得无中生有。
        assert_eq!(t.retry_pending_reaps(), 0);
        assert_eq!(t.reap_stats().retries, 1);
    }

    /// 重试到上限即**固化为终态证据**：既不许无限重试，也不许把失败蒸发掉。
    #[test]
    fn exhausted_reap_retries_become_terminal_evidence() {
        let mut t = RuntimeTable::new();
        t.set_reaper(Arc::new(RecordingReaper {
            fail_with: Some("权限不足".into()),
            ..Default::default()
        }));
        t.register("com.example.proc", 555);
        t.remove_plugin("com.example.proc");

        assert_eq!(t.retry_pending_reaps(), 0, "第二次仍失败");
        assert_eq!(t.reap_stats().pending, 1, "未达上限前必须留在队列里");
        assert_eq!(t.retry_pending_reaps(), 0, "第三次仍失败 ⇒ 到顶");

        let s = t.reap_stats();
        assert_eq!(
            (s.attempts, s.retries, s.failures, s.recovered, s.pending, s.terminal),
            (3, 2, 3, 0, 0, 1),
            "到顶后出队 + 固化，一次不多"
        );
        assert_eq!(s.terminal_records.len(), 1);
        let record = &s.terminal_records[0];
        assert_eq!(record.plugin_id, "com.example.proc");
        assert_eq!(record.pid, 555, "证据必须带得走 pid，否则查不到是谁还活着");
        assert_eq!(record.attempts, MAX_REAP_RETRY_ATTEMPTS);
        assert!(record.reason.contains("权限不足"), "原因不能只剩一个计数：{}", record.reason);
    }

    /// V7 §9 leak gate：终止失败风暴下，队列与证据都必须**有界**。
    #[test]
    fn reap_failure_storm_keeps_queue_and_evidence_bounded() {
        const FAILS: u32 = 400;
        let mut t = RuntimeTable::new();
        t.set_reaper(Arc::new(RecordingReaper {
            fail_with: Some("拒绝访问".into()),
            ..Default::default()
        }));

        for i in 0..FAILS {
            let id = format!("com.leak.p{i}");
            t.register(&id, 1000 + i);
            assert!(t.remove_plugin(&id).is_some());
            assert!(
                t.pending_reaps.len() <= MAX_PENDING_REAPS,
                "待重试队列不得越过上界（第 {i} 次失败后为 {}）",
                t.pending_reaps.len()
            );
        }

        let s = t.reap_stats();
        assert_eq!(s.pending, MAX_PENDING_REAPS, "队列钉在上界，不随失败次数增长");
        assert_eq!(
            s.overflow,
            (FAILS as usize - MAX_PENDING_REAPS) as u32,
            "被拒的每一次都必须计数，不能静默丢弃"
        );
        assert_eq!(s.terminal, s.overflow, "溢出直接固化为终态证据");
        assert_eq!(
            s.terminal_records.len(),
            MAX_TERMINAL_REAPS,
            "证据台账本身也有界（环形覆盖最旧）"
        );
        // 驱动一轮：上界之内的条目照常推进，队列长度不会因驱动而暴涨。
        let before = t.reap_stats().pending;
        assert_eq!(t.retry_pending_reaps(), 0);
        assert_eq!(t.reap_stats().pending, before, "全部仍失败 ⇒ 留在队列，一条不少");
    }

    /// 同一 pid 反复失败只留**一条**待重试记录（队列不能被重复回收撑大）。
    #[test]
    fn repeated_failure_for_one_pid_queues_it_once() {
        let mut t = RuntimeTable::new(); // 不注入终止器 ⇒ 每次必失败
        for _ in 0..5 {
            t.register("com.example.proc", 311);
            assert!(t.remove_plugin("com.example.proc").is_some());
        }
        assert_eq!(t.pending_reaps.len(), 1, "同一 pid 只留一条");
        assert_eq!(t.reap_stats().pending, 1);
        assert_eq!(t.reap_stats().attempts, 5, "每次失败都要计到");

        // 后来补注入终止器 ⇒ 排队的那条必须还能被驱动救回来。
        let reaper = Arc::new(RecordingReaper::default());
        t.set_reaper(reaper.clone());
        assert_eq!(t.retry_pending_reaps(), 1);
        assert_eq!(reaper.killed(), vec![311]);
        assert_eq!(t.reap_stats().pending, 0);
        assert_eq!(t.reap_stats().recovered, 1);
    }

    /// **重试绝不能误杀同号的活进程**（轮 27）。
    ///
    /// 真实形态：卸载时"杀不掉"⇒ 排队；随后进程自己退出，OS 把同一个 pid 给了新起的
    /// sidecar。终止器按 **pid** 查自己的跟踪表，无法区分"旧的那个已经死了"与
    /// "同号的新进程正活着"——拿着旧回收记录再打一下，杀的就是宿主刚起的那条活租约。
    #[test]
    fn reap_retry_yields_when_a_live_lease_owns_the_pid() {
        let reaper = Arc::new(RecordingReaper {
            fail_with: Some("拒绝访问".into()),
            ..Default::default()
        });
        let mut t = RuntimeTable::new();
        t.set_reaper(reaper.clone());

        t.register("com.example.proc", 77);
        assert!(t.remove_plugin("com.example.proc").is_some());
        assert_eq!(t.reap_stats().pending, 1, "首次杀不掉 ⇒ 排队等重试");

        // OS 重用同号：新租约拿到同一个 pid（这里是最贴切的"卸载后重装并起来"）。
        let live = t.register("com.example.proc", 77);
        let before = reaper.killed();

        assert_eq!(t.retry_pending_reaps(), 0, "让位不是回收成功");
        assert_eq!(reaper.killed(), before, "不得对活 pid 再打一下");

        let s = t.reap_stats();
        assert_eq!(s.pending, 0, "让位必须出队，不能永远占着队列");
        assert_eq!(s.skipped_live_pid, 1);
        assert_eq!(s.retries, 0, "没打就没试：让位不占重试次数");
        assert_eq!(s.terminal, 1, "让位也要留证据，不能只算'这次没做'");
        assert_eq!(s.recovered, 0);
        let rec = &s.terminal_records[0];
        assert_eq!(rec.plugin_id, "com.example.proc");
        assert_eq!(rec.pid, 77);
        assert!(rec.reason.contains("误杀"), "原因必须写清为什么让位：{:?}", rec.reason);
        assert_eq!(
            t.handle_of_plugin("com.example.proc").unwrap().lease,
            live.lease,
            "在册租约完全不受影响"
        );
    }

    /// 让位只针对**同号**：不同 pid 的在册租约不得把旧回收记录挡下来，
    /// 否则这条封口会变成"永远不重试"的借口。
    #[test]
    fn reap_retry_still_runs_when_the_live_lease_has_a_different_pid() {
        let reaper =
            Arc::new(RecordingReaper { fail_first: AtomicU32::new(1), ..Default::default() });
        let mut t = RuntimeTable::new();
        t.set_reaper(reaper.clone());

        t.register("com.example.proc", 88);
        t.remove_plugin("com.example.proc"); // 首试失败 ⇒ 排队 88
        t.register("com.example.proc", 99); // 新进程换了号
        assert_eq!(t.retry_pending_reaps(), 1, "号不同就必须照旧重试");
        assert_eq!(reaper.killed(), vec![88, 88], "旧 pid 被打过两次（首试 + 重试）");
        assert_eq!(t.reap_stats().skipped_live_pid, 0);
        assert_eq!(t.reap_stats().recovered, 1);
        assert!(t.handle_of_plugin("com.example.proc").is_some(), "新租约还在");
    }

    // ── 轮 41：跨重启回收台账 + 启动扫描 ──────────────────────────────────

    /// 确定性探针替身：只按预置结论回答。
    struct FixedProbe(PidLiveness);

    impl PidProbe for FixedProbe {
        fn probe(&self, _pid: u32) -> PidLiveness {
            self.0
        }
    }

    /// 在目录里种一份**上一轮宿主**留下的台账（pending 一条）。
    fn plant_restart_ledger(dir: &Path, pid: u32) {
        let payload = ReapLedgerPayload {
            pending: vec![ReapLedgerEntry {
                plugin_id: "com.escaped.proc".to_string(),
                pid,
                attempts: 2,
                reason: "终止失败：拒绝访问".to_string(),
            }],
            terminal: vec![],
        };
        let envelope = DurableEnvelope::seal(REAP_LEDGER_SCHEMA, 1, payload).unwrap();
        crate::durable::write_durable(
            &dir.join(REAP_LEDGER_FILE),
            &encode_durable(&envelope).unwrap(),
        )
        .unwrap();
    }

    /// 台账跨重启往返：上一轮失败入队的条目在新表里读回（分池），
    /// 扫描按探测结论定性、销账、落盘；终态证据随台账读回但在新表里不虚增计数。
    #[test]
    fn reap_ledger_round_trips_across_a_restart_scan() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = RuntimeTable::new();
        first.set_reaper(Arc::new(RecordingReaper {
            fail_with: Some("拒绝访问".into()),
            ..Default::default()
        }));
        first.configure_reap_ledger(dir.path()).unwrap();
        first.register("com.example.proc", 9001);
        first.remove_plugin("com.example.proc"); // 首试失败 ⇒ 入队
        assert_eq!(first.reap_stats().pending, 1);

        let (path, bytes) = first.take_reap_ledger_write().expect("有脏变更就该出快照");
        crate::durable::write_durable(&path, &bytes).unwrap();
        assert!(dir.path().join(REAP_LEDGER_FILE).exists());

        // 「重启」：新表读回上一轮条目，**分池**——不进杀进程重试腿。
        let mut second = RuntimeTable::new();
        second.configure_reap_ledger(dir.path()).unwrap();
        assert_eq!(second.restart_reaps.len(), 1);
        assert_eq!(second.restart_reaps[0].pid, 9001);
        assert_eq!(second.restart_reaps[0].attempts, 1);
        assert!(second.pending_reaps.is_empty(), "读回条目不许直接进重试腿");

        // 探测说进程已不存在 ⇒ 销账 + 终态证据（永不盲杀本就没对象可杀）。
        second.set_pid_probe(Arc::new(FixedProbe(PidLiveness::Gone)));
        assert_eq!(second.sweep_restart_reaps(), 1);
        let s = second.reap_stats();
        assert_eq!((s.sweep_resolved, s.sweep_survivors, s.sweep_unknown), (1, 0, 0));
        assert_eq!(s.already_gone, 1);
        assert_eq!(s.terminal, 1);
        assert!(s.terminal_records[0].reason.contains("重启扫描销账"));

        let (path, bytes) = second.take_reap_ledger_write().expect("扫描后必须落一份新账");
        crate::durable::write_durable(&path, &bytes).unwrap();
        let decoded: DurableEnvelope<ReapLedgerPayload> =
            decode_durable(&std::fs::read(&path).unwrap()).unwrap();
        assert!(decoded.payload.pending.is_empty(), "销账后待重试池必须清空");
        assert_eq!(decoded.payload.terminal.len(), 1);
        assert_eq!(decoded.generation, 2, "落盘沿代数单调");

        // 第三张表（又一次重启）读回终态证据：状态并回，不是新增事件。
        let mut third = RuntimeTable::new();
        third.configure_reap_ledger(dir.path()).unwrap();
        assert_eq!(third.terminal_reaps.len(), 1, "上一轮证据随台账读回");
        assert_eq!(third.reap_stats().terminal, 0, "读回不许虚增终态计数");
        assert!(third.restart_reaps.is_empty(), "销过账的条目不得复活");
    }

    /// 轮 41 的核心安全封口：扫描**绝不**对上一轮条目开枪——哪怕探测说它还活着。
    #[test]
    fn restart_sweep_never_kills_even_when_the_probe_says_alive() {
        let dir = tempfile::tempdir().unwrap();
        plant_restart_ledger(dir.path(), 7001);

        let mut t = RuntimeTable::new();
        let reaper = Arc::new(RecordingReaper::default());
        t.set_reaper(reaper.clone());
        t.configure_reap_ledger(dir.path()).unwrap();
        t.set_pid_probe(Arc::new(FixedProbe(PidLiveness::Alive)));

        assert_eq!(t.sweep_restart_reaps(), 0, "存活不是销账");
        assert!(reaper.killed().is_empty(), "跨重启扫描对任何 pid 都不得调用终止");
        let s = t.reap_stats();
        assert_eq!((s.sweep_resolved, s.sweep_survivors, s.sweep_unknown), (0, 1, 0));
        assert!(s.terminal_records[0].reason.contains("不盲杀"), "证据要说清为什么没杀");

        // 分池铁律：重启条目不会滑进杀进程重试腿——再驱动一轮也不开枪。
        assert_eq!(t.retry_pending_reaps(), 0, "重启条目不得进入重试腿");
        assert!(reaper.killed().is_empty());
    }

    /// 探测不可用（未注入 / Unknown）：不杀、不猜，同样留证。
    #[test]
    fn unknown_or_missing_probe_is_evidence_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        plant_restart_ledger(dir.path(), 7003);

        let mut t = RuntimeTable::new();
        let reaper = Arc::new(RecordingReaper::default());
        t.set_reaper(reaper.clone());
        t.configure_reap_ledger(dir.path()).unwrap();
        // 未注入探针：结论只能是 Unknown。
        assert_eq!(t.sweep_restart_reaps(), 0, "没有结论就不许销账");
        assert_eq!(t.reap_stats().sweep_unknown, 1);
        assert!(t.reap_stats().terminal_records[0].reason.contains("探测不可用"));
        assert!(reaper.killed().is_empty());
    }

    /// 撕裂/篡改的台账 = 拒绝（装配期不得静默重置成空账）。
    #[test]
    fn torn_or_tampered_ledger_is_rejected_not_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REAP_LEDGER_FILE);

        std::fs::write(&path, b"{not json").unwrap();
        let mut torn = RuntimeTable::new();
        assert!(matches!(
            torn.configure_reap_ledger(dir.path()),
            Err(ReapLedgerError::Integrity(_))
        ));
        assert!(torn.restart_reaps.is_empty(), "拒绝后不得留下半截账");

        // 合法 JSON 但载荷被改（校验和必须咬住）。
        plant_restart_ledger(dir.path(), 7001);
        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replace("\"pid\":7001", "\"pid\":7002");
        assert!(tampered.contains("\"pid\":7002"), "改动必须真的落在文件里");
        std::fs::write(&path, tampered).unwrap();
        let mut forged = RuntimeTable::new();
        assert!(matches!(
            forged.configure_reap_ledger(dir.path()),
            Err(ReapLedgerError::Integrity(_))
        ));
    }

    /// 首启（或上一轮干净）：空扫描也落一份"已扫描"的账；没脏就不重复写。
    #[test]
    fn empty_sweep_still_records_a_scanned_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = RuntimeTable::new();
        t.configure_reap_ledger(dir.path()).unwrap();
        assert_eq!(t.sweep_restart_reaps(), 0);

        let (path, bytes) = t.take_reap_ledger_write().expect("空账也要落：'扫描过'本身是可查事实");
        crate::durable::write_durable(&path, &bytes).unwrap();
        let decoded: DurableEnvelope<ReapLedgerPayload> =
            decode_durable(&std::fs::read(&path).unwrap()).unwrap();
        assert!(decoded.payload.pending.is_empty());
        assert!(decoded.payload.terminal.is_empty());

        assert!(t.take_reap_ledger_write().is_none(), "取过快照后无脏变更，不许重复写");
    }

    /// 无盘宿主（未配置目录）：标脏后取快照是 `None`，不得 panic，也不伪造写失败。
    #[test]
    fn unconfigured_table_yields_no_snapshot_and_no_fake_failure() {
        let mut t = RuntimeTable::new();
        t.register("com.example.proc", 1);
        t.remove_plugin("com.example.proc"); // 无 reaper ⇒ 失败 ⇒ 入队 ⇒ 标脏
        assert_eq!(t.reap_stats().pending, 1);
        assert!(t.take_reap_ledger_write().is_none());
        assert_eq!(t.reap_stats().ledger_write_failures, 0, "没有路径不是写失败");
    }
}
