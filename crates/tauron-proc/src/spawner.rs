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
use std::io::{BufRead, BufReader, Write};
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

/// 进程启动器（可注入）。
///
/// **契约**：`spawn` 返回 `Ok` 即表示操作系统层面**真的**有一个进程在跑，
/// 且它的 pid 是该返回值——上层据此铸租约（lease ↔ pid 一一绑定）。失败必须
/// 返回 `Err`，**不得**返回一个假 pid（那会让租约指向不存在的进程，而
/// `runtime_health` 会把它报成崩溃，故障点被彻底演没）。
pub trait ProcSpawner: Send + Sync {
    /// 启动 sidecar。
    fn spawn(&self, cfg: &SpawnConfig) -> ProcResult<SpawnedProc>;

    /// 进程是否仍存活。
    ///
    /// 缺省返回 `true`：**没有探测能力的实现不得凭空判死**。误判"已死"会触发
    /// 崩溃计数与 `RuntimeCrash` 事件，比"测不出来"严重得多（后者只是不报警）。
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
/// - 进程组/作业对象、`kill` 树（孙进程不随父进程一起死）、空闲超时 kill 均**未实现**：
///   终止只覆盖直接子进程。
#[derive(Default)]
pub struct CommandSpawner {
    /// pid → `Child`。持有句柄是 `try_wait` 的前提（否则只能靠平台 API 探测，
    /// 那会引入平台分支与 unsafe）。stdin/stdout 已在 `spawn` 时 `take` 出去，
    /// 因此这里不再持有（不影响 `try_wait`/`kill`）。
    children: Mutex<HashMap<u32, Child>>,
    /// pid → sidecar stdin 写句柄（宿主写 JSON-RPC 行帧用）。封 `Arc` 以便 `spawn`
    /// 时把同一张表克隆给读线程共享。
    stdin_writers: Arc<Mutex<HashMap<u32, ChildStdin>>>,
    /// pid → stdout 帧接收器（读线程把 sidecar 回帧路由给它）。封 `Arc` 共享。
    sinks: Arc<Mutex<HashMap<u32, Arc<dyn ProcessFrameSink>>>>,
    /// 已退出（读线程 EOF）的 pid 集合。`register_frame_sink` 用它挡住"进程已
    /// 退出后才来注册"的迟到登记——那种登记永远不会被读线程清理（线程已退），
    /// 不挡就是无界泄漏（0.4 审计修复）。
    closed: Arc<Mutex<std::collections::HashSet<u32>>>,
}

impl CommandSpawner {
    /// 创建启动器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前被本启动器跟踪的进程数（诊断/测试）。
    pub fn tracked(&self) -> usize {
        self.children.lock().len()
    }
}

impl ProcSpawner for CommandSpawner {
    fn spawn(&self, cfg: &SpawnConfig) -> ProcResult<SpawnedProc> {
        // spawn 前校验（§4.7 硬约束）：路径 / 签名 / sha256 / ABI 任一不合格
        // 都**不得**启动。校验在启动之前，因此不合格的配置连 syscall 都到不了。
        validate_spawn_config(cfg)?;

        let mut child = Command::new(&cfg.binary_path)
            .args(&cfg.args)
            .envs(&cfg.env)
            // stderr 承载日志（§4.7：日志走 stderr），继承宿主 stderr 即可，
            // 不需要管道（也就没有管道写满阻塞的风险）。
            .stderr(Stdio::inherit())
            // stdin/stdout 必须接管道：宿主经 stdin 写 JSON-RPC 请求帧，sidecar
            // 经 stdout 回帧。stdout 由下方读线程**持续排空**，否则 sidecar 写满
            // 管道缓冲区会被阻塞死（比丢弃更糟）。
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
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
        self.stdin_writers.lock().insert(pid, stdin);

        // 读线程：持续排空 sidecar 的 stdout，逐行交给该 pid 注册的 sink。
        // 读到 EOF（进程退出）即退出线程并清理该 pid 的 stdin/sink 登记，
        // 并把 pid 记入 `closed`——挡住此后迟到的 `register_frame_sink`。
        let sinks = self.sinks.clone();
        let writers = self.stdin_writers.clone();
        let closed = self.closed.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        let trimmed = l.trim();
                        if trimmed.is_empty() {
                            continue; // 空行跳过（sidecar 的分帧留白）
                        }
                        let sink = sinks.lock().get(&pid).cloned();
                        if let Some(s) = sink {
                            s.on_frame(pid, trimmed.as_bytes());
                        }
                        // 无 sink：静默丢弃（调用方尚未注册；sidecar 不应自发帧）。
                    }
                    Err(_) => break, // 读错误 / EOF → 退出读线程
                }
            }
            // EOF：清理该 pid 的 stdin 与 sink 登记，并标记已关闭。
            sinks.lock().remove(&pid);
            writers.lock().remove(&pid);
            closed.lock().insert(pid);
        });

        Ok(SpawnedProc { pid })
    }

    fn is_alive(&self, pid: u32) -> bool {
        let mut children = self.children.lock();
        match children.get_mut(&pid) {
            // `try_wait()` = `Some(..)` → 已退出；`None` → 仍在跑。
            Some(child) => match child.try_wait() {
                Ok(Some(_status)) => {
                    // 退出后句柄已无用：顺手移除，避免无界累积。
                    children.remove(&pid);
                    false
                }
                Ok(None) => true,
                // 探测自身失败（句柄失效等）：**不判死**（见 trait 文档）。
                Err(_) => true,
            },
            // 不在表里 = 不是本启动器起的进程：同样不判死。
            None => true,
        }
    }

    /// 终止并**回收**（`wait`）子进程。
    ///
    /// 两条无需真进程即可验证的路径：未跟踪 pid → `AlreadyGone`；`kill` 失败但
    /// `try_wait` 确认已退出 → `AlreadyGone`。**正路（`Terminated`，即真的杀掉一个
    /// 活进程）未验证**：验证它必须真的起一个进程，而本仓测试明确不起真进程
    /// （见模块头注释）。这是诚实标注的未验证部分，不是遗漏。
    fn kill(&self, pid: u32) -> ProcResult<KillOutcome> {
        let mut children = self.children.lock();
        let Some(mut child) = children.remove(&pid) else {
            // 不在跟踪表里：本启动器起过的进程要么已被 `kill` 回收、要么
            // `is_alive` 已观测到退出并把它移走。两种情况下都不存在"本启动器
            // 还在跑的进程"，因此目标已达成，如实报 `AlreadyGone`。
            return Ok(KillOutcome::AlreadyGone);
        };
        match child.kill() {
            Ok(()) => {
                // 必须 `wait`：Unix 上不 wait 会留下僵尸；Windows 上不 wait 会泄漏句柄。
                let _ = child.wait();
                Ok(KillOutcome::Terminated)
            }
            Err(e) => match child.try_wait() {
                // `kill` 对**已自行退出**的子进程会报错。若 `try_wait` 能确认它
                // 已经退出（并顺手回收），就按 `AlreadyGone` 如实上报，而不是
                // 谎报成终止失败。
                Ok(Some(_)) => Ok(KillOutcome::AlreadyGone),
                // 仍在跑（权限不足等）或状态不明：**不得**谎报成功。
                _ => Err(ProcError::ProcessTerminated(format!(
                    "终止 pid {pid} 失败：{e}（进程可能仍在运行）"
                ))),
            },
        }
    }

    /// 写入一帧到 sidecar stdin（JSON-RPC 行帧，自带 `\n` 结尾）。
    ///
    /// 该 pid 必须此前由本启动器 `spawn` 过且尚未退出（stdin 句柄仍在登记表）；
    /// 否则返回 `NotFound`（调用方据此知道"投递落空"，而不是静默丢失）。
    fn write_frame(&self, pid: u32, frame: &[u8]) -> std::io::Result<()> {
        let mut writers = self.stdin_writers.lock();
        match writers.get_mut(&pid) {
            Some(stdin) => {
                stdin.write_all(frame)?;
                stdin.write_all(b"\n")?;
                stdin.flush()
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("pid {pid} 没有可用的 stdin 管道（进程可能尚未启动或已退出）"),
            )),
        }
    }

    /// 注册一个 stdout 帧接收器（sidecar 回帧经此路由回宿主）。返回 `true` 表示已登记。
    ///
    /// **迟到登记拒绝**：读线程 EOF 时会把 pid 记入 `closed` 并清掉登记；此后再来的
    /// `register_frame_sink` 返回 `false`（拒绝）——否则条目永远不会被清理
    /// （读线程已退，没人回收它），反复"崩溃→重启"就无界累积。
    fn register_frame_sink(&self, pid: u32, sink: Arc<dyn ProcessFrameSink>) -> bool {
        if self.closed.lock().contains(&pid) {
            return false;
        }
        self.sinks.lock().insert(pid, sink);
        true
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
}
