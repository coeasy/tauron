// CI 专用的 **真 sidecar 夹具**（V7-P1-05）。
//
// **它证明什么**：`crates/tauron-test-sidecar/tests/sidecar_e2e.rs` 用
// `tauron_proc::CommandSpawner` 把本 crate 编出来的可执行文件**真的**起成一个操作
// 系统进程，然后走 `tauron-proc` 生产实现的那条 stdio 通路——宿主经 **stdin** 写
// JSON-RPC 行帧（`tauron-adapter::process_delivery::ProcessCallDelivery` 造的帧），
// sidecar 经 **stdout** 回帧（读线程逐行排水、`MAX_FRAME_BYTES` 单帧上限、
// pid+代次 的 `SinkTable` 登记），回帧最终落到
// `tauron-adapter::process_delivery::ProcessFrameSinkImpl` → `Registry::settle_call`。
// 于是「sidecar 真收到请求 / 真回包 / 异常退出 / 重启后旧回复被拒」第一次有了
// **运行期证据**，而不是 fake spawner 的契约引用。
//
// **它不证明什么（诚实边界）**：
// - 它是 **CI 夹具，不是产品 sidecar**：行为全部由 argv 指挥，只为把每条故障路径
//   变成可确定的输入，没有任何业务语义，也不该被产品代码引用；
// - **没有对 sidecar 二进制的验签**：`SpawnConfig` 的 `signature` / `binary_hash`
//   仍是调用方**自报**的字符串，`validate_spawn_config` 只查非空与 hex 长度，
//   本夹具既不签自己也没人验它；
// - `ProcSpawner` 的 **ABI 值仍是自报**而非从签名清单推导（V7 Batch 2 那条未结项）：
//   `cmd_runtime_spawn` 用 `validate_abi` 比的是「宿主契约常量」与「profile 里调用方
//   声明的指纹」，`tauron-proc` 与 sidecar 之间**没有任何运行期握手**——进程起来后
//   第一帧就是业务请求帧，所以本夹具也**不发**握手帧（协议里没有这个东西，
//   不去发明一个）。
//
// 帧协议（照 `tauron-proc` + `tauron-adapter` 现状实现，无额外约定）：
// - 请求（宿主 → stdin，一行一帧，`\n` 结尾）：
//   `{"jsonrpc":"2.0","id":<seq>,"method":<cmd>,"params":<args>,"callId":"<uuid>",
//   "caller":"main","target":"<plugin id>","runtimeGeneration":<u64>}`
// - 回帧（stdout → 宿主，一行一帧）：带同一个 `callId`，成功给 `result`，
//   失败给 `error.code` / `error.message`（`ProcessFrameSinkImpl` 据此结算）。
//   `runtimeGeneration` 会被回显，但**宿主不读它**——代次判定用的是注册表里的活租约
//   （见 `tests` 里 `stale_generation_*` 用例的注释）。

use std::io::Write;

/// 行为模式：每个值对应一条需要被端到端证明的故障/时序路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// 逐请求回一帧（默认）。
    Echo,
    /// 收到请求但**永不回帧**（超时路径）。注意它照收不误，因此**不是**写侧背压
    /// 的驱动源——那条要 [`Behavior::Deaf`]。
    Silent,
    /// 以 `--exit-code` 非零退出（崩溃路径）。`--exit-after-requests 0` = 一起来就崩。
    Crash,
    /// 回满 `--close-after-requests` 帧后**关掉自己的 stdout**但继续活着
    /// （病态 sidecar：EOF 而进程未退出——`CommandSpawner` 的 EOF 回收分支此前
    /// 只有源码形状门禁，没有运行期证据）。
    CloseStdout,
    /// 吐一条超过 `tauron_proc::spawner::MAX_FRAME_BYTES` 的帧（协议违规路径）。
    Oversize,
    /// **从不**读 stdin，只是活着（轮 2：宿主写侧背压的唯一真实驱动源）。
    ///
    /// 与 `Silent` 的区别就在读侧：`Silent` 照收不误只是不回帧，管道因此永远
    /// 排得空，宿主写侧看不到背压；`Deaf` 让 OS 管道缓冲写满，宿主的写线程才会
    /// 停在 `write_all` 上——队列填满后 `write_frame` 必须**如实报 WouldBlock**，
    /// 而不是把投递线程一起钉死。
    Deaf,
}

impl Behavior {
    const ALL: [(&'static str, Self); 6] = [
        ("echo", Self::Echo),
        ("silent", Self::Silent),
        ("crash", Self::Crash),
        ("close-stdout", Self::CloseStdout),
        ("oversize", Self::Oversize),
        ("deaf", Self::Deaf),
    ];

    fn parse(raw: &str) -> Result<Self, String> {
        Self::ALL.iter().find(|(name, _)| *name == raw).map(|(_, behavior)| *behavior).ok_or_else(
            || {
                format!(
                    "`--behavior` 取值非法：`{raw}`（可选：{}）",
                    Self::ALL.map(|(name, _)| name).join(" | ")
                )
            },
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Silent => "silent",
            Self::Crash => "crash",
            Self::CloseStdout => "close-stdout",
            Self::Oversize => "oversize",
            Self::Deaf => "deaf",
        }
    }
}

/// 夹具的行为面：全部经 argv 传入（Windows 上没有 shell 语义可依赖，
/// 环境变量在 `SpawnConfig.env` 里反而更绕，因此只认 argv）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub behavior: Behavior,
    /// 行为默认值，见 [`Behavior`] 各变体。
    pub exit_code: i32,
    /// 处理满 N 个请求后按 `exit_code` 退出；`0` = 读第一帧之前就退出。
    pub exit_after_requests: u64,
    /// 回复前延迟（模拟慢 sidecar；是**被测进程**的延迟，不是测试的等待手段）。
    pub delay_ms: u64,
    /// 回满 N 帧后关闭 stdout（仅 `CloseStdout`）。
    pub close_after_requests: u64,
    /// 关 stdout 之后再存活 N ms 并按 `exit_code` 退出；`0` = 一直活着。
    /// 只在 `CloseStdout` 下生效。
    pub exit_after_ms: u64,
    /// 收到请求先**不**回，攒着；等一条 `__tauron_flush__` 控制帧再统一吐。
    /// 这是「旧进程在宿主已经换代之后才回包」这条时序唯一可确定的驱动方式。
    pub reply_on_trigger: bool,
    /// 攒满 N 条回帧后**逆序**吐出（乱序回帧路径）。
    pub out_of_order: u64,
    /// 每个请求吐几条回帧（重复结算 / 快速连帧路径；`1` = 正常）。
    pub fanout: u64,
    /// 回帧改成 `error` 载荷并带上该错误码（`None` = 回 `result`）。
    pub reply_error_code: Option<i64>,
    /// 回帧里的 `runtimeGeneration` 用这个值，而不是回显请求里的值。
    pub force_generation: Option<u64>,
    /// `Oversize` 模式那一帧的**总字节数**（含结尾 `\n`）。
    pub frame_bytes: usize,
    /// JSONL 证据文件：夹具把自己的 pid、收到的每一帧、退出意图写进去。
    /// 测试据此断言「这个 pid 真的跑过、真的收到了这一帧」。
    pub trace: Option<std::path::PathBuf>,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            behavior: Behavior::Echo,
            exit_code: 7,
            exit_after_requests: 0,
            delay_ms: 0,
            close_after_requests: 1,
            exit_after_ms: 0,
            reply_on_trigger: false,
            out_of_order: 0,
            fanout: 1,
            reply_error_code: None,
            force_generation: None,
            frame_bytes: 2 * 1024 * 1024,
            trace: None,
        }
    }
}

/// `__tauron_flush__` = 让 `reply_on_trigger` 模式把攒下的回帧一次性吐出去。
/// 它不是 JSON-RPC 业务方法，只有夹具测试会发。
pub const FLUSH_TRIGGER: &str = "__tauron_flush__";

/// `Spec` 的命令行面：`to_args` 与 `from_args` 严格互逆（测试用前者，
/// 夹具用后者，任何一边的拼写漂移都会让对端直接失败而不是静默降级）。
impl Spec {
    pub fn to_args(&self) -> Vec<String> {
        let mut out = vec![
            "--behavior".to_string(),
            self.behavior.as_str().to_string(),
            "--exit-code".to_string(),
            self.exit_code.to_string(),
            "--exit-after-requests".to_string(),
            self.exit_after_requests.to_string(),
            "--delay-ms".to_string(),
            self.delay_ms.to_string(),
            "--close-after-requests".to_string(),
            self.close_after_requests.to_string(),
            "--exit-after-ms".to_string(),
            self.exit_after_ms.to_string(),
            "--out-of-order".to_string(),
            self.out_of_order.to_string(),
            "--fanout".to_string(),
            self.fanout.to_string(),
            "--frame-bytes".to_string(),
            self.frame_bytes.to_string(),
        ];
        if self.reply_on_trigger {
            out.push("--reply-on-trigger".to_string());
        }
        if let Some(code) = self.reply_error_code {
            out.push("--reply-error".to_string());
            out.push(code.to_string());
        }
        if let Some(gen) = self.force_generation {
            out.push("--force-generation".to_string());
            out.push(gen.to_string());
        }
        if let Some(trace) = &self.trace {
            out.push("--trace".to_string());
            out.push(trace.to_string_lossy().into_owned());
        }
        out
    }

    pub fn from_args<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut spec = Self::default();
        // 一个旗标要么不带值（布尔开关），要么恰好带一个值；没有位置参数。
        let argv: Vec<String> =
            args.into_iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect();
        let mut index = 0usize;
        while index < argv.len() {
            let flag = argv[index].clone();
            index += 1;
            let raw = |index: &mut usize| value_at(&argv, index, &flag);
            match flag.as_str() {
                "--behavior" => spec.behavior = Behavior::parse(&raw(&mut index)?)?,
                "--exit-code" => spec.exit_code = parse_i64(&raw(&mut index)?)? as i32,
                "--exit-after-requests" => spec.exit_after_requests = parse_u64(&raw(&mut index)?)?,
                "--delay-ms" => spec.delay_ms = parse_u64(&raw(&mut index)?)?,
                "--close-after-requests" => {
                    spec.close_after_requests = parse_u64(&raw(&mut index)?)?
                }
                "--exit-after-ms" => spec.exit_after_ms = parse_u64(&raw(&mut index)?)?,
                "--out-of-order" => spec.out_of_order = parse_u64(&raw(&mut index)?)?,
                "--fanout" => {
                    spec.fanout = parse_u64(&raw(&mut index)?)?;
                    if spec.fanout == 0 {
                        return Err("`--fanout` 至少为 1".to_string());
                    }
                }
                "--reply-on-trigger" => spec.reply_on_trigger = true,
                "--reply-error" => spec.reply_error_code = Some(parse_i64(&raw(&mut index)?)?),
                "--force-generation" => spec.force_generation = Some(parse_u64(&raw(&mut index)?)?),
                "--frame-bytes" => spec.frame_bytes = parse_u64(&raw(&mut index)?)? as usize,
                "--trace" => spec.trace = Some(std::path::PathBuf::from(raw(&mut index)?)),
                other => return Err(format!("未知参数：`{other}`")),
            }
        }
        Ok(spec)
    }
}

/// 取下一个参数值；缺值即报错，**不**回落到默认值（静默回落会让拼错的旗标
/// 变成一个看起来正常的行为，那比失败更难查）。
fn value_at(argv: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    if *index >= argv.len() {
        return Err(format!("`{name}` 缺少数值/路径参数"));
    }
    let value = argv[*index].clone();
    *index += 1;
    Ok(value)
}

fn parse_u64(raw: &str) -> Result<u64, String> {
    raw.parse::<u64>().map_err(|e| format!("`{raw}` 不是合法的无符号整数：{e}"))
}

fn parse_i64(raw: &str) -> Result<i64, String> {
    raw.parse::<i64>().map_err(|e| format!("`{raw}` 不是合法的整数：{e}"))
}

/// JSONL 证据文件（`--trace`）。
///
/// 为什么夹具要自己写文件而不只靠 stdout：测试需要区分「进程真的跑过并真收到了
/// 这一帧」与「宿主以为自己投递了」。stdout 里的字节只能证明**回帧内容**，
/// 证明不了「请求真的到达了那个 pid」——那要由 sidecar 自己落笔。
/// 写的是**普通文件**，与 stdout/stderr 两条管道互不干扰（stderr 由
/// `CommandSpawner` 继承给宿主，不能拿来当证据通道）。
pub struct Trace {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
    pid: u32,
}

impl Trace {
    pub fn new(path: Option<std::path::PathBuf>) -> Self {
        let pid = std::process::id();
        let file = path
            .clone()
            .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
        Self { path: path.unwrap_or_default(), file, pid }
    }

    /// 记一条事件。写不进去（路径非法 / 磁盘只读）都**不影响**夹具行为——
    /// 证据面缺了由测试自己的断言去发现，夹具绝不能因为记日志而改变被测行为。
    pub fn record(&mut self, event: &str, mut extra: serde_json::Map<String, serde_json::Value>) {
        let Some(file) = self.file.as_mut() else { return };
        extra.insert("event".to_string(), serde_json::Value::String(event.to_string()));
        extra.insert("pid".to_string(), serde_json::json!(self.pid));
        let line = serde_json::Value::Object(extra);
        let text = match serde_json::to_string(&line) {
            Ok(text) => text,
            Err(_) => return,
        };
        let _ = writeln!(file, "{text}");
        let _ = file.flush();
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// 本进程的 stdout 写端。
///
/// **刻意不用 `std::io::stdout()`**：`close-stdout` 行为要求「关掉写端但进程继续活」，
/// 而标准输出句柄由 std 拥有、关不掉。这里把继承来的 stdio 句柄**接管**成一个
/// `File`（Unix = fd 1，Windows = `GetStdHandle(STD_OUTPUT_HANDLE)`），
/// 于是「关 stdout」就是 drop 这个 `File`（→ `close(1)` / `CloseHandle`），
/// 父进程侧的读线程随即看到 EOF。副作用如实标注：夹具从此不碰 `std::io::stdout()`
/// （否则会在已关闭的句柄上写），日志一律走 stderr。
pub struct SidecarStdout {
    writer: Option<std::io::BufWriter<std::fs::File>>,
}

impl SidecarStdout {
    /// # Safety
    /// 只能对**进程真实继承来的** stdio 句柄各调用一次；接管后不得再使用
    /// `std::io::stdout()`。
    pub unsafe fn inherit() -> Self {
        let file = unsafe {
            #[cfg(unix)]
            {
                use std::os::fd::{AsRawFd, FromRawFd};
                std::fs::File::from_raw_fd(std::io::stdout().as_raw_fd())
            }
            #[cfg(windows)]
            {
                use std::os::windows::io::{AsRawHandle, FromRawHandle};
                std::fs::File::from_raw_handle(std::io::stdout().as_raw_handle())
            }
            #[cfg(not(any(unix, windows)))]
            {
                compile_error!("sidecar 夹具需要 Unix 或 Windows 的 stdio 句柄接管实现")
            }
        };
        Self { writer: Some(std::io::BufWriter::new(file)) }
    }

    pub fn is_open(&self) -> bool {
        self.writer.is_some()
    }

    /// 写一行帧（自带 `\n`）。写失败（对端已关管道）返回 `false`，由调用方决定
    /// 继续还是收手——夹具绝不 panic。
    pub fn write_line(&mut self, frame: &[u8]) -> bool {
        let Some(writer) = self.writer.as_mut() else { return false };
        let ok = writer
            .write_all(frame)
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| writer.flush())
            .is_ok();
        if !ok {
            // 管道断了就丢掉写端，后续写入直接短路（broken pipe 路径）。
            self.writer = None;
        }
        ok
    }

    /// 关掉 stdout 写端（进程继续活）。
    pub fn close(&mut self) -> bool {
        let writer = self.writer.take();
        let Some(mut writer) = writer else { return false };
        let _ = writer.flush();
        // drop(BufWriter<File>) 会关掉底层句柄——这就是「EOF 但进程还活着」的实现点。
        drop(writer);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// argv 面必须**双向自洽**：测试用 `to_args` 造参数，夹具用 `from_args` 解析。
    /// 任何一边改名，这条用例先红，而不是等到端到端测试里出现「参数被静默忽略」。
    #[test]
    fn spec_round_trips_through_argv() {
        let spec = Spec {
            behavior: Behavior::CloseStdout,
            exit_code: 9,
            exit_after_requests: 2,
            delay_ms: 25,
            close_after_requests: 3,
            exit_after_ms: 400,
            reply_on_trigger: true,
            out_of_order: 4,
            fanout: 5,
            reply_error_code: Some(-32001),
            force_generation: Some(4242),
            frame_bytes: 4096,
            trace: Some(std::path::PathBuf::from("trace.jsonl")),
        };
        let parsed = Spec::from_args(spec.to_args()).expect("argv 往返必须可解析");
        assert_eq!(spec, parsed);
    }

    #[test]
    fn default_spec_round_trips() {
        let spec = Spec::default();
        assert_eq!(spec, Spec::from_args(spec.to_args()).unwrap());
        assert_eq!(spec.behavior, Behavior::Echo);
        assert_eq!(spec.fanout, 1);
    }

    #[test]
    fn unknown_flag_and_bad_number_are_rejected_not_ignored() {
        assert!(Spec::from_args(["--behavior", "echo", "--nope", "1"]).is_err());
        assert!(Spec::from_args(["--delay-ms", "abc"]).is_err());
        assert!(Spec::from_args(["--behavior"]).is_err());
        assert!(Spec::from_args(["--behavior", "telepathy"]).is_err());
        assert!(Spec::from_args(["--fanout", "0"]).is_err());
    }

    #[test]
    fn trace_without_path_records_nothing_and_never_panics() {
        let mut trace = Trace::new(None);
        trace.record("event", serde_json::Map::new());
        assert!(trace.path().as_os_str().is_empty());
    }
}
