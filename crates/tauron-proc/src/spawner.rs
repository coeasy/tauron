// 进程执行器的**可注入启动面**（P0-2：让 `PluginType::Process` 有真实执行器入口）。
//
// 为什么必须是 trait：真启动要 fork/exec 一个外部二进制，而单元测试里真起 sidecar
// 会让 CI 变 flaky（进程泄漏、时序竞态、平台差异、CI 上没有 sidecar 二进制）。
// 把「启动 / 探测存活 / 终止」收在这一个 trait 后面，生产实现用
// `std::process::Command`，测试用 fake——于是执行器语义（租约登记、崩溃计数、
// 事件投递、租约回收时的终止）可以完全离线、确定性地验证，而生产实现只需保证
// 同一份契约。
//
// 本模块不依赖 `tauri`，也不认识任何宿主类型（`SpawnConfig` 是唯一输入，
// `SpawnedProc` 是唯一输出）。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{validate_spawn_config, ProcError, ProcResult, SpawnConfig};

/// 一次成功启动的产物。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnedProc {
    /// 操作系统进程号。
    pub pid: u32,
}

/// 终止动作的结果。
///
/// 为什么把"早就退出了"与"这次真杀了"分开报，而不是都算成功/都算失败：
/// 上层要留痕（终止失败必须可查，不能静默吞），但**崩溃后回收租约**（进程早已
/// 退出）是常态——把它混进"失败"会让真正的失败（权限不足、句柄失效、杀不掉）
/// 淹没在噪声里，计数器就失去信号。
///
/// 目前**不跨 IPC**（终止只发生在宿主内部的租约回收路径上）；保留 camelCase
/// 序列化是为了将来做诊断命令面时不必再改线形。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KillOutcome {
    /// 本次真的终止了进程，并已 `wait` 回收（Unix 无僵尸 / Windows 句柄已释放）。
    Terminated,
    /// 进程**此前已经退出**：本次没有杀任何东西，但"没有存活进程"这一目标已达成。
    AlreadyGone,
}

/// V4 process liveness contract. `Unknown` is deliberately distinct from `Alive`:
/// losing the Child handle or failing `try_wait` must not be advertised as a healthy runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessStatus {
    Alive,
    Exited,
    Unknown,
}

impl ProcessStatus {
    pub fn is_proven_alive(self) -> bool {
        matches!(self, ProcessStatus::Alive)
    }
}

/// V4 A97 machine-readable process isolation strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessSandboxEnforcement {
    Unsupported,
    Partial,
    Hard,
}

impl ProcessSandboxEnforcement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Partial => "partial",
            Self::Hard => "hard",
        }
    }
}

/// Honest process sandbox capability descriptor.
///
/// A provider must not claim `Hard` unless process-tree containment and all declared security
/// dimensions are enforced by the OS primitive it installs before exec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessSandboxDescriptor {
    pub enforcement: ProcessSandboxEnforcement,
    pub process_tree_containment: bool,
    pub filesystem_isolation: bool,
    pub network_isolation: bool,
    pub syscall_isolation: bool,
    pub detail: String,
}

impl ProcessSandboxDescriptor {
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self {
            enforcement: ProcessSandboxEnforcement::Unsupported,
            process_tree_containment: false,
            filesystem_isolation: false,
            network_isolation: false,
            syscall_isolation: false,
            detail: detail.into(),
        }
    }
}

/// OS-specific sandbox provider SPI.
///
/// `configure` is called after spawn-config validation but before `Command::spawn`. Returning an
/// error is fail-closed: no child is created. The default Tauron provider is intentionally
/// unsupported and leaves the command unchanged; therefore capability reporting never confuses
/// "can spawn a child" with "child is sandboxed".
pub trait ProcessSandboxProvider: Send + Sync {
    fn descriptor(&self) -> ProcessSandboxDescriptor;
    fn configure(&self, command: &mut Command, cfg: &SpawnConfig) -> ProcResult<()>;

    /// Terminate the provider-owned process tree for `pid`.
    ///
    /// `Ok(true)` means the containment primitive accepted the termination request; the caller
    /// still waits/reaps the direct child. `Ok(false)` means this provider has no tree boundary
    /// for the pid and the caller should fall back to direct-child termination.
    fn terminate_tree(&self, _pid: u32) -> ProcResult<bool> {
        Ok(false)
    }
}

/// Compatibility provider used by the built-in CommandSpawner until an OS sandbox is installed.
#[derive(Debug, Default)]
pub struct UnsupportedProcessSandboxProvider;

impl ProcessSandboxProvider for UnsupportedProcessSandboxProvider {
    fn descriptor(&self) -> ProcessSandboxDescriptor {
        ProcessSandboxDescriptor::unsupported(
            "direct child process only; no process-group/job-object containment, filesystem/network namespace, or syscall sandbox",
        )
    }

    fn configure(&self, _command: &mut Command, _cfg: &SpawnConfig) -> ProcResult<()> {
        Ok(())
    }
}

/// Unix/macOS built-in process-tree containment.
///
/// Each sidecar becomes leader of a fresh POSIX process group before exec. Explicit teardown and
/// parent-crash cleanup send SIGKILL to the negative pgid, so descendants that inherited the group
/// cannot outlive the runtime lease. This remains `Partial`: process groups do not isolate
/// filesystem, network, or syscalls.
#[cfg(unix)]
#[derive(Debug, Default)]
pub struct UnixProcessGroupSandboxProvider;

#[cfg(unix)]
impl ProcessSandboxProvider for UnixProcessGroupSandboxProvider {
    fn descriptor(&self) -> ProcessSandboxDescriptor {
        ProcessSandboxDescriptor {
            enforcement: ProcessSandboxEnforcement::Partial,
            process_tree_containment: true,
            filesystem_isolation: false,
            network_isolation: false,
            syscall_isolation: false,
            detail: "POSIX process-group containment + group termination; filesystem/network/syscall isolation not provided".into(),
        }
    }

    fn configure(&self, command: &mut Command, _cfg: &SpawnConfig) -> ProcResult<()> {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        Ok(())
    }

    fn terminate_tree(&self, pid: u32) -> ProcResult<bool> {
        let pgid = i32::try_from(pid).map_err(|_| {
            ProcError::ProcessTerminated(format!("pid {pid} cannot be represented as POSIX pid_t"))
        })?;
        if pgid <= 0 {
            return Err(ProcError::ProcessTerminated(format!(
                "refusing to signal invalid process group {pgid}"
            )));
        }
        // SAFETY: negative pid targets exactly the process group created in configure().
        let rc = unsafe { libc::kill(-pgid, libc::SIGKILL) };
        if rc == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        Err(ProcError::ProcessTerminated(format!("terminate process group {pgid} failed: {error}")))
    }
}

fn default_process_sandbox_provider() -> Arc<dyn ProcessSandboxProvider> {
    #[cfg(unix)]
    {
        Arc::new(UnixProcessGroupSandboxProvider)
    }
    #[cfg(not(unix))]
    {
        Arc::new(UnsupportedProcessSandboxProvider)
    }
}

/// 进程启动器（可注入）。
///
/// **契约**：`spawn` 返回 `Ok` 即表示操作系统层面**真的**有一个进程在跑，
/// 且它的 pid 是该返回值——上层据此铸租约（lease ↔ pid 一一绑定）。失败必须
/// 返回 `Err`，**不得**返回一个假 pid（那会让租约指向不存在的进程，而
/// `runtime_health` 会把它报成崩溃，故障点被彻底演没）。
pub trait ProcSpawner: Send + Sync {
    /// Honest sandbox capability. Legacy/custom spawners default to unsupported.
    fn sandbox_descriptor(&self) -> ProcessSandboxDescriptor {
        ProcessSandboxDescriptor::unsupported("spawner did not provide a ProcessSandboxProvider")
    }

    /// 启动 sidecar。
    fn spawn(&self, cfg: &SpawnConfig) -> ProcResult<SpawnedProc>;

    /// Tri-state process status. Compatibility implementations that only provide `is_alive`
    /// map their explicit boolean result to Alive/Exited.
    fn status(&self, pid: u32) -> ProcessStatus {
        if self.is_alive(pid) {
            ProcessStatus::Alive
        } else {
            ProcessStatus::Exited
        }
    }

    /// Legacy boolean probe. Implementations without probe support remain conservative.
    ///
    /// Production callers should prefer [`ProcSpawner::status`] so Unknown is not confused
    /// with a proven-alive process.
    fn is_alive(&self, pid: u32) -> bool {
        let _ = pid;
        true
    }

    /// 终止进程（卸载 / 清除 / 租约换新时调用）。
    ///
    /// **刻意没有缺省实现**：终止能力的有无必须由每个实现显式表态。给一个
    /// "什么都不做但返回 `Ok`"的缺省实现就是静默漏杀——卸载后进程还在跑，
    /// 而调用方以为已经回收（正是"卸载留下孤儿进程"这个缺口的成因）。
    /// 做不到的实现必须返回 `Err`，上层会把它计入留痕，但**不会**因此让
    /// 卸载整体失败。
    fn kill(&self, pid: u32) -> ProcResult<KillOutcome>;

    /// 写入一帧到 sidecar 的 stdin（JSON-RPC 行帧，以 `\n` 结尾）。
    ///
    /// 缺省返回 `Err`：**没有 stdin 写入能力的实现不得假装能写**——否则宿主侧的
    /// 投递会静默落到黑洞（调用方以为已送达，sidecar 其实没收到）。
    fn write_frame(&self, _pid: u32, _frame: &[u8]) -> std::io::Result<()> {
        let _ = (_pid, _frame);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "该启动器不支持写入 sidecar stdin",
        ))
    }

    /// 注册一个 stdout 帧接收器：sidecar 每写一帧（JSON-RPC 响应 / 事件）到 stdout，
    /// 启动器就把该行帧交给 `sink.on_frame`。
    ///
    /// 返回 `false` 表示该启动器不支撑 stdout 帧路由（如模拟启动器）——调用方应
    /// 据此知道"投递了也没人回帧"。缺省实现返回 `false`。
    fn register_frame_sink(&self, _pid: u32, _sink: Arc<dyn ProcessFrameSink>) -> bool {
        let _ = (_pid, _sink);
        false
    }
}

/// sidecar stdout 帧接收器（§4.7 JSON-RPC 回路的宿主侧终点）。
///
/// `CommandSpawner` 的读线程持续排空 sidecar 的 stdout，每读到一行协议帧就调用
/// 一次 `on_frame`。宿主借此把 sidecar 的回帧（含 `callId`）路由回注册表的
/// `settle_call`，闭合「宿主 → sidecar → 回帧 → 结算」这条此前断在 `Stdio::null()`
/// 的链路。
pub trait ProcessFrameSink: Send + Sync {
    /// 收到一帧（已是去尾换行的原始字节）。
    fn on_frame(&self, pid: u32, frame: &[u8]);

    /// stdout 关闭（进程退出 / 管道 EOF）时调用一次，便于上层清理；缺省空实现。
    fn on_eof(&self, _pid: u32) {}
}

/// 一次 `close` 的结果：既交还 sink，也报告「本读线程是否还代表当前代次」。
struct CloseOutcome {
    /// 本读线程对应的 sink（若仍挂在表上），交还给调用方触发 `on_eof`。
    sink: Option<Arc<dyn ProcessFrameSink>>,
    /// `false` = 该 pid **已被复用**（新进程占用同一 pid 并开启了新代次）。
    /// 此时本读线程是"上一代"的遗骸，**不得**再清理该 pid 的 stdin / child 表项
    /// ——那些条目已经属于新进程，误删就是把新进程的管道与句柄摘掉（静默断链）。
    is_current: bool,
}

/// 表上的一条帧接收器登记：**带代次**。
struct LiveSink {
    /// 登记时该 pid 的代次。
    gen: u64,
    sink: Arc<dyn ProcessFrameSink>,
}

/// sidecar stdout 的**进程级登记**：帧接收器、「已关闭」标记与**代次**。
///
/// 为什么必须放在**同一把锁**下（1.0-W7 修复 P0-7①）：此前 `sinks` 与 `closed`
/// 是两把独立的锁，`register_frame_sink` 先查 `closed` 再插 `sinks`，而读线程
/// EOF 时先删 `sinks` 再插 `closed`——两次加锁之间可交错：
///
/// ```text
/// 读线程: sinks.remove(pid) ──────────────► closed.insert(pid)
/// 注册方:              closed 检查(未命中) ──► sinks.insert(pid)   ← 永久残留
/// ```
///
/// 交错后 sink 在 `closed` 置位之后才插入，而读线程已经退出、永远不会再清理它
/// ——反复「崩溃 → 重启」即**无界累积**。合并成一把锁后「检查 + 插入」原子，
/// 交错在结构上不可能发生。
///
/// **为什么还要代次（generation）**：pid 会被操作系统复用（Windows 上尤其常见，
/// 进程对象一关闭就可能回收号段）。没有代次时会出现两处**静默断链**：
///
/// 1. 上一代读线程的 EOF 迟到，把 `closed` 置到**复用后的新 pid** 上 →
///    新进程 `register_frame_sink` 被拒 → 它的回帧**永远无人接收**，
///    「宿主 → sidecar → 回帧 → 结算」在新进程上断掉，且不报任何错；
/// 2. 同一个迟到的读线程还会 `children.remove(pid)` / `stdin_writers.remove(pid)`
///    ——摘走的是**新进程**的句柄与 stdin，帧投递立刻 `NotFound`。
///
/// 代次把「pid」变成「pid + 本启动器的第几次使用」：每次 `spawn` 都 `begin` 出
/// 新代次，读线程闭包捕获自己那一代；`close` 时若代次不符即判为遗骸，
/// 既不写 `closed` 也不碰表项。
///
/// **为什么会无界增长**：`closed` / `generation` 按 pid 累积，而 pid 是会被回收
/// 复用的有限空间。两者都由 [`SinkTable::retain_tracked`] 在每次 `spawn` 时
/// 按「仍在 `children` 跟踪表里」裁剪，配合 `register_frame_sink` 的
/// 「不在跟踪表 = 拒绝登记」前提（没有跟踪表条目就没有读线程，登记必然是悬空的），
/// 使裁剪不会重新打开 P0-7① 的泄漏口。
#[derive(Default)]
struct SinkTable {
    /// pid → 当前代次已登记的帧接收器。
    live: HashMap<u32, LiveSink>,
    /// pid → 已 EOF 的代次（仅当代次仍为当前代次时写入）。
    closed: HashMap<u32, u64>,
    /// pid → 当前代次（每次 `begin` 递增）。
    generation: HashMap<u32, u64>,
    /// 下一个代次号（进程内单调递增）。
    next_generation: u64,
}

impl SinkTable {
    /// 开启该 pid 的**新代次**：递增代次、清掉旧的 `closed` 标记。
    ///
    /// 返回 `(新代次, 上一代次遗留的 sink)`——后者若存在说明 pid 已被复用而上一代
    /// 读线程还没来得及 EOF，调用方应在**锁外**替它触发 `on_eof`（否则那个 sink
    /// 再也没人通知）。
    fn begin(&mut self, pid: u32) -> (u64, Option<Arc<dyn ProcessFrameSink>>) {
        let gen = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        self.generation.insert(pid, gen);
        // 上一代的关闭标记属于旧进程，必须清掉，否则新进程永远登记不上。
        self.closed.remove(&pid);
        let stale = self.live.remove(&pid).map(|l| l.sink);
        (gen, stale)
    }

    /// 当前代次（未登记过的 pid 视为代次 0，供 `register` 兜底）。
    fn current_generation(&self, pid: u32) -> u64 {
        self.generation.get(&pid).copied().unwrap_or(0)
    }

    /// 原子地「查 closed → 插入」。返回 `false` = 已关闭，拒绝迟到登记。
    fn register(&mut self, pid: u32, sink: Arc<dyn ProcessFrameSink>) -> bool {
        if self.closed.contains_key(&pid) {
            return false;
        }
        self.live.insert(pid, LiveSink { gen: self.current_generation(pid), sink });
        true
    }

    /// 取本读线程那一代的 sink（每帧调用；clone 出 `Arc` 后即可释放锁）。
    /// 代次不符 = pid 已复用，旧读线程的帧不得再投给新进程。
    fn get(&self, pid: u32, gen: u64) -> Option<Arc<dyn ProcessFrameSink>> {
        self.live.get(&pid).filter(|l| l.gen == gen).map(|l| l.sink.clone())
    }

    /// 标记关闭并摘除 sink（同一把锁内完成）。代次不符时视为遗骸：
    /// 不写 `closed`（否则会把新进程的登记口焊死）。
    fn close(&mut self, pid: u32, gen: u64) -> CloseOutcome {
        let is_current = self.generation.get(&pid).is_none_or(|g| *g == gen);
        if is_current {
            self.closed.insert(pid, gen);
        }
        let sink = match self.live.get(&pid) {
            Some(l) if l.gen == gen => self.live.remove(&pid).map(|l| l.sink),
            _ => None,
        };
        CloseOutcome { sink, is_current }
    }

    /// 裁掉「已不在跟踪表里」的 pid 的 `closed` / `generation` 条目。
    ///
    /// 只在 `spawn` 持有 `children` 锁时调用（见 `retain_tracked` 的调用点），
    /// 保证「裁掉的 pid 必然拿不到跟踪表条目」，而 `register_frame_sink`
    /// 对无跟踪表条目的 pid 一律拒绝——因此裁剪不会重新放行悬空登记。
    fn retain_tracked(&mut self, tracked: &std::collections::HashSet<u32>) {
        self.closed.retain(|pid, _| tracked.contains(pid));
        self.generation.retain(|pid, _| tracked.contains(pid));
    }
}

/// 生产实现：`std::process::Command`。
///
/// 做四件事——**真启动**（`Command::spawn`）、**真探测**（`Child::try_wait`）、
/// **真终止**（`Child::kill` + `wait` 回收）、**真帧回路**（stdin 写请求 /
/// stdout 读线程回帧）。不再多做，也不假装多做：
///
/// **诚实边界（0.4-A1 已接线部分）**
/// - §4.7 的 JSON-RPC 帧回路**已接线**：stdin/stdout 走 `Stdio::piped()`，且
///   每条 sidecar 进程在 `spawn` 时**单独起一个读线程**持续排空 stdout——这就
///   化解了此前"接管道却没人读 → sidecar 写满缓冲区被阻塞死"的风险（见下面
///   「未验证」里剩下的真实 sidecar 端到端缺口）。
/// - 读线程每读到一行协议帧就交给该 pid 注册的 [`ProcessFrameSink`]；无 sink 时
///   静默丢弃（调用方尚未注册；sidecar 不应在收到请求前自发帧）。
///
/// **1.0-W7 加固（本轮）**
/// - `SinkTable` 单锁：登记与关闭原子化，消除迟到登记的泄漏竞态（P0-7①）。
/// - 写帧**不再持全局锁**：`stdin_writers` 是两级锁（外层只用于取 `Arc`，
///   内层才是 per-pid 的阻塞写）——一个不读 stdin 的 sidecar 只阻塞**它自己**
///   的帧投递，不再拖住所有进程（P0-7②）。
/// - 单帧**长度上限** `MAX_FRAME_BYTES`：用 `Read::take` 限制，超长行（无换行）
///   不会无限增长把宿主 OOM（P0-7③）。
/// - EOF 时**主动 `try_wait` 回收**已退出的子进程（不 wait 会留僵尸），
///   仅在「进程仍存活但关了 stdout」这种病态情形下保留句柄（P2-4）。
/// - EOF 时调用 sink 的 `on_eof`（此前该钩子在生产路径上零调用，P2-3）。
///
/// **未验证部分（诚实标注）**
/// - **真实 sidecar 端到端**：本仓**没有**可执行的 sidecar 二进制，测试也明确
///   **不起真进程**（CI flaky / 平台差异）。因此"sidecar 真的收到帧、真的回帧、
///   宿主真的据此结算"这一整条链路**没有运行期证据**——验证的是帧格式、
///   `write_frame`/`register_frame_sink` 的契约、以及读线程排空逻辑（用 mock
///   spawner + 内存管道单测）。带真 sidecar 的 E2E 需另起集成测试环境。
/// - 不能保证跨平台「已退出」判定时机一致（`try_wait` 在子进程退出后返回
///   `Some(status)`）；僵尸进程的回收依赖本类型仍持有 `Child`。
/// - 本类型被**丢弃**时不会 `wait`/`kill`（`Child` 的 `Drop` 不做这两件事）：
///   此时尚未被观测到退出的子进程会留成僵尸，直到宿主进程自己退出。宿主正常
///   退出路径（卸载回收 / 进程退出）都会先走 `kill`，因此这条只覆盖异常路径。
/// - `kill` 的**正路**（真的杀掉一个活进程）在测试里未验证：验证它必须真的起一个
///   进程，本仓测试明确不起真进程。无需进程的路径（未知 pid / 已退出）有测试。
/// - Unix/macOS 默认 provider 已用独立 POSIX process group 覆盖进程树终止；Windows
///   默认 provider 仍为 unsupported，且所有平台的 filesystem/network/syscall 隔离仍
///   未内建，因此默认 capability 最多是 partial、Production 仍 fail-closed。
pub struct CommandSpawner {
    /// pid → `Child`。持有句柄是 `try_wait` 的前提（否则只能靠平台 API 探测，
    /// 那会引入平台分支与 unsafe）。stdin/stdout 已在 `spawn` 时 `take` 出去，
    /// 因此这里不再持有（不影响 `try_wait`/`kill`）。封 `Arc` 以便读线程在
    /// stdout EOF 时**主动回收**已退出的子进程（1.0-W7）。
    children: Arc<Mutex<HashMap<u32, Child>>>,
    /// pid → sidecar stdin 写句柄（宿主写 JSON-RPC 行帧用）。
    ///
    /// **两级锁**（1.0-W7）：外层锁只在「取该 pid 的 `Arc`」时短暂持有，
    /// 阻塞的 `write_all`/`flush` 只持**内层** per-pid 锁——因此一个不读 stdin
    /// 的 sidecar 不会拖住其他进程的帧投递。
    stdin_writers: Arc<Mutex<HashMap<u32, Arc<Mutex<ChildStdin>>>>>,
    /// 帧接收器 + 关闭标记（同一把锁，见 [`SinkTable`]）。
    sinks: Arc<Mutex<SinkTable>>,
    /// V4 A97 sandbox provider invoked before every OS spawn.
    sandbox_provider: Arc<dyn ProcessSandboxProvider>,
}

impl Default for CommandSpawner {
    fn default() -> Self {
        Self {
            children: Arc::new(Mutex::new(HashMap::new())),
            stdin_writers: Arc::new(Mutex::new(HashMap::new())),
            sinks: Arc::new(Mutex::new(SinkTable::default())),
            sandbox_provider: default_process_sandbox_provider(),
        }
    }
}

/// 单帧字节上限（1.0-W7）。超过即判定为协议违规并断开读线程——
/// 防 sidecar 用一条没有换行的超长行把宿主内存吃光。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 去掉行帧首尾的 ASCII 空白（含 `\n` / `\r`），返回子切片。
fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map(|i| i + 1).unwrap_or(start);
    &bytes[start..end]
}

impl CommandSpawner {
    /// 创建启动器。
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject an OS-specific V4 A97 sandbox provider.
    pub fn with_sandbox_provider(provider: Arc<dyn ProcessSandboxProvider>) -> Self {
        let mut spawner = Self::default();
        spawner.sandbox_provider = provider;
        spawner
    }

    pub fn sandbox_descriptor(&self) -> ProcessSandboxDescriptor {
        self.sandbox_provider.descriptor()
    }

    /// 当前被本启动器跟踪的进程数（诊断/测试）。
    pub fn tracked(&self) -> usize {
        self.children.lock().len()
    }

    /// 已 EOF（stdout 关闭）的 pid 数（诊断/测试）。
    pub fn closed_count(&self) -> usize {
        self.sinks.lock().closed.len()
    }

    /// 当前登记的帧接收器数（诊断/测试）。
    pub fn sink_count(&self) -> usize {
        self.sinks.lock().live.len()
    }
}

impl ProcSpawner for CommandSpawner {
    fn sandbox_descriptor(&self) -> ProcessSandboxDescriptor {
        self.sandbox_provider.descriptor()
    }

    fn spawn(&self, cfg: &SpawnConfig) -> ProcResult<SpawnedProc> {
        // spawn 前校验（§4.7 硬约束）：路径 / 签名 / sha256 / ABI 任一不合格
        // 都**不得**启动。校验在启动之前，因此不合格的配置连 syscall 都到不了。
        validate_spawn_config(cfg)?;

        let mut command = Command::new(&cfg.binary_path);
        command
            .args(&cfg.args)
            .envs(&cfg.env)
            // stderr 承载日志（§4.7：日志走 stderr），继承宿主 stderr 即可，
            // 不需要管道（也就没有管道写满阻塞的风险）。
            .stderr(Stdio::inherit())
            // stdin/stdout 必须接管道：宿主经 stdin 写 JSON-RPC 请求帧，sidecar
            // 经 stdout 回帧。stdout 由下方读线程**持续排空**，否则 sidecar 写满
            // 管道缓冲区会被阻塞死（比丢弃更糟）。
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());

        // V4 A97: sandbox policy is applied before the exec boundary. Provider errors are
        // fail-closed and therefore cannot leave a partially tracked child process.
        self.sandbox_provider.configure(&mut command, cfg)?;

        let mut child = command
            .spawn()
            .map_err(|e| ProcError::SpawnFailed(format!("启动 `{}` 失败：{e}", cfg.binary_path)))?;

        let pid = child.id();
        // 取出 stdin 写句柄（宿主写帧用）与 stdout（交给读线程排空）。
        // **任一失败都要回收子进程**：直接 `Err` 返回会把 `Child` 一丢了之——
        // 句柄 drop 不 kill/wait，进程留成无人跟踪的孤儿（0.4 审计修复）。
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            (stdin, stdout) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = (stdin, stdout); // 管道句柄随作用域关闭
                return Err(ProcError::SpawnFailed(
                    "sidecar stdin/stdout 管道未就绪（已回收子进程）".into(),
                ));
            }
        };
        self.children.lock().insert(pid, child);

        // 开启该 pid 的**新代次**并顺手裁剪陈旧登记。两把锁必须按
        // `children → sinks` 的固定顺序嵌套（全仓唯一的嵌套点；读线程只依次
        // 短暂持有各自的一把，不嵌套），否则「裁剪」与「新 pid 插入」之间
        // 会留下交错窗口。
        let (gen, stale_sink) = {
            let children = self.children.lock();
            let mut table = self.sinks.lock();
            let tracked: std::collections::HashSet<u32> = children.keys().copied().collect();
            table.retain_tracked(&tracked);
            table.begin(pid)
        };
        // pid 复用时上一代遗留的 sink：替它触发一次 `on_eof`（锁外），否则
        // 那个 sink 的所有者永远等不到关闭通知。
        if let Some(stale) = stale_sink {
            stale.on_eof(pid);
        }

        self.stdin_writers.lock().insert(pid, Arc::new(Mutex::new(stdin)));

        // 读线程：持续排空 sidecar 的 stdout，逐行交给该 pid 注册的 sink。
        // 读到 EOF（进程退出 / 关闭 stdout）即退出线程、清理登记、并**主动回收**
        // 已退出的子进程；同时把 pid 记入 `SinkTable.closed`——挡住此后迟到的
        // `register_frame_sink`（1.0-W7：检查与插入在同一把锁下，无交错窗口）。
        // 闭包捕获**本代次**的 `gen`：pid 被操作系统复用后，上一代读线程的
        // 迟到 EOF 不得再动新进程的表项（见 `SinkTable` 文档）。
        let sinks = self.sinks.clone();
        let writers = self.stdin_writers.clone();
        let children = self.children.clone();
        let sandbox_provider = self.sandbox_provider.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::with_capacity(256);
            loop {
                buf.clear();
                // `take` 给单帧设上限：sidecar 若写一条没有换行的超长行，
                // `read_until` 会在上限处返回而不是无限增长（1.0-W7③）。
                let read = {
                    let mut limited = (&mut reader).take((MAX_FRAME_BYTES + 1) as u64);
                    limited.read_until(b'\n', &mut buf)
                };
                match read {
                    // EOF：进程退出（或主动关了 stdout）。
                    Ok(0) => break,
                    // 超长帧（无换行）：协议违规，断开该进程的读线程。
                    Ok(_) if buf.len() > MAX_FRAME_BYTES => break,
                    Ok(_) => {
                        let trimmed = trim_ascii(&buf);
                        if trimmed.is_empty() {
                            continue; // 空行跳过（sidecar 的分帧留白）
                        }
                        // **先取后用**：`on_frame` 会一路走到 `Registry::settle_call`，
                        // 绝不能在持着 `sinks` 锁时回调（否则所有读线程串行化，
                        // 且被调方一旦反向触碰登记表就是同锁再入）。
                        let sink = sinks.lock().get(pid, gen);
                        if let Some(s) = sink {
                            s.on_frame(pid, trimmed);
                        }
                        // 无 sink：静默丢弃（调用方尚未注册；sidecar 不应自发帧）。
                    }
                    Err(_) => break, // 读错误 → 退出读线程
                }
            }
            // EOF：标记关闭并摘除 sink（同一把锁内原子完成）。
            // **临时 guard 在本语句结束即释放**——edition 2021 下
            // `if let … = sinks.lock().close(…)` 会把 guard 一直持到 if-let 块尾，
            // 块内若再触碰同一把锁就是自死锁（`parking_lot::Mutex` 不可重入）。
            let outcome = sinks.lock().close(pid, gen);
            if let Some(sink) = outcome.sink {
                sink.on_eof(pid);
            }
            // `is_current == false` = pid 已被复用，下面三张表里的条目已属于
            // **新进程**，这具"上一代遗骸"一个字都不能动（否则摘走新进程的
            // stdin / 句柄 → 静默断链）。
            if outcome.is_current {
                writers.lock().remove(&pid);
                // 主动回收：stdout 关闭**最常见**的原因是进程退出——此时 `try_wait`
                // 能立刻拿到状态并回收（不 wait 会在 Unix 留僵尸）。若进程只是关了
                // stdout 仍在跑（病态 sidecar），`try_wait` 返回 `None`，则放回表里
                // 由 `is_alive` / `kill` 继续管理——不误杀、不丢句柄（1.0-W7）。
                //
                // **同锁再入防护（1.0-R1 P0）**：下面必须先把 guard 绑到变量、
                // 在语句结束时释放，才能在 `if let` 体内再次 `children.lock()`。
                // 早先的写法是直接把 `children.lock().remove(&pid)` 放进 `if let`
                // 的判别式，edition 2021 的临时作用域会把那把 guard 持有到整个
                // if-let 结束，体内再锁同一把 `parking_lot::Mutex` 就是**永久
                // 自死锁**——病态 sidecar（关了 stdout 还在跑）一出现，整个
                // 进程管理面冻结。`eof_reap_must_not_hold_children_lock_inside_if_let`
                // 就是钉死这个形状的源码门禁。
                let reclaimed = children.lock().remove(&pid);
                if let Some(mut child) = reclaimed {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            // Parent is gone; descendants may still hold the same containment
                            // boundary. Best-effort tree teardown prevents crash orphans.
                            let _ = sandbox_provider.terminate_tree(pid);
                        }
                        Ok(None) | Err(_) => {
                            children.lock().insert(pid, child);
                        }
                    }
                }
            }
        });

        Ok(SpawnedProc { pid })
    }

    fn status(&self, pid: u32) -> ProcessStatus {
        let mut children = self.children.lock();
        match children.get_mut(&pid) {
            Some(child) => match child.try_wait() {
                Ok(Some(_status)) => {
                    children.remove(&pid);
                    ProcessStatus::Exited
                }
                Ok(None) => ProcessStatus::Alive,
                // A broken/invalid handle is not proof of life and not proof of death.
                Err(_) => ProcessStatus::Unknown,
            },
            // Not tracked by this spawner: ownership/liveness is unknown.
            None => ProcessStatus::Unknown,
        }
    }

    fn is_alive(&self, pid: u32) -> bool {
        // Preserve old conservative behavior for legacy callers while production V4 code uses
        // status(): Unknown must not increment crash counters but also must not be advertised
        // as proven alive.
        !matches!(self.status(pid), ProcessStatus::Exited)
    }

    /// 终止并**回收**（`wait`）子进程。
    ///
    /// 两条无需真进程即可验证的路径：未跟踪 pid → `AlreadyGone`；`kill` 失败但
    /// `try_wait` 确认已退出 → `AlreadyGone`。**正路（`Terminated`，即真的杀掉一个
    /// 活进程）未验证**：验证它必须真的起一个进程，而本仓测试明确不起真进程
    /// （见模块头注释）。这是诚实标注的未验证部分，不是遗漏。
    fn kill(&self, pid: u32) -> ProcResult<KillOutcome> {
        let Some(mut child) = self.children.lock().remove(&pid) else {
            return Ok(KillOutcome::AlreadyGone);
        };

        match self.sandbox_provider.terminate_tree(pid) {
            Ok(true) => {
                let _ = child.wait();
                return Ok(KillOutcome::Terminated);
            }
            Ok(false) => {}
            Err(error) => {
                // A failed containment teardown may have left the child running. Preserve the
                // tracked handle so a later recovery attempt can retry instead of orphaning it.
                self.children.lock().insert(pid, child);
                return Err(error);
            }
        }

        match child.kill() {
            Ok(()) => {
                let _ = child.wait();
                Ok(KillOutcome::Terminated)
            }
            Err(e) => match child.try_wait() {
                Ok(Some(_)) => Ok(KillOutcome::AlreadyGone),
                _ => {
                    self.children.lock().insert(pid, child);
                    Err(ProcError::ProcessTerminated(format!(
                        "终止 pid {pid} 失败：{e}（进程可能仍在运行，已保留跟踪句柄）"
                    )))
                }
            },
        }
    }

    /// 写入一帧到 sidecar stdin（JSON-RPC 行帧，自带 `\n` 结尾）。
    ///
    /// 该 pid 必须此前由本启动器 `spawn` 过且尚未退出（stdin 句柄仍在登记表）；
    /// 否则返回 `NotFound`（调用方据此知道"投递落空"，而不是静默丢失）。
    ///
    /// **两级锁（1.0-W7）**：外层锁只用于取该 pid 的 `Arc`，取到即释放；
    /// 阻塞的 `write_all` / `flush` 只持**内层** per-pid 锁。因此一个不读 stdin
    /// 的 sidecar 只会阻塞它自己的帧投递，不会拖住其他进程。
    fn write_frame(&self, pid: u32, frame: &[u8]) -> std::io::Result<()> {
        // 注意：临时 guard 在本语句结束即释放（未绑定到变量），阻塞写不持外层锁。
        let stdin = self.stdin_writers.lock().get(&pid).cloned();
        let Some(stdin) = stdin else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("pid {pid} 没有可用的 stdin 管道（进程可能尚未启动或已退出）"),
            ));
        };
        let mut guard = stdin.lock();
        guard.write_all(frame)?;
        guard.write_all(b"\n")?;
        guard.flush()
    }

    /// 注册一个 stdout 帧接收器（sidecar 回帧经此路由回宿主）。返回 `true` 表示已登记。
    ///
    /// **迟到登记拒绝**：读线程 EOF 时会在**同一把锁**下标记 `closed` 并摘除 sink；
    /// 此后再来的 `register_frame_sink` 返回 `false`（拒绝）——否则条目永远不会被
    /// 清理（读线程已退，没人回收它），反复"崩溃→重启"就无界累积。
    ///
    /// **无跟踪表条目也拒绝**：`children` 里没有这个 pid，说明进程已退出并被
    /// `is_alive` / `kill` 回收，或压根不是本启动器起的——两种情况都没有读线程
    /// 在排空 stdout，登记上去就是一条**永远收不到帧的死条目**。这条前置也让
    /// `SinkTable::retain_tracked` 的裁剪是安全的：被裁掉的 pid 必然过不了这道门。
    ///
    /// 1.0-W7：`closed` 检查与 `live` 插入由 [`SinkTable::register`] 原子完成，
    /// 消除了此前「两把锁之间可交错」的 TOCTOU 泄漏窗口。
    fn register_frame_sink(&self, pid: u32, sink: Arc<dyn ProcessFrameSink>) -> bool {
        // 先取值再判断：保证 `children` 的临时 guard 在本语句即释放，
        // 不与下面的 `sinks` 锁叠加成嵌套（锁序只允许 `children → sinks` 一处嵌套）。
        let tracked = self.children.lock().contains_key(&pid);
        if !tracked {
            return false;
        }
        self.sinks.lock().register(pid, sink)
    }
}

impl Drop for CommandSpawner {
    fn drop(&mut self) {
        // Final owner teardown must not abandon still-tracked sidecars. Reader threads only keep
        // the internal tables alive, not another CommandSpawner, so every remaining pid is driven
        // through the same tree-aware kill path.
        let pids: Vec<u32> = self.children.lock().keys().copied().collect();
        for pid in pids {
            let _ = ProcSpawner::kill(self, pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AbiFingerprint, BinarySignature};

    fn cfg_with_path(path: &str) -> SpawnConfig {
        SpawnConfig {
            binary_path: path.to_string(),
            args: vec!["--flag".to_string()],
            env: HashMap::new(),
            signature: BinarySignature {
                algorithm: "ed25519".to_string(),
                signature: "sig".to_string(),
                signer_id: "signer-001".to_string(),
            },
            binary_hash: "a".repeat(64),
            abi: AbiFingerprint::now("1.98.0", "iface-hash"),
        }
    }

    struct RejectingSandbox {
        configured: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ProcessSandboxProvider for RejectingSandbox {
        fn descriptor(&self) -> ProcessSandboxDescriptor {
            ProcessSandboxDescriptor {
                enforcement: ProcessSandboxEnforcement::Hard,
                process_tree_containment: true,
                filesystem_isolation: true,
                network_isolation: true,
                syscall_isolation: true,
                detail: "test hard sandbox".into(),
            }
        }

        fn configure(&self, _command: &mut Command, _cfg: &SpawnConfig) -> ProcResult<()> {
            self.configured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(ProcError::InvalidSpawnConfig("sandbox rejected before exec".into()))
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_default_provider_reports_partial_real_process_tree_containment() {
        let spawner = CommandSpawner::new();
        let descriptor = spawner.sandbox_descriptor();
        assert_eq!(descriptor.enforcement, ProcessSandboxEnforcement::Partial);
        assert!(descriptor.process_tree_containment);
        assert!(!descriptor.filesystem_isolation);
        assert!(!descriptor.network_isolation);
        assert!(!descriptor.syscall_isolation);
    }

    #[cfg(unix)]
    #[test]
    fn unix_process_group_is_real_and_tree_kill_removes_the_group() {
        let spawner = CommandSpawner::new();
        let mut cfg = cfg_with_path("/bin/sh");
        cfg.args = vec!["-c".into(), "sleep 30 & wait".into()];
        let spawned = spawner.spawn(&cfg).expect("spawn real process-group fixture");

        let pid = i32::try_from(spawned.pid).unwrap();
        // SAFETY: getpgid only inspects the live child pid.
        assert_eq!(unsafe { libc::getpgid(pid) }, pid, "child must lead its own process group");
        assert_eq!(spawner.kill(spawned.pid).unwrap(), KillOutcome::Terminated);

        let mut group_gone = false;
        for _ in 0..200 {
            // SAFETY: signal 0 performs existence/permission probing only.
            let rc = unsafe { libc::kill(-pid, 0) };
            if rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                group_gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(group_gone, "process group must not survive runtime teardown");
    }

    #[test]
    fn sandbox_provider_runs_before_os_spawn_and_fails_closed() {
        let configured = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spawner = CommandSpawner::with_sandbox_provider(Arc::new(RejectingSandbox {
            configured: configured.clone(),
        }));
        let cfg = cfg_with_path("definitely-not-a-real-binary");
        let err = spawner.spawn(&cfg).unwrap_err();
        assert!(matches!(err, ProcError::InvalidSpawnConfig(ref m) if m.contains("sandbox")));
        assert_eq!(configured.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(spawner.tracked(), 0, "sandbox rejection occurs before child creation");
        assert_eq!(spawner.sandbox_descriptor().enforcement, ProcessSandboxEnforcement::Hard);
    }

    #[test]
    fn built_in_command_spawner_reports_sandbox_unsupported_honestly() {
        let d = CommandSpawner::new().sandbox_descriptor();
        assert_eq!(d.enforcement, ProcessSandboxEnforcement::Unsupported);
        assert!(!d.process_tree_containment);
        assert!(d.detail.contains("direct child"));
    }

    /// 配置不合格时**必须**在启动之前被挡下：不产生进程、不留下句柄。
    #[test]
    fn command_spawner_rejects_invalid_config_before_spawning() {
        let spawner = CommandSpawner::new();
        let mut cfg = cfg_with_path("definitely-not-a-real-binary");
        cfg.binary_hash = "short".into(); // 非 64 位 hex
        let err = spawner.spawn(&cfg).unwrap_err();
        assert!(matches!(err, ProcError::InvalidSpawnConfig(_)));
        assert_eq!(spawner.tracked(), 0, "校验失败不得启动进程");
    }

    /// 不存在的二进制：`spawn` 必须返回 `Err` 且**不得**留下任何被跟踪的进程。
    ///
    /// 注意：这不是"真起 sidecar"——`Command::spawn` 在 exec 失败时立即返回错误，
    /// 没有任何进程被创建，因此 CI 上不会留下残留进程（本用例在 CI 里是确定的）。
    #[test]
    fn command_spawner_reports_spawn_failure_without_tracking() {
        let spawner = CommandSpawner::new();
        let cfg = cfg_with_path("definitely-not-a-real-binary-xyz-123");
        let err = spawner.spawn(&cfg).unwrap_err();
        assert!(
            matches!(err, ProcError::SpawnFailed(_)),
            "启动失败必须是 SpawnFailed（而不是假造一个 pid）：{err:?}"
        );
        assert_eq!(spawner.tracked(), 0, "启动失败不得留下句柄");
    }

    /// 未跟踪的 pid 一律视为存活：**不得凭空判死**（trait 契约）。
    #[test]
    fn unknown_pid_is_not_declared_dead() {
        let spawner = CommandSpawner::new();
        assert!(spawner.is_alive(0));
        assert!(spawner.is_alive(u32::MAX));
    }

    /// trait 的缺省 `is_alive` 同样不得判死（非 Tauri 宿主 / 测试 mock 的兜底语义）。
    ///
    /// 同时钉住 `kill` **没有**缺省实现：没有终止能力的实现必须自己显式返回 `Err`
    /// （这里是 `ProcessTerminated`），而不是拿到一个"静默不杀但返回 Ok"的兜底。
    #[test]
    fn default_is_alive_is_true_and_kill_must_be_declared() {
        struct NoProbe;
        impl ProcSpawner for NoProbe {
            fn spawn(&self, _cfg: &SpawnConfig) -> ProcResult<SpawnedProc> {
                Ok(SpawnedProc { pid: 42 })
            }
            fn kill(&self, pid: u32) -> ProcResult<KillOutcome> {
                Err(ProcError::ProcessTerminated(format!("该实现不提供终止能力（pid {pid}）")))
            }
        }
        assert!(NoProbe.is_alive(42));
        // 诚实表态：能力不足 = Err，**不是** Ok（否则就是静默漏杀）。
        assert!(NoProbe.kill(42).is_err());
    }

    /// 未跟踪的 pid：终止必须如实报 `AlreadyGone`（目标"无存活进程"已达成），
    /// 而不是谎报"杀掉了"或谎报"失败了"。
    ///
    /// 无需真进程：本启动器从没起过 pid 0 / u32::MAX。
    #[test]
    fn command_spawner_kill_of_untracked_pid_is_already_gone() {
        let spawner = CommandSpawner::new();
        assert_eq!(spawner.kill(0).unwrap(), KillOutcome::AlreadyGone);
        assert_eq!(spawner.kill(u32::MAX).unwrap(), KillOutcome::AlreadyGone);
        assert_eq!(spawner.tracked(), 0, "终止不得凭空造出被跟踪的进程");
    }

    // ── 1.0-W7：登记/关闭的原子性与写帧语义 ────────────────────────────

    /// 记录 `on_frame` / `on_eof` 调用的测试 sink。
    #[derive(Default)]
    struct RecordingSink {
        frames: Mutex<Vec<Vec<u8>>>,
        eof: Mutex<Vec<u32>>,
    }

    impl ProcessFrameSink for RecordingSink {
        fn on_frame(&self, _pid: u32, frame: &[u8]) {
            self.frames.lock().push(frame.to_vec());
        }
        fn on_eof(&self, pid: u32) {
            self.eof.lock().push(pid);
        }
    }

    /// 门禁（P0-7①）：`closed` 标记与 `live` 登记必须在**同一把锁**下变更，
    /// 因此「关闭后再登记」必须被拒绝——不允许出现"登记成功但永不被回收"的条目。
    #[test]
    fn sink_table_registration_after_close_is_rejected_atomically() {
        let mut table = SinkTable::default();
        let (gen, _) = table.begin(7);
        assert!(table.register(7, Arc::new(RecordingSink::default())));
        assert_eq!(table.live.len(), 1, "首次登记应入表");

        // 读线程 EOF：标记关闭 + 摘除 sink（同一把锁内完成）。
        let removed = table.close(7, gen);
        assert!(removed.sink.is_some(), "close 必须把 sink 交还给调用方以便触发 on_eof");
        assert!(removed.is_current, "同一代次的 EOF 必须判为当前代次");
        assert!(table.live.is_empty(), "关闭后 live 必须为空");
        assert!(table.closed.contains_key(&7));

        // 迟到登记：必须被拒，否则该条目永远不会被回收（无界累积）。
        assert!(!table.register(7, Arc::new(RecordingSink::default())));
        assert!(table.live.is_empty(), "迟到登记被拒后不得留下任何条目（这正是 P0-7① 的泄漏形态）");
    }

    /// 关闭时交还的 sink 必须能被调用 `on_eof`——该钩子此前在生产路径上零调用
    /// （P2-3），现在由读线程在 EOF 时触发。
    #[test]
    fn close_hands_back_sink_so_on_eof_can_fire() {
        let mut table = SinkTable::default();
        let (gen, _) = table.begin(11);
        let sink = Arc::new(RecordingSink::default());
        assert!(table.register(11, sink.clone()));
        let handed_back = table.close(11, gen).sink.expect("close 应返回被摘除的 sink");
        handed_back.on_eof(11);
        assert_eq!(sink.eof.lock().as_slice(), &[11]);
    }

    /// **1.0-R1 P0 回归**：pid 被操作系统复用后，上一代读线程的迟到 EOF
    /// 既不能把新进程的登记口焊死（否则新进程回帧**永远无人接收**——
    /// 「宿主 → sidecar → 回帧 → 结算」静默断链），也不能被判成"当前代次"
    /// 而去摘新进程的 stdin / child 表项。
    #[test]
    fn stale_eof_from_previous_generation_cannot_poison_reused_pid() {
        let mut table = SinkTable::default();
        let (gen_a, _) = table.begin(77); // 第一代
        assert!(table.register(77, Arc::new(RecordingSink::default())));

        // pid 复用：第二代开始（复用本身就要清掉第一代的关闭标记）。
        let (gen_b, stale) = table.begin(77);
        assert!(stale.is_some(), "上一代遗留的 sink 必须交还，否则它的所有者永远等不到 on_eof");
        assert_ne!(gen_a, gen_b, "同 pid 的两代必须不同，否则代次判别形同虚设");
        assert!(
            table.register(77, Arc::new(RecordingSink::default())),
            "复用后的新进程必须能登记——被旧代次的 closed 标记焊死就是静默断链"
        );

        // 第一代读线程的 EOF 迟到：必须判为遗骸。
        let outcome = table.close(77, gen_a);
        assert!(!outcome.is_current, "旧代次 EOF 不得判为当前代次");
        assert!(outcome.sink.is_none(), "旧代次不得摘走新代次的 sink");
        assert!(!table.closed.contains_key(&77), "旧代次不得写 closed，否则新进程的回帧口被焊死");
        assert!(
            table.register(77, Arc::new(RecordingSink::default())),
            "旧代次 EOF 之后新进程仍应能登记"
        );

        // 第二代自己的 EOF：正常关闭。
        let outcome = table.close(77, gen_b);
        assert!(outcome.is_current);
        assert!(outcome.sink.is_some());
        assert!(table.closed.contains_key(&77));
    }

    /// 无跟踪表条目（进程已回收 / 非本启动器起的）= 没有读线程在排水，
    /// 登记上去就是一条永远收不到帧的死条目 → 必须拒绝。
    #[test]
    fn register_frame_sink_rejects_pid_without_tracked_child() {
        let spawner = CommandSpawner::new();
        assert!(
            !spawner.register_frame_sink(4242, Arc::new(RecordingSink::default())),
            "未跟踪的 pid 必须拒绝登记（否则是永久残留的死条目）"
        );
        assert_eq!(spawner.sink_count(), 0, "拒绝后不得留下任何条目");
    }

    /// **锁形门禁（1.0-R1 P0）**：`children.lock()` 的临时 guard 在 edition 2021
    /// 下会一直活到 `if let` 块尾，块内再锁同一把 `parking_lot::Mutex` 就是
    /// **永久自死锁**（不可重入、无死锁检测）。真实触发条件是"病态 sidecar 关了
    /// stdout 还在跑"，本仓测试不起真进程，抓不到运行期——只能在源码形上钉死。
    #[test]
    fn eof_reap_must_not_hold_children_lock_inside_if_let() {
        let src = include_str!("spawner.rs");
        // needle 用片段拼出来，避免「门禁自身的字面量」被自己匹配到。
        let needle = ["if let Some(mut child) = ", "children", ".lock().remove(&pid)"].concat();
        assert!(
            !src.contains(&needle),
            "禁止在 if let 里直接持有 children.lock() 的临时 guard（edition 2021 \
             会把它持到块尾，体内同锁再入即永久自死锁）——必须先 let 绑定释放"
        );
        let fixed = ["let reclaimed = ", "children", ".lock().remove(&pid);"].concat();
        assert!(src.contains(&fixed), "回收路径必须以「先 let 绑定、语句结束即释放」的形状存在");
    }

    /// `closed` / `generation` 不得无界增长：每次 `spawn` 按跟踪表裁剪。
    #[test]
    fn sink_table_prunes_closed_and_generation_for_untracked_pids() {
        let mut table = SinkTable::default();
        for pid in 1..=8u32 {
            let (gen, _) = table.begin(pid);
            let _ = table.register(pid, Arc::new(RecordingSink::default()));
            let _ = table.close(pid, gen);
        }
        assert_eq!(table.closed.len(), 8, "裁剪前应按 pid 累积");
        table.retain_tracked(&std::collections::HashSet::from([2, 5]));
        assert_eq!(table.closed.len(), 2, "只保留仍在跟踪表里的 pid");
        assert_eq!(table.generation.len(), 2, "代次表同步裁剪");
        assert!(table.closed.contains_key(&2) && table.closed.contains_key(&5));
    }

    /// 写帧到未登记的 pid：必须如实报 `NotFound`（"投递落空"），不得静默成功。
    /// 同时验证两级锁下写路径不需要真进程即可覆盖失败分支。
    #[test]
    fn write_frame_to_unknown_pid_reports_not_found() {
        let spawner = CommandSpawner::new();
        let err = spawner.write_frame(4242, b"{\"id\":1}").expect_err("未登记的 pid 必须报错");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    /// 帧首尾的 ASCII 空白（含 `\r\n`）必须被裁掉：sidecar 用 CRLF 分帧时
    /// 不能把 `\r` 带进 JSON 解析。
    #[test]
    fn frame_trimming_strips_crlf_and_spaces() {
        assert_eq!(trim_ascii(b"  {\"a\":1}\r\n"), b"{\"a\":1}");
        assert_eq!(trim_ascii(b"\n"), b"");
        assert_eq!(trim_ascii(b"{}"), b"{}");
    }

    /// 单帧上限必须是**有界**的常量（防 sidecar 用无换行超长行把宿主 OOM）。
    #[test]
    // clippy 建议把下面的断言改成 `const { assert!(..) }`——那会把断言提前到编译期、
    // 令测试体变空，语义改变，故保留运行时断言并在此显式放行。
    #[allow(clippy::assertions_on_constants)]
    fn max_frame_bytes_is_bounded() {
        assert!(MAX_FRAME_BYTES > 0);
        assert!(MAX_FRAME_BYTES <= 16 * 1024 * 1024, "单帧上限过大等于没有上限");
    }
}
