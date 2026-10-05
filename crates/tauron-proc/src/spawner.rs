// 进程执行器的**可注入启动面**（P0-2：让 `PluginType::Process` 有真实执行器入口）。
//
// 为什么必须是 trait：执行器语义（租约登记、崩溃计数、事件投递、租约回收时的
// 终止）要能在没有外部二进制的前提下被确定性验证——fake 启动面负责这一层，
// 生产实现用 `std::process::Command` 兑现同一份契约。
//
// 轮 22（V7-P1-05）补上的是另一半：仓内现在有**可执行的 sidecar 夹具**
// （`crates/tauron-test-sidecar`，`publish = false`，永不上架），
// `crates/tauron-test-sidecar/tests/sidecar_e2e.rs` 真起操作系统进程、真走 stdio
// 帧回路，把下面「未验证」清单里原本靠 fake 的条目逐条换成运行期证据。
// 也就是说：**trait 是为了分层验证而存在，不再是因为「测不了真进程」**。
//
// 本模块不依赖 `tauri`，也不认识任何宿主类型（`SpawnConfig` 是唯一输入，
// `SpawnedProc` 是唯一输出）。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
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

    /// Attach the just-created child to the provider-owned containment primitive.
    ///
    /// Called immediately after `Command::spawn` and before the child is inserted into Tauron's
    /// runtime tables. Failure is fail-closed: CommandSpawner kills/waits the direct child and
    /// reports spawn failure, so a half-contained runtime can never become visible as Running.
    fn attach_spawned(&self, _child: &Child) -> ProcResult<()> {
        Ok(())
    }

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

/// Windows built-in process-tree containment using a per-runtime Job Object.
///
/// The Job Object is configured with KILL_ON_JOB_CLOSE and the spawned process is attached
/// immediately after `Command::spawn`. Closing the sole Job handle therefore terminates the
/// process and all descendants that remain in the job. This is still `Partial`: the standard
/// library does not expose CREATE_SUSPENDED + primary-thread resume, so a tiny post-spawn attach
/// race remains, and Job Objects do not provide filesystem/network/syscall isolation.
#[cfg(windows)]
#[derive(Default)]
pub struct WindowsJobObjectSandboxProvider {
    /// pid -> sole owned Job Object HANDLE encoded as usize so the provider stays Send + Sync.
    jobs: Mutex<HashMap<u32, usize>>,
}

#[cfg(windows)]
impl ProcessSandboxProvider for WindowsJobObjectSandboxProvider {
    fn descriptor(&self) -> ProcessSandboxDescriptor {
        ProcessSandboxDescriptor {
            enforcement: ProcessSandboxEnforcement::Partial,
            process_tree_containment: true,
            filesystem_isolation: false,
            network_isolation: false,
            syscall_isolation: false,
            detail: "Windows Job Object KILL_ON_JOB_CLOSE process-tree containment; post-spawn attach race and filesystem/network/syscall isolation remain".into(),
        }
    }

    fn configure(&self, _command: &mut Command, _cfg: &SpawnConfig) -> ProcResult<()> {
        Ok(())
    }

    fn attach_spawned(&self, child: &Child) -> ProcResult<()> {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: null security/name requests an unnamed Job Object with default security.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() || job == INVALID_HANDLE_VALUE {
            return Err(ProcError::SpawnFailed(format!(
                "CreateJobObjectW failed for pid {}: {}",
                child.id(),
                std::io::Error::last_os_error()
            )));
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: info points to the exact structure required by JobObjectExtendedLimitInformation.
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: job is a live handle owned by this function.
            unsafe {
                CloseHandle(job);
            }
            return Err(ProcError::SpawnFailed(format!(
                "SetInformationJobObject failed for pid {}: {error}",
                child.id()
            )));
        }

        // SAFETY: Child owns a live process HANDLE until it is waited/dropped; Job assignment does
        // not transfer ownership of that process handle.
        let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) };
        if assigned == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: job is a live handle owned by this function.
            unsafe {
                CloseHandle(job);
            }
            return Err(ProcError::SpawnFailed(format!(
                "AssignProcessToJobObject failed for pid {}: {error}",
                child.id()
            )));
        }

        if let Some(previous) = self.jobs.lock().insert(child.id(), job as usize) {
            // Defensive PID-reuse cleanup. A live previous job must never outlive replacement.
            // SAFETY: previous was minted by CreateJobObjectW and removed from the ownership map.
            unsafe {
                CloseHandle(previous as _);
            }
        }
        Ok(())
    }

    fn terminate_tree(&self, pid: u32) -> ProcResult<bool> {
        use windows_sys::Win32::Foundation::CloseHandle;

        let Some(raw) = self.jobs.lock().remove(&pid) else {
            return Ok(false);
        };
        // KILL_ON_JOB_CLOSE makes closing our sole Job handle the tree-termination primitive.
        // SAFETY: raw was minted by CreateJobObjectW and removed exactly once from the map.
        let closed = unsafe { CloseHandle(raw as _) };
        if closed == 0 {
            return Err(ProcError::ProcessTerminated(format!(
                "CloseHandle(JobObject) failed for pid {pid}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(true)
    }
}

#[cfg(windows)]
impl Drop for WindowsJobObjectSandboxProvider {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        for (_, raw) in self.jobs.get_mut().drain() {
            // KILL_ON_JOB_CLOSE ensures provider teardown cannot orphan descendants.
            // SAFETY: each raw handle is uniquely owned by the drained map entry.
            unsafe {
                CloseHandle(raw as _);
            }
        }
    }
}

fn default_process_sandbox_provider() -> Arc<dyn ProcessSandboxProvider> {
    #[cfg(unix)]
    {
        Arc::new(UnixProcessGroupSandboxProvider)
    }
    #[cfg(windows)]
    {
        Arc::new(WindowsJobObjectSandboxProvider::default())
    }
    #[cfg(not(any(unix, windows)))]
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

    /// 读到一条**超过 `MAX_FRAME_BYTES` 且没有换行**的帧（协议违规）时调用一次，
    /// 紧接着才断开该进程的读线程；缺省空实现。
    ///
    /// 存在的理由：断链之前这条路径与「进程干净地退出（EOF）」在宿主侧**完全不可
    /// 区分**——两者都只是读线程结束并触发 `on_eof`，上层因此无法把「sidecar 吐了
    /// 一条超长帧」报告成它自己的类型化结局。钩子是**追加**的、默认空实现，
    /// 不改变任何既有行为：帧仍被丢弃、读线程仍断开、`on_eof` 仍照常触发。
    fn on_frame_oversized(&self, _pid: u32, _observed_bytes: usize) {}
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
/// - 写帧**不阻塞调用线程**（轮 2 改造）：`write_frame` 只做 `try_send`，管道写
///   交给该 pid 专属的写线程；队列满（`WouldBlock`）、帧超长（`InvalidInput`）、
///   无句柄（`NotFound`）、管道断（`BrokenPipe`）四种情形**全部如实失败**，
///   没有一条会假装成功。此前这里是「两级锁 + 同步 `write_all`」——锁只挡住了
///   「一个坏 sidecar 拖累别的 pid」，对不读 stdin 的 sidecar 本身，命令线程会
///   永久停在管道写上（P0-7② 只算修了一半）。
/// - 终止后的回收**有上界**（轮 2）：`kill` / `Drop` 不再用阻塞的 `Child::wait()`，
///   改用 [`reap_bounded`]（[`KILL_REAP_TIMEOUT_MS`]）；取不到退出状态就把句柄放回
///   跟踪表并报 `Err`，让上层留痕重试。
/// - 单帧**长度上限** `MAX_FRAME_BYTES`：用 `Read::take` 限制，超长行（无换行）
///   不会无限增长把宿主 OOM（P0-7③）。
/// - EOF 时**主动 `try_wait` 回收**已退出的子进程（不 wait 会留僵尸），
///   仅在「进程仍存活但关了 stdout」这种病态情形下保留句柄（P2-4）。
/// - EOF 时调用 sink 的 `on_eof`（此前该钩子在生产路径上零调用，P2-3）。
///
/// **验证面（轮 22 起有运行期证据）**
/// - **真实 sidecar 端到端**：`crates/tauron-test-sidecar/tests/sidecar_e2e.rs`
///   用 `env!("CARGO_BIN_EXE_tauron_test_sidecar")` 拿到真二进制、真起进程，
///   经**生产**投递/结算面（`tauron-adapter` 的 `ProcessFrameSinkImpl` →
///   `Registry::settle_call`）证明「sidecar 真收到帧、真回帧、宿主真结算」。
///   同一条链路还覆盖了真崩溃计数、超长帧的类型化结局、旧代际回帧被守卫丢弃、
///   EOF 回收不留孤儿、pending 上限、乱序回帧与静默 sidecar。
/// - 仍然**不保证**跨平台「已退出」判定时机一致（`try_wait` 在子进程退出后返回
///   `Some(status)`）；僵尸进程的回收依赖本类型仍持有 `Child`。
/// - **本类型被丢弃时会回收**：`Drop` 把仍在跟踪的每个 pid 都驱动到**同一套**
///   tree-aware `kill`（`terminate_tree` + `wait`）。读线程只持有内部表的 `Arc`、
///   不持有第二个 `CommandSpawner`，因此 Drop 只由最终持有者触发一次。
///   两条边界如实标注：① 端到端用例的收尾**显式断言 `tracked()` 归零**，并在断言
///   失败时依赖 Drop 兜底 tree-kill，但「Drop 自己逐个 kill 遗留 pid」这条路径
///   仍只有代码与结构门禁 + E2E 的兜底纪律，没有专门的注入式失败测试；② 终止失败
///   （`terminate_tree` 报错）时 `kill` 会保留跟踪句柄，而正在退出的宿主没有重试方，
///   该子进程因此可能存活——这是 OS 语义，不假装已经解决。
/// - `kill` 的**正路**（真的杀掉一个活进程 → `Terminated`）由
///   `real_sidecar_round_trip_settles_and_disable_kills_the_process` 与
///   `silent_sidecar_receives_but_never_answers_and_is_killed_cleanly` 真进程验证；
///   无需进程的路径（未知 pid / 已退出）另有单测。
/// - Unix/macOS 默认 provider 用独立 POSIX process group，Windows 默认 provider 用
///   KILL_ON_JOB_CLOSE Job Object；三桌面平台都已有真实 process-tree containment。
///   但 filesystem/network/syscall 隔离仍未内建，Windows 还有 post-spawn attach
///   race，因此默认 capability 仍最多是 partial、Production 仍 fail-closed。
pub struct CommandSpawner {
    /// pid → `Child`。持有句柄是 `try_wait` 的前提（否则只能靠平台 API 探测，
    /// 那会引入平台分支与 unsafe）。stdin/stdout 已在 `spawn` 时 `take` 出去，
    /// 因此这里不再持有（不影响 `try_wait`/`kill`）。封 `Arc` 以便读线程在
    /// stdout EOF 时**主动回收**已退出的子进程（1.0-W7）。
    children: Arc<Mutex<HashMap<u32, Child>>>,
    /// pid → sidecar stdin 写队列的发送端（宿主写 JSON-RPC 行帧用）。
    ///
    /// **轮 2 改造：写侧从「调用线程同步写管道」改为「有界队列 + 专属写线程」**。
    /// 旧写法的阻塞点在 OS 管道上：sidecar 不读 stdin 时管道缓冲写满，
    /// `write_all` 就**永久停在调用线程上**——生产路径的调用线程就是 Tauri 命令
    /// 线程（`tauron-adapter/src/process_delivery.rs` 的 `CallDelivery::deliver`
    /// 内联调用）。此前的两级锁只解决「一个坏 sidecar 拖累别的 pid」，
    /// 对这个 pid 自己的投递线程既无超时也无上限。
    ///
    /// 现在的形态：`write_frame` 只做 `try_send`（不阻塞、可失败），真正面向管道
    /// 的阻塞写发生在该 pid 专属的写线程里。条目被摘除（EOF 回收 / 未登记）时
    /// 发送端 drop → 写线程收尾退出，同时 sidecar 的 stdin 拿到 EOF。
    stdin_writers: Arc<Mutex<HashMap<u32, Arc<mpsc::SyncSender<Vec<u8>>>>>>,
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

/// 写侧队列深度（轮 2）。`write_frame` 只做 `try_send`：队列满即**如实报错**，
/// 绝不阻塞调用线程，也绝不丢帧（返回 `Err` 的帧调用方知道没投递出去）。
///
/// 与 [`MAX_FRAME_BYTES`]（入队时校验）合起来给出每个 pid 的写侧内存上界
/// = `WRITE_QUEUE_FRAMES × MAX_FRAME_BYTES` = 32 MiB（最坏情况）。
pub const WRITE_QUEUE_FRAMES: usize = 32;

/// 某 pid 的 stdin 写线程：独占管道写句柄，按队列顺序逐帧写。
///
/// 为什么单独一个线程：管道写满只能靠**对端读**来解，宿主这边没有任何
/// 非阻塞又能等的写法。把阻塞圈在这个线程里，代价是「这一路帧落后」，
/// 换来的是命令线程永不被一个不读 stdin 的 sidecar 钉死。
///
/// 退出条件只有两条，且都**有界**：
/// - 写管道出错（进程已退/stdin 已断）→ 立即收尾，后续入队会拿到 `Disconnected`；
/// - 发送端全部 drop（EOF 回收摘条目 / spawner 被丢弃）→ 队列自然耗尽后退出，
///   此时 `ChildStdin` 随本结构 drop，sidecar 读到 stdin EOF（体面的收尾信号）。
struct StdinWriter {
    stdin: ChildStdin,
    rx: mpsc::Receiver<Vec<u8>>,
}

impl StdinWriter {
    fn run(self) {
        let StdinWriter { mut stdin, rx } = self;
        for frame in rx.iter() {
            if write_frame_to(&mut stdin, &frame).is_err() {
                return; // 管道断了：留在表里的写句柄已无意义，如实结束线程
            }
        }
    }
}

/// 一帧 = 内容 + 换行 + flush（sidecar 按行分帧，见 `tauron-test-sidecar`）。
fn write_frame_to(stdin: &mut ChildStdin, frame: &[u8]) -> std::io::Result<()> {
    stdin.write_all(frame)?;
    stdin.write_all(b"\n")?;
    stdin.flush()
}

/// 已发出终止后等退出状态的上界（轮 2）。
///
/// 取值依据：SIGKILL / Job Object 的生效在毫秒级，2s 已是三个数量级的余量；
/// 再大就是把「收尾可能卡住」换成「收尾一定很慢」。
const KILL_REAP_TIMEOUT_MS: u64 = 2_000;

/// **有界**地取子进程退出状态，替代阻塞的 `Child::wait()`。
///
/// `Some(状态)` = 已确认退出并回收（Unix 不留僵尸）；`None` = 上界内取不到，
/// 或 `try_wait` 本身报错——两者都**不是**「进程还活着」的证明，调用方必须
/// 保留句柄并如实报告，而不是当成已回收。
fn reap_bounded(child: &mut Child) -> Option<std::process::ExitStatus> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(KILL_REAP_TIMEOUT_MS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

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

        // V4 A97: post-spawn containment attachment must complete before this process is visible
        // in any Tauron runtime table. A failed Job/cgroup/container attach is fail-closed.
        if let Err(error) = self.sandbox_provider.attach_spawned(&child) {
            let _ = child.kill();
            // 有界回收：这里的 `Child` 从未进表，没人能重试，因此**不允许**
            // 阻塞在 `wait()` 上（轮 2：收尾路径一律有上界）。
            let _ = reap_bounded(&mut child);
            return Err(error);
        }

        let pid = child.id();
        // 取出 stdin 写句柄（宿主写帧用）与 stdout（交给读线程排空）。
        // **任一失败都要回收子进程**：直接 `Err` 返回会把 `Child` 一丢了之——
        // 句柄 drop 不 kill/wait，进程留成无人跟踪的孤儿（0.4 审计修复）。
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            (stdin, stdout) => {
                let _ = child.kill();
                let _ = reap_bounded(&mut child);
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

        // 写侧：有界队列 + 该 pid 专属写线程（见 `StdinWriter`）。发送端进表，
        // 接收端与管道句柄交给线程。先把发送端插入表里再起线程——否则一个立刻
        // 退出并触发 EOF 回收的 sidecar 可能抢在插入之前把条目摘掉，留下一条
        // 永远没人摘的僵尸管道句柄。
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE_FRAMES);
        self.stdin_writers.lock().insert(pid, Arc::new(tx));
        std::thread::spawn(move || StdinWriter { stdin, rx }.run());

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
                    // 超长帧（无换行）：协议违规。先报告类型化结局，再断开该进程的读线程。
                    Ok(_) if buf.len() > MAX_FRAME_BYTES => {
                        // **先取后用**：与下面 `on_frame` 同一套锁纪律——不得在持着
                        // `sinks` 临时 guard 时回调 sink。
                        let sink = sinks.lock().get(pid, gen);
                        if let Some(s) = sink {
                            s.on_frame_oversized(pid, buf.len());
                        }
                        break;
                    }
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
        let status = {
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
        };
        if status == ProcessStatus::Exited {
            // Parent exit is also a containment lifecycle boundary. Descendants must not survive
            // merely because status() observed the direct child before the stdout EOF thread did.
            let _ = self.sandbox_provider.terminate_tree(pid);
        }
        status
    }

    fn is_alive(&self, pid: u32) -> bool {
        // Preserve old conservative behavior for legacy callers while production V4 code uses
        // status(): Unknown must not increment crash counters but also must not be advertised
        // as proven alive.
        !matches!(self.status(pid), ProcessStatus::Exited)
    }

    /// 终止并**有界地**回收子进程（[`reap_bounded`]，上界
    /// [`KILL_REAP_TIMEOUT_MS`]；轮 2 之前用的是阻塞的 `Child::wait()`）。
    ///
    /// 三条路径都有证据：未跟踪 pid → `AlreadyGone`（单测）；`kill` 失败但
    /// `try_wait` 确认已退出 → `AlreadyGone`（单测）；**正路 `Terminated`**（真的
    /// 杀掉一个活进程）由 `crates/tauron-test-sidecar/tests/sidecar_e2e.rs` 的真
    /// sidecar 用例验证（轮 22 / V7-P1-05），不再只是代码推导。
    fn kill(&self, pid: u32) -> ProcResult<KillOutcome> {
        let Some(mut child) = self.children.lock().remove(&pid) else {
            return Ok(KillOutcome::AlreadyGone);
        };

        match self.sandbox_provider.terminate_tree(pid) {
            Ok(true) => {
                // **有界**回收（轮 2）：`Child::wait()` 在进程不可杀（Unix D 状态、
                // Windows 作业已脱离）时永不返回，而这里既跑在调用线程上，也会被
                // `Drop` 逐个 pid 调用——一处卡住就把整个运行时收尾冻结。取不到退出
                // 状态时**放回句柄并如实报错**：上层据此留痕并可重试，不假装已回收。
                return match reap_bounded(&mut child) {
                    Some(_) => Ok(KillOutcome::Terminated),
                    None => {
                        self.children.lock().insert(pid, child);
                        Err(ProcError::ProcessTerminated(format!(
                            "已向 pid {pid} 的进程树发出终止，但 {KILL_REAP_TIMEOUT_MS}ms 内未取到退出状态（句柄已保留，可重试）"
                        )))
                    }
                };
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
            Ok(()) => match reap_bounded(&mut child) {
                Some(_) => Ok(KillOutcome::Terminated),
                None => {
                    self.children.lock().insert(pid, child);
                    Err(ProcError::ProcessTerminated(format!(
                        "已终止 pid {pid}，但 {KILL_REAP_TIMEOUT_MS}ms 内未取到退出状态（句柄已保留，可重试）"
                    )))
                }
            },
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

    /// 写入一帧到 sidecar stdin（JSON-RPC 行帧，`\n` 由写线程补）。
    ///
    /// 该 pid 必须此前由本启动器 `spawn` 过且尚未被摘除（写队列发送端仍在表里）；
    /// 否则返回 `NotFound`（调用方据此知道"投递落空"，而不是静默丢失）。
    ///
    /// **本函数不阻塞**（轮 2）：真正的管道写在那个 pid 专属的写线程上做，这里只
    /// `try_send`。三种结局都是**如实失败**，没有一条会假装成功：
    /// - `NotFound`：表里没有这个 pid（未起 / 已被 EOF 回收 / 已 kill 摘除）；
    /// - `InvalidInput`：帧长超过 [`MAX_FRAME_BYTES`]——与读侧同一道上界，
    ///   否则 32 帧队列就成了无界的宿主内存口子；
    /// - `WouldBlock`：队列已满（对端不读 stdin）。调用方（`ProcessCallDelivery`）
    ///   把它当作投递失败上抛，宿主侧因此看得见背压而不是无限堆帧；
    /// - `BrokenPipe`：写线程已因管道断裂退出。
    fn write_frame(&self, pid: u32, frame: &[u8]) -> std::io::Result<()> {
        // 临时 guard 在本语句结束即释放：外层锁只用于取发送端，入队与写管道都在锁外。
        let tx = self.stdin_writers.lock().get(&pid).cloned();
        let Some(tx) = tx else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("pid {pid} 没有可用的 stdin 管道（进程可能尚未启动或已退出）"),
            ));
        };
        if frame.len() > MAX_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("单帧 {} 字节超过上限 {MAX_FRAME_BYTES}，未投递", frame.len()),
            ));
        }
        match tx.try_send(frame.to_vec()) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!(
                    "pid {pid} 的写队列已满（{WRITE_QUEUE_FRAMES} 帧未消化，sidecar 未读 stdin）"
                ),
            )),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("pid {pid} 的 stdin 管道已断（写线程因写入失败退出）"),
            )),
        }
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

    #[cfg(windows)]
    #[test]
    fn windows_default_provider_reports_partial_job_object_containment() {
        let spawner = CommandSpawner::new();
        let descriptor = spawner.sandbox_descriptor();
        assert_eq!(descriptor.enforcement, ProcessSandboxEnforcement::Partial);
        assert!(descriptor.process_tree_containment);
        assert!(!descriptor.filesystem_isolation);
        assert!(!descriptor.network_isolation);
        assert!(!descriptor.syscall_isolation);
        assert!(descriptor.detail.contains("Job Object"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_job_object_attaches_and_tree_kill_is_exercised_on_real_process() {
        let spawner = CommandSpawner::new();
        let mut cfg = cfg_with_path("cmd.exe");
        // cmd launches ping.exe as a child, so the Job Object contains a real descendant tree.
        cfg.args = vec!["/C".into(), "ping -n 30 127.0.0.1 >NUL".into()];
        let spawned = spawner.spawn(&cfg).expect("spawn real Windows Job Object fixture");
        assert_eq!(spawner.status(spawned.pid), ProcessStatus::Alive);
        assert_eq!(spawner.kill(spawned.pid).unwrap(), KillOutcome::Terminated);
        assert_eq!(spawner.status(spawned.pid), ProcessStatus::Unknown);
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
    fn built_in_command_spawner_reports_platform_sandbox_honestly() {
        let d = CommandSpawner::new().sandbox_descriptor();
        #[cfg(unix)]
        {
            assert_eq!(d.enforcement, ProcessSandboxEnforcement::Partial);
            assert!(d.process_tree_containment);
            assert!(d.detail.contains("process-group"));
        }
        #[cfg(windows)]
        {
            assert_eq!(d.enforcement, ProcessSandboxEnforcement::Partial);
            assert!(d.process_tree_containment);
            assert!(d.detail.contains("Job Object"));
        }
        #[cfg(not(any(unix, windows)))]
        {
            assert_eq!(d.enforcement, ProcessSandboxEnforcement::Unsupported);
            assert!(!d.process_tree_containment);
            assert!(d.detail.contains("direct child"));
        }
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
    /// stdout 还在跑"：轮 22 起该路径有真进程 E2E
    /// （`crates/tauron-test-sidecar/tests/sidecar_e2e.rs` 的
    /// `stdout_eof_mid_conversation_reaps_without_hang_or_orphan`），但这条源码形
    /// 门禁仍然保留——自死锁命中时 E2E 表现为挂到硬超时而不是稳定断言，形锁才是
    /// 确定、秒级的证据。
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
    /// 同时验证写路径（有界队列 + 专属写线程）不需要真进程即可覆盖失败分支。
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
