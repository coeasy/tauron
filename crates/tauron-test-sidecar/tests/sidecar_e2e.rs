//! 真 sidecar 端到端（V7-P1-05）：**真起操作系统进程**、**真走 stdio 帧回路**。
//!
//! 这些用例是 `tauron-proc` 那条「宿主 → stdin 帧 → sidecar → stdout 帧 → 结算」
//! 链路第一次拿到运行期证据。此前仓内只有 fake spawner + 内存管道，审计因此判定
//! 「测试不能证明 sidecar 真收到请求、回包、异常退出、重启后旧回复被拒绝」。
//!
//! 覆盖面（每条一个 `#[test]`，标题即结论）：
//! 1. `real_sidecar_round_trip_*` —— 真进程真收真回，经**生产**投递/结算面闭环，
//!    禁用时经**生产**租约回收真杀进程；
//! 2. `crash_is_counted_once_*` —— 真崩溃只在正向观测到 `Exited` 时计一次，
//!    重复轮询不放大，`Unknown` 不二次计入；
//! 3. `oversized_frame_*` —— 超 `MAX_FRAME_BYTES` 的帧是**类型化结局**
//!    （`ProcessFrameSink::on_frame_oversized`），且"只断帧回路、不杀进程"；
//!    同构造但在上限内的对照帧正常结算；
//! 4. `stale_generation_reply_*` —— 旧进程**真的还活着**、真的在换代之后回帧，
//!    被生产代次守卫丢弃（不是靠"顺手把它杀了"侥幸躲过）；
//! 5. `stdout_eof_mid_conversation_*` —— 回完就关 stdout 但进程不退（病态 sidecar），
//!    读线程 EOF 回收不悬挂、不丢句柄、不留孤儿，且**写侧句柄一起摘掉**（后续投递
//!    以 `NotFound` 如实失败，不静默丢帧）；
//! 6. `rapid_calls_are_bounded_*` —— 连帧不打穿有界 pending 表，触顶报
//!    `E_CALL_PENDING_FULL`，且已投递的每一帧都有真回答；**轮 30** 起这条同时覆盖
//!    第二道界（pid 写队列 `WouldBlock`），两道界必须同码且各自交代出处；
//! 7. `out_of_order_replies_*` —— 到达顺序等于 sidecar 写出顺序（守卫与顺序无关）；
//! 8. `error_reply_*` —— `error` 回帧结算成失败并带上错误码；
//! 9. `silent_sidecar_*` —— 真收到但永不回帧（超时路径的驱动源）；
//! 10. `deaf_sidecar_backpressures_*` —— 对端从不读 stdin：写侧填满后报
//!     `WouldBlock`、每次投递都在毫秒级返回、超限帧入队前即被拒（轮 2）。
//!
//! 跨平台约定：不用信号、不依赖 shell 语义、不假设 pid 复用；所有等待都是
//! **带谓词的轮询 + 硬超时**（`wait_until`），`sleep` 只是轮询间隔，不是正确性证据。
//! 泄漏纪律：断言失败时 `CommandSpawner::Drop` 仍会把每个仍在跟踪的 pid 走一遍
//! tree-kill；每个用例结束前还显式断言 `tracked() / sink_count()` 归零。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauron_adapter::process_delivery::ProcessFrameSinkImpl;
use tauron_adapter::{
    cmd_call_plugin, cmd_registry_admin, cmd_runtime_health, cmd_runtime_spawn, PluginRuntimeState,
    ProviderResult, RuntimeAbiFingerprint, RuntimeBinarySignature, RuntimeSpawnProfile,
};
use tauron_host::authz::RegistryAdminOp;
use tauron_host::error::ErrorCode;
use tauron_host::manifest::{
    parse_version_range, EntrySpec, PermissionIndex, PluginId, PluginManifest, PluginType,
};
use tauron_host::registry::{CallState, PendingCall, Registry};
use tauron_host::runtime::{LeaseReaper, ReapOutcome, ReapStats, RuntimeHandle};
use tauron_proc::spawner::MAX_FRAME_BYTES;
use tauron_proc::{
    AbiFingerprint, BinarySignature, CommandSpawner, KillOutcome, ProcSpawner, ProcessFrameSink,
    ProcessStatus, SpawnConfig, SIDECAR_ABI_INTERFACE_HASH, SIDECAR_ABI_RUST_VERSION,
};
use tauron_test_sidecar::{Behavior, Spec, FLUSH_TRIGGER};

/// 一次等待的硬上限。真进程 + 真管道在本地与 CI 上都远快于此；用它而不是无限等待，
/// 保证任何路径失配时是**失败**而不是挂死。
const WAIT: Duration = Duration::from_secs(30);
/// 轮询间隔。**只是**间隔——正确性一律由谓词与硬超时保证。
const POLL: Duration = Duration::from_millis(5);

const SIDECAR_EXE: &str = env!("CARGO_BIN_EXE_tauron_test_sidecar");

/// 带谓词的轮询：命中即返回，超过 `WAIT` 直接 panic（附等待目标）。
fn wait_until(what: &str, mut probe: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        if probe() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("等待超时（{WAIT:?}）：{what}");
        }
        std::thread::sleep(POLL);
    }
}

/// 读 sidecar 自己写的 JSONL 证据（`--trace`）。
///
/// 为什么测试要靠它：stdout 上的字节只能证明**回帧内容**，证明不了"请求真的到达了
/// 那个 pid"。后者必须由 sidecar 自己落笔（连同它自己的 pid），才排掉"宿主以为投了"。
fn trace_events(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

// ──────────────────────────────────────────────────────────────────────────
// 观测件
// ──────────────────────────────────────────────────────────────────────────

/// 裸 stdout 帧记录器：按到达顺序留下 `(pid, 原始字节)`、超帧结局与 EOF 名单。
#[derive(Default)]
struct Recorder {
    frames: Mutex<Vec<(u32, Vec<u8>)>>,
    oversized: Mutex<Vec<(u32, usize)>>,
    eof: Mutex<Vec<u32>>,
}

impl Recorder {
    /// 全部已到达的帧（**非破坏性**：读多次得到同一份历史）。
    fn frames(&self) -> Vec<(u32, Vec<u8>)> {
        self.frames.lock().clone()
    }

    /// 取走并清空（需要把真字节交给别处重放时用）。
    fn drain_frames(&self) -> Vec<(u32, Vec<u8>)> {
        std::mem::take(&mut *self.frames.lock())
    }

    fn oversized(&self) -> Vec<(u32, usize)> {
        self.oversized.lock().clone()
    }

    fn saw_eof(&self, pid: u32) -> bool {
        self.eof.lock().contains(&pid)
    }

    /// 已到达且能解析成 JSON 的帧（顺序保留，非破坏性）。
    fn parsed(&self) -> Vec<Value> {
        self.frames().iter().filter_map(|(_, b)| serde_json::from_slice(b).ok()).collect()
    }
}

impl ProcessFrameSink for Recorder {
    fn on_frame(&self, pid: u32, frame: &[u8]) {
        self.frames.lock().push((pid, frame.to_vec()));
    }

    fn on_eof(&self, pid: u32) {
        self.eof.lock().push(pid);
    }

    fn on_frame_oversized(&self, pid: u32, observed_bytes: usize) {
        self.oversized.lock().push((pid, observed_bytes));
    }
}

/// 组合接收器：先记原始帧，再把同一帧交给**生产** `ProcessFrameSinkImpl`。
///
/// 为什么必须组合：生产守卫丢弃回帧时**不留任何可查痕迹**（只有 `eprintln!`）。
/// 于是「帧真的到了宿主」由记录段证明，「调用没被结算」由生产段 + pending 表证明，
/// 少任何一段都退化成猜测。
struct StagedSink {
    recorder: Arc<Recorder>,
    production: ProcessFrameSinkImpl,
}

impl ProcessFrameSink for StagedSink {
    fn on_frame(&self, pid: u32, frame: &[u8]) {
        self.recorder.on_frame(pid, frame);
        self.production.on_frame(pid, frame);
    }

    fn on_eof(&self, pid: u32) {
        self.recorder.on_eof(pid);
    }
}

/// 记账版 `LeaseReaper`：与生产 `SpawnerReaper` 同语义（转调 `ProcSpawner::kill`），
/// 但结果按 pid 留痕，且**可以关掉真终止**。
///
/// `disable_termination` 复刻的是生产里被文档化的那条路径：**没有真终止能力时，
/// 租约照摘、进程照活、`reap.failures` 留痕**。用例 4 需要它——否则旧进程在换代
/// 那一刻就被杀掉了，「旧回帧晚到」这个被测对象根本不存在。
struct Reaper {
    spawner: Arc<CommandSpawner>,
    terminate: AtomicBool,
    log: Mutex<Vec<(u32, String)>>,
}

impl Reaper {
    fn new(spawner: Arc<CommandSpawner>) -> Self {
        Self { spawner, terminate: AtomicBool::new(true), log: Mutex::new(Vec::new()) }
    }

    fn disable_termination(&self) {
        self.terminate.store(false, Ordering::SeqCst);
    }

    fn outcomes(&self) -> Vec<(u32, String)> {
        self.log.lock().clone()
    }
}

impl LeaseReaper for Reaper {
    fn kill(&self, pid: u32) -> Result<ReapOutcome, String> {
        if !self.terminate.load(Ordering::SeqCst) {
            self.log.lock().push((pid, "skipped".to_string()));
            // 如实报告"没杀成"——生产语义里这正是 `failures += 1` 的来源。
            return Err(format!("夹具回收器被显式停用（pid {pid} 未被终止，进程存活）"));
        }
        let outcome = ProcSpawner::kill(&*self.spawner, pid).map_err(|e| e.to_string())?;
        self.log.lock().push((pid, format!("{outcome:?}")));
        Ok(match outcome {
            KillOutcome::Terminated => ReapOutcome::Terminated,
            KillOutcome::AlreadyGone => ReapOutcome::AlreadyGone,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 装配
// ──────────────────────────────────────────────────────────────────────────

struct Harness {
    spawner: Arc<CommandSpawner>,
    state: PluginRuntimeState,
    reaper: Arc<Reaper>,
    plugin: String,
    spec: Spec,
    trace_path: PathBuf,
    /// 声明在**最后**：spawner/状态先释放（内含 tree-kill），再销毁 sidecar 写
    /// `--trace` 的临时目录，避免在清理途中把证据文件抽掉。刻意不读，只为持有。
    _dir: tempfile::TempDir,
}

fn empty_index() -> PermissionIndex {
    PermissionIndex { version: 1, generated_at: None, entries: vec![] }
}

fn process_manifest(id: &str, exe: &Path) -> PluginManifest {
    PluginManifest {
        id: PluginId::new(id).expect("合法插件 id"),
        name: "sidecar fixture".to_string(),
        version: semver::Version::parse("1.0.0").expect("合法版本"),
        plugin_type: PluginType::Process,
        // §4.2：process 型必须声明非空 `entry.sidecar`——这里就是夹具二进制的真路径。
        entry: EntrySpec {
            js: None,
            sidecar: Some(exe.to_string_lossy().into_owned()),
            wasm: None,
            ui: None,
        },
        permissions: vec![],
        scopes: Default::default(),
        platforms: vec![],
        framework: parse_version_range(">=1.0.0, <3.0.0").expect("合法框架区间"),
        abi: None,
        contributes: Default::default(),
        settings_schema: None,
        events: Default::default(),
        host_functions: vec![],
        min_allowed_version: None,
        signature: None,
        publisher: None,
    }
}

impl Harness {
    /// 装配一套「真 spawner + 真注册表 + 真投递面」，并把行为面 `spec` 落到 argv。
    fn new(label: &str, mut spec: Spec) -> Self {
        let dir = tempfile::TempDir::new().expect("临时目录");
        let trace_path = dir.path().join(format!("{label}.jsonl"));
        spec.trace = Some(trace_path.clone());
        let exe = PathBuf::from(SIDECAR_EXE);
        let spawner = Arc::new(CommandSpawner::new());
        let state = PluginRuntimeState::with_spawner(spawner.clone());
        let reaper = Arc::new(Reaper::new(spawner.clone()));
        // 覆盖装配层注入的生产终止器：语义一致（同样转调 `kill`），但结果可按 pid 记账。
        state.registry.set_lease_reaper(reaper.clone());
        let plugin = format!("com.example.{label}");
        let id = PluginId::new(&plugin).expect("合法插件 id");
        state
            .registry
            .install(&empty_index(), process_manifest(&plugin, &exe))
            .unwrap_or_else(|e| panic!("安装 process 型夹具插件失败：{e:?}"));
        state
            .registry
            .admin_op(&id, RegistryAdminOp::Enable)
            .unwrap_or_else(|e| panic!("启用夹具插件失败：{e:?}"));
        Self { spawner, state, reaper, plugin, spec, trace_path, _dir: dir }
    }

    fn exe(&self) -> String {
        PathBuf::from(SIDECAR_EXE).to_string_lossy().into_owned()
    }

    fn spawn_config(&self) -> SpawnConfig {
        SpawnConfig {
            binary_path: self.exe(),
            args: self.spec.to_args(),
            env: Default::default(),
            // 如实自报：这三项**没有任何运行期校验**（见 `src/lib.rs` 头部的诚实边界）。
            signature: BinarySignature {
                algorithm: "fixture".to_string(),
                signature: "not-a-signature".to_string(),
                signer_id: "ci-fixture".to_string(),
            },
            binary_hash: "0".repeat(64),
            abi: AbiFingerprint::now(SIDECAR_ABI_RUST_VERSION, SIDECAR_ABI_INTERFACE_HASH),
        }
    }

    /// 生产 spawn 面：`host_runtime_spawn` → 配置/ABI 校验 → 真起进程 → 铸租约 →
    /// 注册生产回帧接收器（`ProcessFrameSinkImpl`）。
    fn spawn_via_adapter(&self) -> RuntimeHandle {
        let profile = RuntimeSpawnProfile {
            binary_path: None, // 走 manifest 的 `entry.sidecar`
            args: self.spec.to_args(),
            env: Default::default(),
            signature: RuntimeBinarySignature {
                algorithm: "fixture".to_string(),
                signature: "not-a-signature".to_string(),
                signer_id: "ci-fixture".to_string(),
            },
            binary_hash: "0".repeat(64),
            abi: RuntimeAbiFingerprint {
                rust_version: SIDECAR_ABI_RUST_VERSION.to_string(),
                interface_hash: SIDECAR_ABI_INTERFACE_HASH.to_string(),
            },
        };
        let spawned = cmd_runtime_spawn(&self.state, &self.plugin, &profile);
        spawned.unwrap_or_else(|e| panic!("`cmd_runtime_spawn` 起真 sidecar 失败：{e:?}"))
    }

    /// 直连 `CommandSpawner`：只要「真起进程 + 真帧回路」，不经过插件状态机。
    fn spawn_with_sink(&self, sink: Arc<dyn ProcessFrameSink>) -> u32 {
        let spawned =
            ProcSpawner::spawn(&*self.spawner, &self.spawn_config()).expect("真起进程应成功");
        assert!(
            self.spawner.register_frame_sink(spawned.pid, sink),
            "刚起的进程不该已经 EOF（登记回帧接收器被拒）"
        );
        spawned.pid
    }

    fn status(&self, pid: u32) -> ProcessStatus {
        ProcSpawner::status(&*self.spawner, pid)
    }

    fn is_alive(&self, pid: u32) -> bool {
        matches!(self.status(pid), ProcessStatus::Alive)
    }

    fn kill(&self, pid: u32) -> KillOutcome {
        ProcSpawner::kill(&*self.spawner, pid)
            .unwrap_or_else(|e| panic!("kill pid {pid} 失败：{e}"))
    }

    /// 收干净一个 pid 并断言"没有孤儿"。
    ///
    /// 用在**结局随时序而变**的场合：读线程若已经把 stdout EOF 的进程回收掉，
    /// `kill` 就如实报 `AlreadyGone`；仍活着才是 `Terminated`。两者都算收干净，
    /// 但都不许留下仍在跟踪的句柄——所以真正的断言是"探测不到活进程 + 表里没了"。
    fn reap_pid(&self, pid: u32) {
        // 只有**确认跟踪着句柄**（tracked ≥ 1）时才要求杀进程这一步有明确结局：
        // 此时 `kill` 要么真杀掉（Terminated）、要么如实报 AlreadyGone，都不留孤儿。
        // 若条目已被别的路径摘干净（tracked == 0），句柄早在 EOF/status 回收时就
        // 释放了，`kill` 对未知 pid 只能报 AlreadyGone，这里改断言"不再有活进程"。
        if self.spawner.tracked() >= 1 {
            if let Err(err) = ProcSpawner::kill(&*self.spawner, pid) {
                // provider 终止失败会保留句柄；不 panic，交给下面的轮询 + Drop 兜底。
                eprintln!("夹具收尾：kill({pid}) 未确认终止（{err}），继续等待回收");
            }
        }
        wait_until("进程真的没了且不再被跟踪", || {
            // `status()` 本身就会顺带回收已退出的子进程（P2-4 的主动 reap）。
            !matches!(self.status(pid), ProcessStatus::Alive) && self.spawner.tracked() == 0
        });
    }

    fn reap(&self) -> ReapStats {
        self.state.registry.runtime_reap_stats()
    }

    fn registry(&self) -> &Registry {
        &self.state.registry
    }

    fn plugin_id(&self) -> PluginId {
        PluginId::new(&self.plugin).expect("合法插件 id")
    }

    /// 生产调用面发起一次调用（投递 = 真写进 sidecar 的 stdin）。
    fn call(&self, cmd: &str, args: Value) -> PendingCall {
        let outcome =
            cmd_call_plugin(&self.state, "main", &self.plugin, cmd, args).expect("命令面应成功");
        match outcome {
            ProviderResult::Value(call) => call,
            ProviderResult::Unsupported(body) => {
                panic!("进程插件应有投递通路，却返回 Unsupported：{}", body.reason)
            }
        }
    }

    /// 裸请求帧（不经注册表；守卫/超帧/触发帧用例用）。
    fn write_raw_frame(&self, pid: u32, id: u64, call_id: &str, generation: u64, method: &str) {
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {},
            "callId": call_id,
            "caller": "main",
            "target": self.plugin,
            "runtimeGeneration": generation,
        });
        let bytes = serde_json::to_vec(&frame).expect("帧应可序列化");
        ProcSpawner::write_frame(&*self.spawner, pid, &bytes).expect("写 stdin 帧应成功");
    }

    /// 裸写并**返回结局**（背压/超长用例要判的就是这个结局，不能 `expect`）。
    fn try_write_frame(&self, pid: u32, bytes: &[u8]) -> std::io::Result<()> {
        ProcSpawner::write_frame(&*self.spawner, pid, bytes)
    }

    fn wait_trace_event(&self, event: &str) -> Value {
        let path = self.trace_path.clone();
        let mut found = None;
        wait_until(&format!("sidecar 自证事件 `{event}`"), || {
            match trace_events(&path).into_iter().find(|e| e["event"] == json!(event)) {
                Some(entry) => {
                    found = Some(entry);
                    true
                }
                None => false,
            }
        });
        found.expect("wait_until 命中即有值")
    }

    fn count_trace_events(&self, event: &str) -> usize {
        trace_events(&self.trace_path).iter().filter(|e| e["event"] == json!(event)).count()
    }

    fn wait_state(&self, call_id: &str, want: CallState) -> PendingCall {
        let mut last = None;
        wait_until(&format!("调用 {call_id} 进入 {want:?}"), || {
            match self.registry().peek_call(call_id) {
                Ok(call) => {
                    let done = call.state == want;
                    last = Some(call);
                    done
                }
                Err(_) => false,
            }
        });
        last.expect("wait_until 命中即有值")
    }

    /// 生产离场路径：禁用 → 状态离开 ENABLED/RUNNING → 注册表回收**仍在跑**的租约。
    fn disable(&self) {
        cmd_registry_admin(&self.state, &self.plugin, RegistryAdminOp::Disable)
            .unwrap_or_else(|e| panic!("禁用夹具插件失败：{e:?}"));
    }

    /// 收尾：启动面簿记必须归零（无孤儿进程、无残留 sink、无悬挂写句柄）。
    ///
    /// `closed_count()` **不**在这里断言：EOF 标记按设计保留到下一次 `spawn` 才由
    /// `retain_tracked` 裁掉（它挡的是"已 EOF 的 pid 迟到登记 sink"），本用例之后
    /// 没有再起进程，非零才是正确状态。
    fn finish(&self) {
        wait_until("启动面簿记归零（无孤儿进程 / 无残留 sink）", || {
            self.spawner.tracked() == 0 && self.spawner.sink_count() == 0
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 用例
// ──────────────────────────────────────────────────────────────────────────

/// ① 真进程 → 真收帧 → 真回帧 → 生产结算 → 禁用时真杀进程。
#[test]
fn real_sidecar_round_trip_settles_and_disable_kills_the_process() {
    let harness =
        Harness::new("rt", Spec { behavior: Behavior::Echo, exit_code: 0, ..Default::default() });
    let handle = harness.spawn_via_adapter();
    let pid = handle.pid;
    assert!(harness.is_alive(pid), "`cmd_runtime_spawn` 之后必须真有一个活进程");

    let call = harness.call("ping", json!({"x": 1}));
    assert_eq!(call.runtime_generation, Some(handle.generation));

    let settled = harness.wait_state(&call.call_id, CallState::Settled);
    let result = settled.result.expect("成功回帧应带 result");
    // `sidecarPid` 由**子进程自己**填：证明这一帧真是那个 OS 进程写的。
    assert_eq!(result["sidecarPid"], json!(pid));
    // 生产投递造的帧 shape，就是被对端消费掉的那一份。
    assert_eq!(result["echo"]["callId"], json!(call.call_id));
    assert_eq!(result["echo"]["method"], json!("ping"));
    assert_eq!(result["echo"]["params"], json!({"x": 1}));

    // sidecar 自己的证据文件：同一个 pid 真的落笔收到这一帧。
    let received = harness.wait_trace_event("request-received");
    assert_eq!(received["pid"], json!(pid));
    assert_eq!(received["callId"], json!(call.call_id));
    assert_eq!(received["runtimeGeneration"], json!(handle.generation.0));

    // 发起方取件 = pending 表回收。
    let taken = harness.registry().take_call(&call.call_id).expect("取件应成功");
    assert_eq!(taken.state, CallState::Settled);
    assert_eq!(harness.registry().pending_len(), 0, "取走后不应残留 pending");

    // 禁用 → 租约回收 → 生产 kill 路径真的终止这个**活**进程。
    harness.disable();
    wait_until("禁用后进程真的消失", || !harness.is_alive(pid));
    let reap = harness.reap();
    assert_eq!(reap.attempts, 1, "禁用应恰好发起一次终止");
    assert_eq!(reap.terminated, 1, "这次终止必须真的杀掉一个活进程");
    assert_eq!(reap.failures, 0, "回收不该留痕失败：{:?}", reap.last_error);
    assert!(harness.reaper.outcomes().iter().any(|(p, o)| *p == pid && o == "Terminated"));
    harness.finish();
}

/// ② 真崩溃：正向观测到 `Exited` 才计一次，重复轮询不放大，`Unknown` 不二次计入。
#[test]
fn crash_is_counted_once_and_reap_reports_the_dead_pid_honestly() {
    // 关 stdout 之后 `--exit-after-ms` 到点退出：读线程先看到「进程还活着」并把句柄
    // 放回表里，于是崩溃由**下一次 health 轮询**确定地观测为 `Exited`——不再赌
    // "EOF 与 try_wait 谁先到"（那条竞态里 `Unknown` 故意不计崩溃）。
    let harness = Harness::new(
        "crash",
        Spec {
            behavior: Behavior::CloseStdout,
            exit_code: 9,
            close_after_requests: 1,
            exit_after_ms: 400,
            ..Default::default()
        },
    );
    let handle = harness.spawn_via_adapter();
    let pid = handle.pid;
    assert!(harness.is_alive(pid));

    let call = harness.call("ping", json!({"x": 1}));
    harness.wait_state(&call.call_id, CallState::Settled);
    harness.registry().take_call(&call.call_id).expect("取件");
    // EOF 已发生，但进程还在跑 → 句柄必须留在表里（否则崩溃永远观测不到）。
    wait_until("EOF 后仍跟踪该 pid", || harness.spawner.tracked() == 1);

    let mut first = None;
    wait_until("崩溃被正向观测（Exited → 计一次）", || {
        let health = cmd_runtime_health(&harness.state, &handle.lease).expect("health 应可查");
        if health.crashes == 1 {
            first = Some(health);
            return true;
        }
        false
    });
    let health = first.expect("命中即有值");
    assert_eq!(health.status, ProcessStatus::Exited, "首见崩溃必须来自正向 `Exited` 观测");
    assert_eq!(health.pid, pid);
    assert!(!health.alive, "已退出的进程不得报成存活");
    assert_eq!(harness.state.proc_runtime.crash_count(&harness.plugin), 1);

    // 反复轮询不得把同一次死亡计第二次；此时句柄已被摘走 → `Unknown` → 更不该计入。
    for _ in 0..5 {
        let again = cmd_runtime_health(&harness.state, &handle.lease).expect("重复 health");
        assert_eq!(again.crashes, 1, "重复轮询放大了崩溃计数");
        assert!(!again.alive);
    }
    assert_eq!(harness.status(pid), ProcessStatus::Unknown, "句柄摘走后再探测应是 Unknown");

    // 摘租约（生产语义：已标崩溃的租约**留**在表里给 health 报账，所以这里显式回收）。
    // 进程早就不在了 → 终止动作必须如实记为 AlreadyGone，不许谎报"我杀了它"。
    let removed = harness.registry().runtime_remove(&harness.plugin_id());
    assert_eq!(removed.map(|h| h.pid), Some(pid));
    let reap = harness.reap();
    assert_eq!(reap.attempts, 1);
    assert_eq!(reap.terminated, 0, "进程本就不在，不该记成被杀掉的");
    assert_eq!(reap.already_gone, 1);
    assert_eq!(reap.failures, 0);
    assert_eq!(harness.spawner.tracked(), 0);
    harness.finish();
}

/// ③ 超帧 = 类型化结局，且只断开帧回路；对照帧（同构造、在上限内）正常结算。
#[test]
fn oversized_frame_surfaces_as_typed_outcome_not_a_generic_error() {
    let recorder = Arc::new(Recorder::default());
    let harness = Harness::new(
        "oversize",
        Spec {
            behavior: Behavior::Oversize,
            // 刻意比上限大一字节（含结尾换行）：帧**内容仍是合法 JSON**，被拒的理由
            // 只能是字节上限，不混进"解析失败"这个干扰项。
            frame_bytes: MAX_FRAME_BYTES + 1,
            exit_code: 0,
            ..Default::default()
        },
    );
    let pid = harness.spawn_with_sink(recorder.clone());
    harness.write_raw_frame(pid, 1, "call-oversize", 1, "ping");

    wait_until("超帧报告成类型化结局", || !recorder.oversized().is_empty());
    let events = recorder.oversized();
    assert_eq!(events.len(), 1, "一条超帧应恰好报告一次");
    assert_eq!(events[0].0, pid, "结局必须挂在真 pid 上");
    // 读线程用 `take(MAX+1)` 截断，因此观测到的字节数正落在截断点上。
    assert_eq!(events[0].1, MAX_FRAME_BYTES + 1, "观测字节数不等于读线程的截断点");
    assert!(recorder.frames().is_empty(), "超帧不得被当成正常帧交给 `on_frame`");
    // 类型化结局之后仍会走 EOF 收尾（与干净退出同一条收束路径），但**不杀进程**：
    // 帧回路断开与进程终止是两件事，这里如实固定成现状。
    wait_until("超帧后 sink 摘除、帧回路断开", || {
        recorder.saw_eof(pid) && harness.spawner.sink_count() == 0
    });
    // 断开的是**帧回路**，不是进程：终止记账一次都不该涨。
    assert_eq!(harness.reap().attempts, 0, "超帧不该触发任何终止进程的动作");
    // 而宿主摘掉写侧之后，夹具自己会看到 stdin EOF 退出——所以这里的结局允许
    // `Terminated`（还没退）或 `AlreadyGone`（已经退了），但必须收干净。
    harness.reap_pid(pid);
    harness.finish();

    // 对照：同一份构造代码、长度在上限内 → 必须正常交到 `on_frame`（证明失败只因字节数）。
    let control = Arc::new(Recorder::default());
    let under = Harness::new(
        "undersize",
        Spec {
            behavior: Behavior::Oversize,
            frame_bytes: MAX_FRAME_BYTES - 1024,
            exit_code: 0,
            ..Default::default()
        },
    );
    let pid2 = under.spawn_with_sink(control.clone());
    under.write_raw_frame(pid2, 2, "call-under", 1, "ping");
    wait_until("上限内的同构造帧应正常到达", || !control.frames().is_empty());
    assert!(control.oversized().is_empty(), "没超就不该报超帧结局");
    let (frame_pid, bytes) = control.frames().remove(0);
    assert_eq!(frame_pid, pid2);
    // 精确尺寸：夹具写出的整帧（含换行）恰等于 `--frame-bytes`。
    assert_eq!(bytes.len() + 1, MAX_FRAME_BYTES - 1024);
    let frame: Value = serde_json::from_slice(&bytes).expect("对照帧应是合法 JSON");
    assert_eq!(frame["callId"], json!("call-under"));
    // 对照用例没有 EOF：读线程还阻塞着，写侧没被摘 → 进程一定还活着，必须真杀掉。
    assert_eq!(under.kill(pid2), KillOutcome::Terminated);
    under.finish();
}

/// ④ 换代之后旧进程**真的**回帧 → 生产守卫丢弃（不是靠"顺手杀了它"）。
#[test]
fn stale_generation_reply_from_a_live_old_process_is_dropped_by_the_guard() {
    let harness = Harness::new(
        "stale",
        Spec {
            behavior: Behavior::Echo,
            exit_code: 0,
            // 回帧先攒着，等宿主已经换代之后再由 sidecar 自己吐出去。
            reply_on_trigger: true,
            ..Default::default()
        },
    );
    // 关掉真终止：这是被文档化的生产路径（摘租约照做、进程照活、`reap.failures` 留痕）。
    // 没有它，旧进程在换代瞬间就被杀掉，「旧回帧晚到」这个被测对象压根不存在。
    harness.reaper.disable_termination();

    let old = harness.spawn_via_adapter();
    let recorder = Arc::new(Recorder::default());
    let staged = Arc::new(StagedSink {
        recorder: recorder.clone(),
        production: ProcessFrameSinkImpl::new(harness.state.registry.clone()),
    });
    assert!(harness.spawner.register_frame_sink(old.pid, staged), "应为旧进程登记组合接收器");

    let call = harness.call("late", json!({"x": 1}));
    assert_eq!(call.runtime_generation, Some(old.generation));
    // 攒帧阶段：请求真到了，但没有回帧。
    let received = harness.wait_trace_event("request-received");
    assert_eq!(received["pid"], json!(old.pid));
    assert!(recorder.frames().is_empty(), "攒帧模式下不该有任何回帧");
    assert_eq!(
        harness.registry().peek_call(&call.call_id).expect("调用在表").state,
        CallState::Pending
    );

    // 换代：把 old 标成崩溃（就是 health 探测到 `Exited` 时调的那同一个公开 API），
    // 再起一个**真**的新进程占住同一插件身份。
    harness.registry().runtime_mark_crashed(&old.lease).expect("标记崩溃");
    let fresh = harness.spawn_via_adapter();
    assert_ne!(fresh.pid, old.pid, "新进程应是另一个真 pid");
    assert_ne!(fresh.generation, old.generation, "换代应推进代次");
    assert!(harness.is_alive(old.pid), "旧进程必须真的还活着——守卫才是唯一防线");
    assert!(harness.reap().failures >= 1, "关掉终止能力必须在留痕里看得见：{:?}", harness.reap());

    // 让旧进程真的把攒下的回帧写进**它自己的 stdout** → 宿主读线程真的收到。
    harness.write_raw_frame(old.pid, 99, "trigger-1", old.generation.0, FLUSH_TRIGGER);
    wait_until("旧进程的真回帧到达宿主", || {
        recorder.parsed().iter().any(|f| f["callId"] == json!(call.call_id))
    });
    let stale = recorder
        .parsed()
        .into_iter()
        .find(|frame| frame["callId"] == json!(call.call_id))
        .expect("旧进程应回这一帧");
    assert_eq!(
        stale["runtimeGeneration"],
        json!(old.generation.0),
        "回帧自报旧代次（该字段只是回显，守卫不采信它）"
    );
    // 关键：帧真的到了，调用**没有**被结算。
    assert_eq!(
        harness.registry().peek_call(&call.call_id).expect("调用还在表").state,
        CallState::Pending,
        "旧代次进程的真回帧被结算了——代次守卫失效"
    );
    assert_eq!(harness.registry().pending_for("main"), 1);

    // 反向验证：守卫认的是注册表里的活租约，不是帧上自报的字段——把**同一份真字节**
    // 冒充当前 pid 重放，仍必须被拒（代次不符，而非仅 pid 不符）。
    let replay = recorder
        .drain_frames()
        .into_iter()
        .find(|(_, bytes)| {
            serde_json::from_slice::<Value>(bytes)
                .map(|v| v["callId"] == json!(call.call_id))
                .unwrap_or(false)
        })
        .expect("上面已按内容找到这一帧");
    ProcessFrameSinkImpl::new(harness.state.registry.clone()).on_frame(fresh.pid, &replay.1);
    assert_eq!(
        harness.registry().peek_call(&call.call_id).expect("调用还在表").state,
        CallState::Pending,
        "冒充当前 pid 的旧代次回帧被结算了——守卫只认 pid、没认代次"
    );

    // 清理：被丢弃的调用由发起方取消回收（不留 pending 痕）。
    assert_eq!(harness.kill(old.pid), KillOutcome::Terminated);
    assert!(!harness.is_alive(old.pid));
    harness.registry().call_cancel(&call.call_id).expect("取消这次未结算调用");
    assert_eq!(harness.registry().pending_len(), 0, "丢弃的回帧不该在表里留痕");
    // 离场：终止能力被关掉时生产语义是「租约照摘、进程照活、`failures` 留痕」——
    // 这里如实断言这条留痕（换代时一次、禁用时再一次），再由测试显式杀掉那个真进程。
    harness.disable();
    assert_eq!(harness.reap().failures, 2, "两次该终止没终止成，都必须留痕：{:?}", harness.reap());
    assert!(harness.is_alive(fresh.pid), "没有终止能力时进程照旧活着（守卫才是拦下旧帧的东西）");
    assert_eq!(harness.kill(fresh.pid), KillOutcome::Terminated);
    harness.finish();
}

/// ⑤ 回完就关 stdout、进程不退：EOF 回收不悬挂、不丢句柄、不留孤儿，且写侧被一起摘掉。
///
/// 时序是**双向竞态**，两条都算通过——测试判的是「结局集合」，不是某一条路径：
/// - sidecar 先关 stdout：读线程 EOF 回收时 `try_wait` 报「仍活着」，于是**放回**
///   `children` 由 `kill` 继续管（本用例①断言的就是这条：句柄没丢、能真杀掉）；
/// - 宿主先 `status()` 探测：`try_wait` 报「已退出」→ `status()` 当场摘除条目并回收
///   tree，读线程随后 EOF 时发现已不再跟踪。此时 `kill` 如实报 `AlreadyGone`，
///   「还活着」这个瞬时观测点根本不存在，① 的断言无从成立。
///
/// 两条路径都不许留孤儿、也不许留下仍在跟踪的句柄。
#[test]
fn stdout_eof_mid_conversation_reaps_without_hang_or_orphan() {
    let recorder = Arc::new(Recorder::default());
    let harness = Harness::new(
        "eof",
        Spec {
            behavior: Behavior::CloseStdout,
            close_after_requests: 1,
            exit_after_ms: 0, // 关完 stdout 之后**一直活着**
            exit_code: 0,
            ..Default::default()
        },
    );
    let pid = harness.spawn_with_sink(recorder.clone());
    harness.write_raw_frame(pid, 1, "call-eof", 1, "ping");
    wait_until("第一帧正常收到", || !recorder.frames().is_empty());
    wait_until("stdout 已 EOF", || recorder.saw_eof(pid));
    // 写侧句柄必须被 EOF 收尾连带摘掉：此后投递要如实失败（`NotFound`），
    // 不许假装"写进去了"（那正是静默丢帧的形态）。这条与谁先观察到退出无关。
    let err = ProcSpawner::write_frame(&*harness.spawner, pid, b"{\"id\":2}")
        .expect_err("stdout 已 EOF，写侧句柄应已被摘掉");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "失败原因必须是\"没有写句柄\"：{err}");
    assert_eq!(harness.count_trace_events("request-received"), 1, "被拒的写入不该到达对端");
    assert_eq!(recorder.frames().len(), 1, "stdout 已关，不该再有第二帧");

    // ① 病态但真实：进程只是关了 stdout，还在跑——此刻句柄必须在表里、探测必须活着，
    //    并且能被**真杀掉**（不误判成"已经没了"、不丢句柄）。
    //    刻意在 EOF 之后立刻看这一眼，而不是等一个「tracked==1 && alive」的合取谓词：
    //    那个谓词在宿主先探测到退出的路径上永远不成立（见本用例文档注释）。
    if harness.is_alive(pid) {
        assert_eq!(harness.spawner.tracked(), 1, "活着却不被跟踪 = 句柄已丢，收尾会留孤儿");
        assert_eq!(harness.kill(pid), KillOutcome::Terminated, "活着的病态进程应被真杀掉");
        assert!(!harness.is_alive(pid));
    }

    // ② 无论走哪条：收干净——探测不到活进程，且跟踪表归零。
    harness.reap_pid(pid);
    harness.finish();
}

/// ⑥ 连帧不打穿有界界：触顶报 `E_CALL_PENDING_FULL`，且已投递的每一帧都有真回答。
#[test]
fn rapid_calls_are_bounded_by_the_pending_cap_and_every_delivered_frame_is_answered() {
    let harness = Harness::new(
        "bounded",
        Spec { behavior: Behavior::Echo, exit_code: 0, ..Default::default() },
    );
    let handle = harness.spawn_via_adapter();
    assert!(harness.is_alive(handle.pid));

    // 刻意不取件：让 pending 一路涨到**每插件**上限（100，见 `Registry` 文档）。
    //
    // 轮 30 的口径修正——这里断言的是**上界 + 零静默丢帧**，不是"恰好 100 收 30 拒"：
    // 130 发同步洪峰外面还套着第二道界，那个 pid 的**写队列**（32 帧，
    // `CommandSpawner::write_frame` 满时报 `WouldBlock`）。它咬不咬取决于对端进程
    // 这一拍有没有被调度到：机器空闲时 100 帧全数入队，负载下第 33 帧就被拒。
    // 两种结局对调用方本来就是同一件事（受理额度满了，退避后重投），所以必须**同码**；
    // 轮 30 之前写侧失败被塌成 `E_STATE_INVALID_TRANSITION`，本用例正是在并行门禁
    // 下被它抓出来的（负载无关的确定性反证见 `process_delivery_write_failures_map_to_their_own_codes`）。
    let mut accepted: Vec<String> = Vec::new();
    let mut rejected = 0usize;
    let mut unexpected = Vec::new();
    for index in 0..130u64 {
        match cmd_call_plugin(&harness.state, "main", &harness.plugin, "flood", json!({"i": index}))
        {
            Ok(ProviderResult::Value(call)) => accepted.push(call.call_id),
            Ok(ProviderResult::Unsupported(body)) => unexpected.push(body.reason),
            Err(error) => match error.code {
                ErrorCode::E_CALL_PENDING_FULL => {
                    rejected += 1;
                    // 两道界都必须自报出处：只说"满了"而不说满在哪一层，调用方就
                    // 无法判断该取件腾位，还是该等 sidecar 把 stdin 追上。
                    let names_a_bound = error.message.contains("pending call 容量")
                        || error.message.contains("写队列");
                    if !names_a_bound {
                        unexpected
                            .push(format!("E_CALL_PENDING_FULL 没交代出处：{}", error.message));
                    }
                }
                other => unexpected.push(format!("{other:?}")),
            },
        }
    }
    assert!(unexpected.is_empty(), "只该看到「成功」与「受理满」两种结局：{unexpected:?}");
    assert!(accepted.len() <= 100, "每插件 pending 上限应是 100，实收 {}", accepted.len());
    assert!(rejected >= 30, "130 > 100 必然至少拒 30 发；一发没拒说明上限根本没生效");
    // 受理多少就挂在表上多少：写侧落空的调用不得留下影子条目。
    assert_eq!(harness.registry().pending_for("main"), accepted.len());
    assert!(harness.registry().pending_len() <= harness.registry().config().max_pending_calls);
    // sidecar 真收到了这些请求（不是投递即忘）。
    let delivered = accepted.len();
    wait_until(&format!("{delivered} 条请求全部到达对端"), || {
        harness.count_trace_events("request-received") >= delivered
    });

    // 已投递的帧必须逐条有答；全部取件后 pending 归零（准入令牌随之释放）。
    for call_id in &accepted {
        let settled = harness.wait_state(call_id, CallState::Settled);
        assert_eq!(settled.result.as_ref().expect("应有 result")["echo"]["method"], json!("flood"));
    }
    for call_id in &accepted {
        harness.registry().take_call(call_id).expect("取件");
    }
    assert_eq!(harness.registry().pending_len(), 0, "全部取走后 pending 应归零");

    harness.disable();
    wait_until("回收真进程", || !harness.is_alive(handle.pid));
    assert_eq!(harness.reap().terminated, 1);
    harness.finish();
}

/// ⑦ 乱序回帧：到达顺序 = sidecar 写出顺序，守卫与顺序无关。
#[test]
fn out_of_order_replies_arrive_in_the_order_the_sidecar_wrote_them() {
    let recorder = Arc::new(Recorder::default());
    let harness = Harness::new(
        "ooo",
        Spec { behavior: Behavior::Echo, out_of_order: 3, exit_code: 0, ..Default::default() },
    );
    let pid = harness.spawn_with_sink(recorder.clone());
    let ids: Vec<String> = (1..=3u64).map(|i| format!("ooo-{i}")).collect();
    for (index, call_id) in ids.iter().enumerate() {
        harness.write_raw_frame(pid, index as u64 + 1, call_id, 1, "ping");
    }
    wait_until("攒满 3 条后一次性倒序吐出", || recorder.parsed().len() >= 3);
    let frames = recorder.parsed();
    let arrived: Vec<&str> =
        frames.iter().take(3).map(|f| f["callId"].as_str().expect("回帧应带 callId")).collect();
    assert_eq!(arrived, vec![ids[2].as_str(), ids[1].as_str(), ids[0].as_str()], "乱序驱动失效");
    assert_eq!(harness.kill(pid), KillOutcome::Terminated);
    harness.finish();
}

/// ⑧ `error` 回帧结算成失败并带上错误码；顺带证明宿主不采信帧上的代次字段。
#[test]
fn error_reply_settles_as_failure_with_its_error_code() {
    let harness = Harness::new(
        "err",
        Spec {
            behavior: Behavior::Echo,
            reply_error_code: Some(-32001),
            // 帧里 `runtimeGeneration` 被刻意改写成一个不可能的值：结算仍应成功，
            // 因为代次判定看的是注册表活租约，不是自报字段。
            force_generation: Some(999_999),
            exit_code: 0,
            ..Default::default()
        },
    );
    let handle = harness.spawn_via_adapter();
    let call = harness.call("fail", json!({}));
    let settled = harness.wait_state(&call.call_id, CallState::Settled);
    assert_eq!(settled.error_code.as_deref(), Some("-32001"));
    assert!(settled.result.is_none(), "失败回帧不该带 result");
    harness.registry().take_call(&call.call_id).expect("取件");
    harness.disable();
    wait_until("回收真进程", || !harness.is_alive(handle.pid));
    harness.finish();
}

/// ⑨ 真收到但永不回帧：对明确认收到，pending 停在 `Pending`（超时/写侧背压驱动源）。
#[test]
fn silent_sidecar_receives_but_never_answers_and_is_killed_cleanly() {
    let recorder = Arc::new(Recorder::default());
    let harness = Harness::new(
        "silent",
        Spec { behavior: Behavior::Silent, exit_code: 0, ..Default::default() },
    );
    let pid = harness.spawn_with_sink(recorder.clone());
    harness.write_raw_frame(pid, 1, "call-silent", 1, "ping");
    let received = harness.wait_trace_event("request-received");
    assert_eq!(received["pid"], json!(pid));
    assert_eq!(received["callId"], json!("call-silent"));

    // 有界观察窗：这段时间里 sidecar 一帧都没回——"没回"与"没送到"就此分开。
    let window = Instant::now() + Duration::from_millis(300);
    while Instant::now() < window {
        assert!(recorder.frames().is_empty(), "silent 模式不该回帧");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(harness.is_alive(pid), "不回帧的进程仍应活着——超时路径的前提就是进程好着、只是不答");
    assert_eq!(harness.kill(pid), KillOutcome::Terminated);
    wait_until("杀掉后读线程收尾", || recorder.saw_eof(pid));
    harness.finish();
}

/// ⑩ 对端装聋（**从不**读 stdin）：宿主写侧必须有界、且不钉住投递线程。
///
/// 轮 2 修掉的是这条链路上真实存在的可用性缺陷：`write_frame` 此前在调用线程上
/// 直接 `write_all` 管道。OS 管道缓冲一旦写满，那一次写**永不返回**——而生产路径
/// 的调用线程就是 Tauri 命令线程（`ProcessCallDelivery::deliver` 内联调用它），
/// 于是一个装聋的 sidecar 能把宿主的一个命令线程永久吃掉，后续同 pid 的投递再排队。
/// 改造后投递只做 `try_send`，阻塞被关进该 pid 专属的写线程。
///
/// 本用例证明三件事，都只有真进程能做：
/// - 队列填满后如实报 `WouldBlock`（既不假装成功，也不静默丢帧）；
/// - **每一次** `write_frame` 都在毫秒级返回——没有任何一次把调用线程钉住；
/// - 超限帧在**入队之前**就被 `InvalidInput` 拒掉（否则 32 帧队列就是无界的宿主
///   内存口子，把「阻塞」换成「OOM」）。
#[test]
fn deaf_sidecar_backpressures_the_write_queue_without_blocking_the_caller() {
    let recorder = Arc::new(Recorder::default());
    let harness =
        Harness::new("deaf", Spec { behavior: Behavior::Deaf, exit_code: 0, ..Default::default() });
    let pid = harness.spawn_with_sink(recorder.clone());
    // 先让 sidecar 自证「我起来了，而且我确实一眼都不看 stdin」——
    // 少了这笔留痕，背压断言可能只是在测一个没起来的进程。
    harness.wait_trace_event("deaf-parked");

    // 长度门与读侧同一道上界：`MAX_FRAME_BYTES + 1` 必须落地成 InvalidInput。
    let too_big = vec![b'x'; MAX_FRAME_BYTES + 1];
    let err =
        harness.try_write_frame(pid, &too_big).expect_err("超过 MAX_FRAME_BYTES 的帧必须被拒");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "超限帧的结局应是长度门：{err}");
    assert_eq!(harness.count_trace_events("request-received"), 0, "超限帧不该到达对端");

    // 2 KiB 的帧：明显低于长度门（不撞上面那道），又足以在有限轮次里把
    // OS 管道缓冲 + 写队列一起填满（Windows 管道缓冲可能只有 4 KiB，
    // Linux 是 64 KiB，因此轮次上界按最坏情况给足）。
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {},
        "callId": "call-deaf",
        "caller": "main",
        "target": harness.plugin,
        "runtimeGeneration": 1,
        "pad": "x".repeat(2048),
    });
    let bytes = serde_json::to_vec(&frame).expect("帧应可序列化");

    let mut slowest = Duration::ZERO;
    let mut backpressure = None;
    for _ in 0..512 {
        let started = Instant::now();
        let result = harness.try_write_frame(pid, &bytes);
        slowest = slowest.max(started.elapsed());
        match result {
            Ok(()) => continue,
            Err(e) => {
                backpressure = Some(e);
                break;
            }
        }
    }

    let err =
        backpressure.expect("装聋的 sidecar 从不读 stdin，写侧迟早要填满；一次都没满就是没有背压");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::WouldBlock,
        "填满的结局必须是背压报错，而不是别的失败：{err}"
    );
    assert!(
        slowest < Duration::from_millis(250),
        "最慢的一次 write_frame 用了 {slowest:?}——投递线程被管道拖住了"
    );
    assert!(recorder.frames().is_empty(), "装聋进程不该回任何帧");
    assert!(harness.is_alive(pid), "背压只该拦住投递，不该顺手把进程判死");

    // 收尾：杀掉之后写线程从阻塞的 `write_all` 里拿到管道错误自行退出；
    // 读线程 EOF 摘掉写侧条目，启动面簿记归零。
    assert_eq!(harness.kill(pid), KillOutcome::Terminated);
    wait_until("杀掉后读线程收尾", || recorder.saw_eof(pid));
    harness.finish();
}
