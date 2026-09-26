//! 启动恢复的持久化与崩溃检测（§4.14）。
//!
//! `RecoveryEngine` 只提供内存态与 `to_json` / `from_json`；跨进程留痕、崩溃
//! 检测与原子落盘由本模块承担。所有判定只依赖标记文件与计数器，**不解析日志
//! 字符串**，与引擎自己的约束保持一致。
//!
//! ## 崩溃检测（干净退出标记 + 启动校验）
//!
//! 标记文件里有一个 `bootInFlight` 位，语义是「本次启动已开始，但尚未上报任何
//! 完成结果」：
//!
//! - `load()` 时若磁盘上的 `bootInFlight == true`，说明上一次启动没有走到
//!   「上报 success / failure」（进程崩溃、被强杀、断电），因此记一次
//!   `record_boot_failure(None)`；
//! - 上报 `success` / `failure` 时把 `bootInFlight` 写回 `false`，下一次
//!   `load()` 就不会重复计数；
//! - 标记文件本身不可解析（写一半断电）同样按「上一次未干净结束」处理——
//!   这是安全方向：多一次失败计数只是多降一级模式，而一次成功启动即可自动
//!   清零自愈。
//!
//! ## 持久化是显式可选的
//!
//! [`RecoveryStore::disabled()`]（`CommandState::new` 的默认值）完全不碰磁盘，
//! 单元测试因此保持无副作用；生产宿主在装配时传入 `app_config_dir`。
//!
//! ## 关键事件上下文（R7「持久化对称」）
//!
//! 除 `bootInFlight` 标记外，标记文件还带一份 `lastContext`：最近
//! [`tauron_recovery::RecoveryEngine::CONTEXT_CAPACITY`] 条关键事件摘要
//! （故障插件 id + 故障类型 + 计数器读数）。它让「崩溃 → 重启」之后
//! `host_recover_boot` 仍能回传**上一轮**发生了什么，而不是只剩一个阶段名。
//!
//! 三个容错约定，逐条对应用户面不变量：
//!
//! - **写失败绝不阻断启动/上报**：`load`/`save*` 的错误只进
//!   [`RecoveryStore::last_error`]，返回值仍是布尔/结构体，不 panic、不 Err；
//! - **读失败视为无 context**：`lastContext` 缺失（旧标记文件）或损坏都只让
//!   上下文为空——诊断字段坏了就把整份标记判成 `Corrupt`，等于平白多算一次
//!   启动失败，那是判定被诊断污染；
//! - **不伪造**：解析不出来就是空，既不猜也不补默认条目。
//!
//! `lastContext` 与 `engine.context` 在标记文件里各出现一次，二者**同源**
//! （都从引擎快照派生，见 [`RecoveryStore::save_json`]）：顶层那份是
//! `RecoveryStore` 自己的持久化字段，`engine` 里那份是 `RecoveryEngine`
//! 自洽 round-trip 的一部分。同源派生不会漂移。
//!
//! ## 已知边界
//!
//! - 崩溃检测要求应用**必须**在启动流程结束时上报一次（`success`）。不上报的
//!   应用每次重启都会被算作一次崩溃，2 次后进入安全模式——这是刻意的：安全
//!   模式只禁用非必需插件，壳仍可用，且一次成功上报即清零自愈。
//! - 应用**降级**（新版本的标记文件被旧版本读取）会被当作「不可解析」而多算
//!   一次失败。同理：安全方向，一次成功即自愈。
//! - 上下文是**历史**，不是当前状态：`record_boot_success` 清零计数器时**不**
//!   清它。一次健康启动之后仍能读到上一次崩溃的证据，配合时间戳判断新鲜度。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use tauron_recovery::{BootContextEntry, RecoveryEngine};

/// 标记文件 envelope 的格式版本。
const RECOVERY_STORE_VERSION: u64 = 1;

/// 标记文件名（位于宿主数据目录下）。
const RECOVERY_FILE: &str = "recovery-state.json";

/// 原子写用的临时文件名。
const RECOVERY_TMP_FILE: &str = "recovery-state.json.tmp";

/// 一次 [`RecoveryStore::load`] 的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    /// 持久化未启用：纯内存引擎。
    Disabled,
    /// 标记文件不存在：首次启动。
    Fresh,
    /// 从标记文件载入。
    Restored,
    /// 标记文件不可解析：按「上一次未干净结束」处理。
    Corrupt,
}

impl LoadSource {
    /// 线名（供宿主 UI / TS 侧展示，见 `RecoveryBootResult.loadSource`）。
    pub fn as_str(self) -> &'static str {
        match self {
            LoadSource::Disabled => "disabled",
            LoadSource::Fresh => "fresh",
            LoadSource::Restored => "restored",
            LoadSource::Corrupt => "corrupt",
        }
    }
}

/// [`RecoveryStore::load`] 的结果。
///
/// `RecoveryEngine` 未实现 `Debug`，故这里不 derive。
pub struct BootRecord {
    /// 就绪的引擎状态。
    pub engine: RecoveryEngine,
    /// 上一次启动是否未上报完成结果（标记为 `true`，或文件损坏）。
    pub previous_in_flight: bool,
    /// 本次加载来源。
    pub source: LoadSource,
}

/// 恢复标记文件的持久化载体。
///
/// 线程安全由外层 `Arc<Mutex<>>` 提供，本结构自身不做同步。
///
/// 字段是 `pub` 的：`in_flight` 是崩溃检测的状态位，`last_error` 是必须能被
/// 宿主读到的失败原因（禁止静默丢弃），`last_context` 是崩溃后要被
/// `host_recover_boot` 回传的诊断上下文。
#[derive(Debug, Clone, Default)]
pub struct RecoveryStore {
    dir: Option<PathBuf>,
    /// 本次启动是否已开始且未上报完成结果。
    pub in_flight: bool,
    /// 最近一次持久化相关的失败原因（加载期损坏或落盘失败）。
    pub last_error: Option<String>,
    /// 最近 [`tauron_recovery::RecoveryEngine::CONTEXT_CAPACITY`] 条关键事件摘要。
    ///
    /// `load` 时来自磁盘，`save*` 时来自引擎快照。空 = 没有可回传的上下文
    /// （首次启动、或标记文件里这一段读不出来）——**不伪造**。
    pub last_context: Vec<BootContextEntry>,
}

impl RecoveryStore {
    /// 持久化关闭（不碰磁盘）。`CommandState::new` 的默认值。
    pub fn disabled() -> Self {
        Self::default()
    }

    /// 在宿主数据目录下启用持久化。
    pub fn new(dir: PathBuf) -> Self {
        Self { dir: Some(dir), in_flight: false, last_error: None, last_context: Vec::new() }
    }

    /// 持久化是否启用。
    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// 宿主数据目录（`None` = 未启用）。
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// 标记文件的完整路径。
    pub fn path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(RECOVERY_FILE))
    }

    /// 读取持久化状态并准备本轮启动。
    ///
    /// 返回的引擎可以直接使用（不需要调用方再做任何补偿步骤）：
    /// - 持久化关闭 → 全新引擎，[`LoadSource::Disabled`]；
    /// - 无标记文件 → 全新引擎，[`LoadSource::Fresh`]；
    /// - 有有效标记 → 载入的引擎，[`LoadSource::Restored`]；
    /// - 标记不可解析 → 全新引擎，[`LoadSource::Corrupt`]。
    ///
    /// **两次副作用**（都在这一次调用里完成，不留给调用方）：
    /// 1. 若上一次启动未干净结束（`bootInFlight` 仍为 `true`，或标记文件损坏），
    ///    立刻 `record_boot_failure_at(None, now)` 计一次失败。把这件事留给调用方
    ///    就等于谁忘了谁就把崩溃检测关掉；
    /// 2. 把「本轮进行中」（`bootInFlight = true`）落盘。否则进程在 `load` 之后、
    ///    第一次 `save` 之前就崩掉，文件里还是上一轮的 `false`，这次崩溃检测不到。
    ///
    /// **上下文跨进程续接**：磁盘上的 `lastContext` 先播种进引擎的环形缓冲，
    /// 再记本次崩溃（若上一次未干净结束）。于是本次启动回传的上下文 =
    /// 「上一轮的历史 + 本轮识别到的那次崩溃」，且仍受环形上限约束。
    ///
    /// 干净退出的调用方负责把 `in_flight` 置回 `false` 再 `save`（见
    /// [`CommandState`] 的启动上报命令）。
    pub fn load(&mut self, required_plugins: HashSet<String>) -> BootRecord {
        let Some(dir) = self.dir.clone() else {
            return BootRecord {
                engine: RecoveryEngine::new(required_plugins),
                previous_in_flight: false,
                source: LoadSource::Disabled,
            };
        };

        let path = dir.join(RECOVERY_FILE);
        let mut load_error: Option<String> = None;
        let (mut engine, previous_in_flight, persisted_context, source) =
            match fs::read_to_string(&path) {
                Ok(text) => match parse(&text) {
                    Ok((engine, in_flight, last_context)) => {
                        (engine, in_flight, last_context, LoadSource::Restored)
                    }
                    Err(err) => {
                        load_error = Some(format!("恢复标记文件不可解析：{err}"));
                        (
                            RecoveryEngine::new(required_plugins.clone()),
                            true,
                            Vec::new(),
                            LoadSource::Corrupt,
                        )
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                    RecoveryEngine::new(required_plugins.clone()),
                    false,
                    Vec::new(),
                    LoadSource::Fresh,
                ),
                Err(e) => {
                    // 目录不可读（权限被拒）不是「上次崩溃」的证据，不能误计数。
                    load_error = Some(format!("恢复标记文件读取失败：{e}"));
                    (
                        RecoveryEngine::new(required_plugins.clone()),
                        false,
                        Vec::new(),
                        LoadSource::Fresh,
                    )
                }
            };

        // 先播种上一轮的历史，再记本次失败——顺序反了本轮那条就会把历史挤掉。
        engine.set_context(persisted_context);

        if previous_in_flight {
            engine.record_boot_failure_at(None, now_ms());
        }

        self.in_flight = true;
        self.last_context = engine.context().to_vec();
        self.save(&engine);

        // `save` 的成功分支会清空 `last_error`，所以错误必须在落盘之后记录，
        // 否则「上一次标记损坏」这条证据会被自己的成功写入覆盖掉。
        if let Some(err) = load_error {
            self.last_error = Some(err);
        }

        BootRecord { engine, previous_in_flight, source }
    }

    /// 原子落盘（先写临时文件，再 `rename` 覆盖）。
    ///
    /// # 返回
    ///
    /// `false` 表示写入失败，原因记入 [`Self::last_error`]；此时本轮判定在内存里
    /// 仍然生效，只是不跨进程。持久化关闭时返回 `true`（无操作不算失败）。
    pub fn save(&mut self, engine: &RecoveryEngine) -> bool {
        self.save_json(&engine.to_json())
    }

    /// 用**预先取好的序列化快照**落盘。
    ///
    /// 适配层用它避免「持着 recovery_store 锁去取 recovery 锁」：先在 recovery
    /// 锁内取出快照并释放，再持 recovery_store 锁写入。锁顺序与
    /// `recovery_boot_payload`（recovery → recovery_store）保持一致，否则
    /// 并发调用会构成 ABBA 死锁。
    ///
    /// 快照里的 `context` 会被提炼成 [`Self::last_context`]（与 `bootInFlight`
    /// 同一次原子写落盘）。提炼**读失败即视为无 context**：诊断字段解析不出来
    /// 只让上下文为空，绝不让落盘失败——那是「系统通知/诊断失败不阻断存储」的
    /// 同一个不变量。
    pub fn save_json(&mut self, engine: &serde_json::Value) -> bool {
        self.last_context = context_from_engine_json(engine);

        let Some(dir) = self.dir.clone() else {
            return true;
        };

        let payload = serde_json::json!({
            "version": RECOVERY_STORE_VERSION,
            "bootInFlight": self.in_flight,
            // 顶层一份显式的 `lastContext`：`RecoveryStore` 自己的持久化字段
            // （门禁「RecoveryStore 必须持久化 last_context」读的就是它）。
            "lastContext": self.last_context,
            "engine": engine,
        });
        // payload 由 json! 构造，序列化不可能失败。
        let text = serde_json::to_string(&payload).expect("json! 构造的 Value 序列化不会失败");

        let tmp = dir.join(RECOVERY_TMP_FILE);
        let target = dir.join(RECOVERY_FILE);
        let result = fs::create_dir_all(&dir)
            .and_then(|_| fs::write(&tmp, text))
            .and_then(|_| fs::rename(&tmp, &target));

        match result {
            Ok(()) => {
                self.last_error = None;
                true
            }
            Err(e) => {
                // 清掉临时文件，避免下次 rename 时留下半截内容。
                let _ = fs::remove_file(&tmp);
                self.last_error = Some(format!("恢复标记落盘失败：{e}"));
                false
            }
        }
    }
}

/// 解析标记文件 envelope。
///
/// 返回 `(引擎, bootInFlight, lastContext)`。
fn parse(text: &str) -> std::result::Result<(RecoveryEngine, bool, Vec<BootContextEntry>), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let version = v
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "缺少 version 字段".to_string())?;
    if version != RECOVERY_STORE_VERSION {
        return Err(format!("不支持的标记版本：{version}"));
    }
    let engine = v.get("engine").ok_or_else(|| "缺少 engine 字段".to_string())?;
    let engine = RecoveryEngine::from_json(engine).map_err(|e| e.to_string())?;
    // 字段缺失时按「上一次未干净结束」处理（安全方向）。
    let in_flight = v.get("bootInFlight").and_then(serde_json::Value::as_bool).unwrap_or(true);
    // `lastContext` 缺失/损坏 → 视为无 context，**不**把整份标记判为损坏。
    // 顶层没有时退回引擎快照里那份（两者同源，兼容只写了其中一份的标记）。
    let last_context = v
        .get("lastContext")
        .and_then(|c| serde_json::from_value::<Vec<BootContextEntry>>(c.clone()).ok())
        .unwrap_or_else(|| engine.context().to_vec());
    Ok((engine, in_flight, last_context))
}

/// 从引擎快照里提炼上下文；缺失或损坏都返回空（诊断字段不做「猜」）。
fn context_from_engine_json(engine: &serde_json::Value) -> Vec<BootContextEntry> {
    engine
        .get("context")
        .and_then(|c| serde_json::from_value::<Vec<BootContextEntry>>(c.clone()).ok())
        .unwrap_or_default()
}

/// 当前时间（epoch 毫秒）。时钟异常（早于 UNIX_EPOCH）记 `0`，不 panic。
///
/// `pub(crate)`：适配层的 `cmd_recover_report` 也要给关键事件摘要盖同一个时钟，
/// 两处各写一份 SystemTime 逻辑迟早会分叉。
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauron_recovery::{BootPhase, PluginState};

    fn store_in(t: &tempfile::TempDir) -> RecoveryStore {
        RecoveryStore::new(t.path().to_path_buf())
    }

    #[test]
    fn disabled_store_never_touches_disk() {
        let mut store = RecoveryStore::disabled();
        let record = store.load(HashSet::new());
        assert_eq!(record.source, LoadSource::Disabled);
        assert!(!record.previous_in_flight);
        assert_eq!(record.engine.phase(), BootPhase::Normal);
        assert!(store.save(&record.engine));
        assert!(store.last_error.is_none());
        assert!(store.path().is_none());
    }

    #[test]
    fn fresh_boot_then_restored() {
        let t = tempfile::tempdir().unwrap();
        let mut store = store_in(&t);

        let r1 = store.load(HashSet::new());
        assert_eq!(r1.source, LoadSource::Fresh);
        assert!(!r1.previous_in_flight);
        // 干净退出：调用方负责把标记写回 false 再落盘。
        store.in_flight = false;
        assert!(store.save(&r1.engine));

        // 干净退出后重开：标记为 false，不应重复计数。
        let mut store2 = store_in(&t);
        let r2 = store2.load(HashSet::new());
        assert_eq!(r2.source, LoadSource::Restored);
        assert!(!r2.previous_in_flight);
    }

    #[test]
    fn in_flight_flag_is_persisted_by_load_itself() {
        let t = tempfile::tempdir().unwrap();
        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert!(store.in_flight);
        assert!(store.save(&r.engine));

        // 文件里必须已经写着 bootInFlight=true，否则本轮崩溃检测不到。
        let text = fs::read_to_string(store.path().unwrap()).unwrap();
        assert!(text.contains("\"bootInFlight\":true"), "落盘内容：{text}");
    }

    #[test]
    fn load_persists_in_flight_without_a_save() {
        // 崩溃检测的关键：如果 load 之后、第一次 save 之前进程就死了，文件里
        // 仍是上一轮的 false，崩溃就检测不到。所以 load 必须自己落盘。
        let t = tempfile::tempdir().unwrap();

        {
            let mut store = store_in(&t);
            let r = store.load(HashSet::new());
            assert_eq!(r.source, LoadSource::Fresh);
            // 不调用 save，直接「进程死亡」。
        }

        let text =
            fs::read_to_string(RecoveryStore::new(t.path().to_path_buf()).path().unwrap()).unwrap();
        assert!(text.contains("\"bootInFlight\":true"), "load 自己没落盘：{text}");

        // 下一次启动必须能识别出这次崩溃。
        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Restored);
        assert!(r.previous_in_flight);
        assert_eq!(r.engine.counter().consecutive_failures, 1);
    }

    #[test]
    fn crash_marker_is_detected_and_counted_on_next_boot() {
        let t = tempfile::tempdir().unwrap();

        // 第一次启动：崩溃了（标记仍是 in_flight=true）。
        {
            let mut store = store_in(&t);
            let r = store.load(HashSet::new());
            assert!(store.save(&r.engine));
        }

        // 第二次启动：必须识别出上一次未干净结束，并且**计数由 load 自己完成**
        // ——调用方忘记这一步不能悄悄把崩溃检测关掉。
        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Restored);
        assert!(r.previous_in_flight, "上一次崩溃必须被识别");
        assert_eq!(r.engine.counter().consecutive_failures, 1);
    }

    #[test]
    fn clean_exit_stops_repeat_counting() {
        let t = tempfile::tempdir().unwrap();

        // 第 1 次：首次启动后崩溃（标记 true）。
        {
            let mut store = store_in(&t);
            let r = store.load(HashSet::new());
            assert!(!r.previous_in_flight, "首次启动没有前一次");
            assert!(store.save(&r.engine));
        }
        // 第 2 次：识别出上一次崩溃 → 计数 1；这次也没上报。
        {
            let mut store = store_in(&t);
            let r = store.load(HashSet::new());
            assert!(r.previous_in_flight);
            assert_eq!(r.engine.counter().consecutive_failures, 1);
            assert!(store.save(&r.engine));
        }
        // 第 3 次：计数 2 → 安全模式；这次上报成功 → 计数清零、标记清零。
        {
            let mut store = store_in(&t);
            let mut r = store.load(HashSet::new());
            assert!(r.previous_in_flight);
            assert_eq!(r.engine.counter().consecutive_failures, 2);
            assert_eq!(r.engine.phase(), BootPhase::Safemode);
            r.engine.record_boot_success();
            store.in_flight = false;
            assert!(store.save(&r.engine));
        }
        // 第 4 次：不再重复计数，也不会进安全模式。
        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Restored);
        assert!(!r.previous_in_flight);
        assert_eq!(r.engine.counter().consecutive_failures, 0);
        assert_eq!(r.engine.phase(), BootPhase::Normal);
    }

    #[test]
    fn two_crashes_reach_safemode_across_processes() {
        let t = tempfile::tempdir().unwrap();

        // 每次启动后都崩溃（标记仍是 in_flight=true），计数由 load 自己完成。
        for _ in 0..2 {
            let mut store = store_in(&t);
            let r = store.load(HashSet::new());
            assert!(store.save(&r.engine));
        }

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        // 两次崩溃已经让阶段推进到安全模式，且跨进程留痕。
        assert_eq!(r.engine.counter().consecutive_failures, 2);
        assert_eq!(r.engine.phase(), BootPhase::Safemode);
    }

    #[test]
    fn corrupt_marker_counts_as_unclean_exit_and_heals() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(RECOVERY_FILE), "{ this is not json").unwrap();

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Corrupt);
        assert!(r.previous_in_flight);
        assert!(store.last_error.is_some(), "损坏原因必须可见，不静默");
        // 损坏等同未干净结束：load 自己就计了一次失败。
        assert_eq!(r.engine.counter().consecutive_failures, 1);

        // load 已经把标记重写为合法内容（自愈），下一次读取不再是 Corrupt。
        let mut store2 = store_in(&t);
        let r2 = store2.load(HashSet::new());
        assert_eq!(r2.source, LoadSource::Restored);
        assert!(r2.previous_in_flight);

        // 这次干净退出后，计数不再累积。
        store2.in_flight = false;
        assert!(store2.save(&r2.engine));
        let mut store3 = store_in(&t);
        let r3 = store3.load(HashSet::new());
        assert_eq!(r3.source, LoadSource::Restored);
        assert!(!r3.previous_in_flight);
    }

    #[test]
    fn unsupported_version_is_corrupt() {
        let t = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "version": 99,
            "bootInFlight": false,
            "engine": RecoveryEngine::new(HashSet::new()).to_json(),
        });
        fs::write(t.path().join(RECOVERY_FILE), serde_json::to_string(&payload).unwrap()).unwrap();

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Corrupt);
    }

    #[test]
    fn missing_in_flight_flag_means_in_flight() {
        let t = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "version": RECOVERY_STORE_VERSION,
            "engine": RecoveryEngine::new(HashSet::new()).to_json(),
        });
        fs::write(t.path().join(RECOVERY_FILE), serde_json::to_string(&payload).unwrap()).unwrap();

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert!(r.previous_in_flight);
    }

    #[test]
    fn counter_and_phase_round_trip_through_disk() {
        let t = tempfile::tempdir().unwrap();

        // 两次失败 → 安全模式，状态必须跨进程留痕。
        for _ in 0..2 {
            let mut store = store_in(&t);
            let mut r = store.load(HashSet::new());
            r.engine.record_boot_failure(None);
            store.in_flight = false;
            assert!(store.save(&r.engine));
        }

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.engine.counter().consecutive_failures, 2);
        assert_eq!(r.engine.phase(), BootPhase::Safemode);
    }

    #[test]
    fn required_plugins_and_plugin_states_survive() {
        let t = tempfile::tempdir().unwrap();
        let mut required = HashSet::new();
        required.insert("p.core".to_string());

        {
            let mut store = store_in(&t);
            let mut r = store.load(required.clone());
            r.engine.set_required_plugins(required.clone());
            r.engine.register_plugin("p.core");
            r.engine.register_plugin("p.audio");
            assert!(store.save(&r.engine));
        }

        let mut store = store_in(&t);
        let r = store.load(required);
        assert_eq!(r.source, LoadSource::Restored);
        assert_eq!(r.engine.plugin_state("p.core"), Some(PluginState::Enabled));
        assert!(r.engine.plugin_state("p.audio").is_some());
    }

    #[test]
    fn save_reports_failure_without_panicking() {
        // 用一个「目录」占住目标路径，让 rename 覆盖失败。
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(RECOVERY_FILE)).unwrap();

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert!(!store.save(&r.engine));
        assert!(store.last_error.as_ref().unwrap().contains("落盘失败"));
    }

    // ── 关键事件上下文（R7：崩溃后留诊断上下文）────────────────────

    #[test]
    fn last_context_round_trips_through_disk() {
        let t = tempfile::tempdir().unwrap();

        // 第 1 轮：上报一次失败（带真实时间戳），然后干净退出。
        {
            let mut store = store_in(&t);
            let mut r = store.load(HashSet::new());
            r.engine.record_boot_failure_at(Some("p.audio"), 111);
            store.in_flight = false;
            assert!(store.save(&r.engine));
            assert_eq!(store.last_context.len(), 1, "save 后 store 自己也有上下文");
        }

        // 第 2 轮：上一轮干净退出，但上下文必须被读回来（诊断要跨进程）。
        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Restored);
        assert!(!r.previous_in_flight);
        assert_eq!(store.last_context.len(), 1, "上一轮的上下文必须跨进程留存");
        let c = &store.last_context[0];
        assert_eq!(c.ts, 111, "时间戳原样留存");
        assert_eq!(c.plugin_id.as_deref(), Some("p.audio"));
        assert_eq!(c.failure_kind(), "boot-failure");
        // 引擎那一侧也被播种了（后续追加不会把历史挤掉）。
        assert_eq!(r.engine.context(), store.last_context.as_slice());
    }

    #[test]
    fn crash_on_top_of_prior_context_appends_and_keeps_history() {
        let t = tempfile::tempdir().unwrap();

        // 第 1 轮：一次带插件归因的失败，然后**崩溃**（不写回 in_flight）。
        {
            let mut store = store_in(&t);
            let mut r = store.load(HashSet::new());
            r.engine.record_boot_failure_at(Some("p.audio"), 111);
            assert!(store.save(&r.engine));
        }

        // 第 2 轮：load 自己识别出上一次未干净结束 → 追加一条崩溃摘要，
        // 且**保留**上一轮那条历史。
        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert!(r.previous_in_flight, "上一次崩溃必须被识别");
        assert_eq!(r.engine.counter().consecutive_failures, 2, "上一轮那次失败 + 本轮识别到的崩溃");
        assert_eq!(store.last_context.len(), 2, "历史 + 本次崩溃各一条");
        assert_eq!(store.last_context[0].ts, 111, "最旧的是上一轮那条");
        assert_eq!(store.last_context[0].plugin_id.as_deref(), Some("p.audio"));
        let crash = &store.last_context[1];
        assert_eq!(crash.plugin_id, None, "崩溃未能归因到具体插件");
        assert_eq!(crash.failure_kind(), "boot-failure");
        assert!(crash.ts > 0, "崩溃摘要应带真实时间戳");

        // 标记文件里 `lastContext` 与 engine.context 同源。
        let text = fs::read_to_string(store.path().unwrap()).unwrap();
        assert!(text.contains("\"lastContext\""), "落盘内容：{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["lastContext"].as_array().unwrap().len(), 2);
        assert_eq!(v["lastContext"], v["engine"]["context"], "两份必须同源");
    }

    #[test]
    fn corrupt_last_context_is_treated_as_empty_not_as_a_corrupt_marker() {
        // 诊断字段坏了不能把整份标记判成损坏——那会平白多算一次启动失败。
        let t = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "version": RECOVERY_STORE_VERSION,
            "bootInFlight": false,
            "lastContext": "这不是数组",
            "engine": RecoveryEngine::new(HashSet::new()).to_json(),
        });
        fs::write(t.path().join(RECOVERY_FILE), serde_json::to_string(&payload).unwrap()).unwrap();

        let mut store = store_in(&t);
        let r = store.load(HashSet::new());
        assert_eq!(r.source, LoadSource::Restored, "文件本身仍可解析");
        assert!(!r.previous_in_flight);
        // 引擎快照里也没有 context → 视为无 context，不伪造。
        assert!(store.last_context.is_empty());
        assert_eq!(r.engine.counter().consecutive_failures, 0, "不该多计一次失败");
    }

    #[test]
    fn missing_last_context_falls_back_to_the_engine_snapshot() {
        // 只写了 engine.context 的标记文件（兼容「顶层字段未来才加」的形态）。
        let mut engine = RecoveryEngine::new(HashSet::new());
        engine.record_boot_failure_at(Some("p.x"), 7);
        let t = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "version": RECOVERY_STORE_VERSION,
            "bootInFlight": false,
            "engine": engine.to_json(),
        });
        fs::write(t.path().join(RECOVERY_FILE), serde_json::to_string(&payload).unwrap()).unwrap();

        let mut store = store_in(&t);
        store.load(HashSet::new());
        assert_eq!(store.last_context.len(), 1);
        assert_eq!(store.last_context[0].ts, 7);
    }

    #[test]
    fn context_is_capped_by_the_engine_ring_across_restarts() {
        let t = tempfile::tempdir().unwrap();
        let cap = tauron_recovery::RecoveryEngine::CONTEXT_CAPACITY;

        for i in 0..(cap as u64 + 4) {
            let mut store = store_in(&t);
            let r = store.load(HashSet::new());
            let mut engine = r.engine;
            engine.record_boot_failure_at(Some("p.x"), i + 1);
            store.in_flight = false;
            assert!(store.save(&engine));
        }

        let mut store = store_in(&t);
        store.load(HashSet::new());
        assert_eq!(store.last_context.len(), cap, "跨进程累积也必须受环形上限约束");
        assert_eq!(store.last_context[cap - 1].ts, cap as u64 + 4, "保留最近 N 条");
    }

    #[test]
    fn failing_save_still_exposes_context_and_never_errors() {
        // 目标路径被目录占住 → rename 失败。上下文必须仍在内存里可见，
        // 且 `save` 只是返回 false（启动/上报路径不因此失败）。
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(RECOVERY_FILE)).unwrap();

        let mut store = store_in(&t);
        let mut r = store.load(HashSet::new());
        r.engine.record_boot_failure_at(Some("p.audio"), 5);
        assert!(!store.save(&r.engine), "写失败如实返回 false");
        assert!(store.last_error.as_ref().unwrap().contains("落盘失败"));
        assert_eq!(store.last_context.len(), 1, "写失败不影响内存里的上下文");
        assert_eq!(store.last_context[0].plugin_id.as_deref(), Some("p.audio"));
    }

    #[test]
    fn disabled_store_has_no_context_and_never_panics() {
        let mut store = RecoveryStore::disabled();
        let mut r = store.load(HashSet::new());
        r.engine.record_boot_failure_at(Some("p.audio"), 3);
        assert!(store.save(&r.engine));
        assert_eq!(store.last_context.len(), 1);
        assert!(store.path().is_none());
    }

    #[test]
    fn disabled_plugins_do_not_include_removed_one() {
        let mut e = RecoveryEngine::new(HashSet::new());
        e.set_required_plugins(HashSet::new());
        e.register_plugin("p.audio");
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        assert_eq!(e.phase(), BootPhase::Safemode);
        assert_eq!(e.disabled_plugins().len(), 1);

        assert!(e.remove_plugin("p.audio"));
        assert_eq!(e.disabled_plugins().len(), 0);
        assert!(e.plugin_state("p.audio").is_none());
    }
}
