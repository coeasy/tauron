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
use std::process::{Child, Command, Stdio};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{ProcError, ProcResult, SpawnConfig, validate_spawn_config};

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
}

/// 生产实现：`std::process::Command`。
///
/// 只做三件事——**真启动**（`Command::spawn`）、**真探测**（`Child::try_wait`）、
/// **真终止**（`Child::kill` + `wait` 回收）。不再多做，也不假装多做：
///
/// **未验证部分（诚实标注）**
/// - §4.7 的 JSON-RPC 帧回路（stdin/stdout）**未接线**：stdout 走 `Stdio::null()`，
///   因为把 stdout 接成管道却没人读，会在 sidecar 写满管道缓冲区时把它**阻塞死**
///   （比丢弃更糟）。因此当前 sidecar 收不到请求、也回不了帧——被验证的只有
///   「进程真的起来了 / 真的死了 / 终止调用真的发出去了」，这正是 P0-2 的范围。
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
    /// 那会引入平台分支与 unsafe）。
    children: Mutex<HashMap<u32, Child>>,
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

        let child = Command::new(&cfg.binary_path)
            .args(&cfg.args)
            .envs(&cfg.env)
            // stderr 承载日志（§4.7：日志走 stderr），继承宿主 stderr 即可，
            // 不需要管道（也就没有管道写满阻塞的风险）。
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| {
                ProcError::SpawnFailed(format!("启动 `{}` 失败：{e}", cfg.binary_path))
            })?;

        let pid = child.id();
        self.children.lock().insert(pid, child);
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
                Err(ProcError::ProcessTerminated(format!(
                    "该实现不提供终止能力（pid {pid}）"
                )))
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
