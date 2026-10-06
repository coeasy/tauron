// 升级 journal 的**启动对账腿**（审计 V9 N-03；V7 Batch 3「restart 后 resume/recover」
// 的读侧半成品收口）。
//
// 写侧（[`crate::UpgradeRunner`]）早就每一步落盘 journal；缺的是**读侧**：没有任何
// 生产启动路径在 boot 时读回这些 journal、把「上次升级中断于哪一步」如实报告出来。
// 本模块补的正是这条读腿，并严格遵守本仓的诚实姿态：
//
// - **boot 只补发事实、不改状态**：[`UpgradeReconciler::scan`] 只读 journal + 目录，
//   唯一的写盘副作用是把**撕裂/解析失败**的 journal 隔离成 `.corrupt-<ts>`（沿用
//   `DurableEnvelope` / 适配层 `quarantine_corrupt` 的姿态——坏数据不盲动、也不阻塞
//   启动，见 [`crate::UpgradeJournal::load`] 的失败面）。绝不自动回滚、绝不自动提交。
// - **恢复动作必须显式**：`resume` / `restore` 只在调用方明确要求时执行（隐式副作用
//   会和「用户重启后想自己决定」的预期冲突）。二者都走真实文件效果 + 逐文件哈希核对，
//   且 `restore` 复用 [`crate::UpgradeRunner`] 回滚那**同一份** `restore_backup_tree`，
//   绝不为恢复腿写第二套字节一致性判定。
//
// 处置分派（按 journal `currentState` + commit 标记 + 磁盘实况三者对账）：
// - `Committed`（commit 标记落盘）→ 升级真完成，boot 无事可做（staging 已在交换时 rename 走）。
// - `NeedsDecision`（交换已发生但未提交：Swapped / HealthChecked，或 Failed 后 previous 仍在）
//   → current 已是新树、previous 是旧树；提供 `resume`（补记 commit）或 `restore`（回滚到旧树）。
// - `StaleStaging`（未交换：Checking..Extracted，previous 不存在）→ current 仍是旧树、
//   staging 里是本操作残骸；`restore` 在这里 = 清 staging、把日志记成 RolledBack（回 Idle）。
// - `Settled`（Failed / RolledBack / Completed 且无悬挂交换）→ 终态，字节已自洽，不动。
// - `Quarantined`（journal 撕裂/解析失败）→ 隔离，不解析、不据此改任何树。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::upgrade::{
    io_fail, remove_tree_if_exists, restore_backup_tree, tree_hashes, BACKUP_TREE_DIR, CURRENT_DIR,
    FAILED_DIR_PREFIX, JOURNAL_FILE, PREVIOUS_DIR, STAGING_DIR,
};
use crate::{DistributeError, DistributeResult, UpgradeJournal, UpgradeState};

/// 对账所需的安装根布局路径（与 [`crate::UpgradeOptions`] 的 `install_dir` / `backup_dir`
/// 同一建模：staging/current/previous 三段式 + 每操作 `backup/<op>/{journal.json,tree}`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPaths {
    /// 安装根目录（内含 `current` / `previous` / `staging` / `failed-*`）。
    pub install_dir: PathBuf,
    /// 备份根目录（内含按操作隔离的 `<op>/{journal.json,tree}`）。
    pub backup_dir: PathBuf,
}

impl RecoveryPaths {
    fn journal_path(&self, operation_id: &str) -> PathBuf {
        self.backup_dir.join(operation_id).join(JOURNAL_FILE)
    }

    fn backup_tree(&self, operation_id: &str) -> PathBuf {
        self.backup_dir.join(operation_id).join(BACKUP_TREE_DIR)
    }

    fn staging_dir(&self, operation_id: &str) -> PathBuf {
        self.install_dir.join(STAGING_DIR).join(operation_id)
    }

    fn previous_dir(&self) -> PathBuf {
        self.install_dir.join(PREVIOUS_DIR)
    }

    fn install_current_dir(&self) -> PathBuf {
        self.install_dir.join(CURRENT_DIR)
    }
}

/// 每个中断操作的处置结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryDisposition {
    /// 已提交：升级真完成，无需对账。
    Committed,
    /// 交换已发生但未提交：`resume`（补记提交）或 `restore`（回滚旧树）二选一。
    NeedsDecision,
    /// 未交换、仅 staging 残骸：`restore` 清残骸回 Idle（current 未被动过）。
    StaleStaging,
    /// 终态且字节自洽（Failed / RolledBack / Completed，无悬挂交换）：不动。
    Settled,
    /// journal 撕裂/解析失败：已隔离成 `.corrupt-<ts>`，不据此改树。
    Quarantined,
}

/// 一个中断升级操作的对账结果（camelCase 线形，可直接进宿主读数）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryFinding {
    /// 操作 id（= 备份/日志目录名）。
    pub operation_id: String,
    /// journal 磁盘路径（撕裂隔离后指向 `.corrupt-<ts>`）。
    pub journal_path: String,
    /// 升级前版本（来自 journal `old_version`，可为 `None`）。
    pub old_version: Option<String>,
    /// 升级目标版本。
    pub new_version: String,
    /// journal 读到的最后状态——「上次中断于 X 步」的事实来源。
    pub interrupted_at: UpgradeState,
    /// 处置结论。
    pub disposition: RecoveryDisposition,
    /// 是否可 `resume`（补记提交）——仅 `NeedsDecision` 为真。
    pub resume_available: bool,
    /// 是否可 `restore`（回滚或清残骸）——`NeedsDecision` / `StaleStaging` 为真。
    pub restore_available: bool,
    /// 人类可读的处置说明（供宿主读数直接展示，不含判定逻辑）。
    pub detail: String,
}

/// boot 时一次全量对账的汇总报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    /// 扫描到的含 journal 的操作目录数。
    pub scanned: usize,
    /// 已提交（无需动作）数。
    pub committed: usize,
    /// 需显式决策（交换未提交）数。
    pub needs_decision: usize,
    /// staging 残骸（未交换）数。
    pub stale_staging: usize,
    /// 撕裂隔离数。
    pub quarantined: usize,
    /// 终态自洽数。
    pub settled: usize,
    /// 逐操作明细（按 operation_id 升序，读数稳定可测）。
    pub findings: Vec<RecoveryFinding>,
}

impl ReconcileReport {
    /// 是否存在「需要人来收尾」的挂起项（需决策 / 残骸 / 撕裂）。
    ///
    /// 这是宿主 boot 之后判断「要不要提示用户上次升级没收尾」的**唯一判据**——
    /// `committed` / `settled` 不算挂起（那是干净的历史记录）。
    pub fn has_pending(&self) -> bool {
        self.needs_decision > 0 || self.stale_staging > 0 || self.quarantined > 0
    }

    fn tally_disposition(&mut self, disposition: RecoveryDisposition) {
        match disposition {
            RecoveryDisposition::Committed => self.committed += 1,
            RecoveryDisposition::NeedsDecision => self.needs_decision += 1,
            RecoveryDisposition::StaleStaging => self.stale_staging += 1,
            RecoveryDisposition::Settled => self.settled += 1,
            RecoveryDisposition::Quarantined => self.quarantined += 1,
        }
    }
}

/// 升级 journal 启动对账器。
pub struct UpgradeReconciler {
    paths: RecoveryPaths,
}

impl UpgradeReconciler {
    /// 以对账路径创建。
    pub fn new(paths: RecoveryPaths) -> Self {
        Self { paths }
    }

    /// boot 扫描：**只读**每个 `backup/<op>/journal.json` 并分派处置。
    ///
    /// 唯一的写盘副作用是把解析失败的 journal 隔离成 `.corrupt-<ts>`（不解析、不据此
    /// 改任何安装树）；其余状态一律「补发事实、不动状态」。`backup_dir` 不存在视为
    /// 全新安装（无历史操作可对账），返回空报告而非报错。
    pub fn scan(&self) -> DistributeResult<ReconcileReport> {
        let mut report = ReconcileReport {
            scanned: 0,
            committed: 0,
            needs_decision: 0,
            stale_staging: 0,
            quarantined: 0,
            settled: 0,
            findings: Vec::new(),
        };
        let entries = match fs::read_dir(&self.paths.backup_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(e) => {
                return Err(io_fail(format!(
                    "读取备份根目录 `{}` 失败：{e}",
                    self.paths.backup_dir.display()
                )))
            }
        };
        let mut operations: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                io_fail(format!("读取备份目录项（`{}`）失败：{e}", self.paths.backup_dir.display()))
            })?;
            let dir = entry.path();
            if dir.is_dir() && dir.join(JOURNAL_FILE).is_file() {
                operations.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        operations.sort();
        for operation_id in operations {
            let finding = self.classify(&operation_id);
            report.scanned += 1;
            report.tally_disposition(finding.disposition);
            report.findings.push(finding);
        }
        Ok(report)
    }

    /// 显式 `resume`：把「交换已发生但未提交」的操作补记为已提交。
    ///
    /// 前提（缺一不可，全部失败即硬拒、零改动）：journal 读得出、处置为 `NeedsDecision`、
    /// `install/previous` 仍在（证明交换真发生过、旧树还可回退）、`install/current` 非空
    /// （新树真在里面）。满足后只写 commit 标记 + 复读验证——这是把**已经物理落地**的
    /// 交换如实记成完成，不新增任何文件移动；下次 boot 即看到干净的 `Committed`。
    pub fn resume(&self, operation_id: &str) -> DistributeResult<RecoveryFinding> {
        let journal_path = self.paths.journal_path(operation_id);
        let mut journal = UpgradeJournal::load(&journal_path)?;
        let disposition = self.disposition_of(operation_id, &journal);
        if disposition != RecoveryDisposition::NeedsDecision {
            return Err(DistributeError::RollbackFailed(format!(
                "操作 `{operation_id}` 当前处置为 {disposition:?}，不是未提交的交换，拒绝 resume"
            )));
        }
        if !self.paths.previous_dir().is_dir() {
            return Err(DistributeError::RollbackFailed(format!(
                "previous 目录缺失（`{}`），无从确认交换真发生，拒绝补记提交",
                self.paths.previous_dir().display()
            )));
        }
        let live = tree_hashes(&self.paths.install_current_dir())?;
        if live.is_empty() {
            return Err(DistributeError::InstallRootMissing {
                path: format!(
                    "{}（current 树为空，resume 会提交一棵空树）",
                    self.paths.install_current_dir().display()
                ),
            });
        }
        journal.states.push(UpgradeState::Committed);
        journal.current_state = UpgradeState::Committed;
        journal.commit_marker = true;
        journal.error = None;
        journal.save(&journal_path)?;
        let reloaded = UpgradeJournal::load(&journal_path)?;
        if !reloaded.commit_marker || reloaded.current_state != UpgradeState::Committed {
            return Err(DistributeError::Journal(
                "resume 写 commit 标记后复读缺失，拒绝宣告成功".into(),
            ));
        }
        Ok(self.finding_from(operation_id, &reloaded, RecoveryDisposition::Committed))
    }

    /// 显式 `restore`：把中断的操作恢复到已知良好态。
    ///
    /// - `NeedsDecision`（交换未提交）：从 `backup/<op>/tree` **真实回滚**旧树——复用
    ///   [`crate::UpgradeRunner`] 回滚那同一份 `restore_backup_tree`（复制 → 逐文件哈希
    ///   核对 → 原子换入 → 再核对），随后把日志记成 `RolledBack`，并清本操作 staging 残骸。
    /// - `StaleStaging`（未交换）：current 从没被动过，只清 staging 残骸、把日志记成
    ///   `RolledBack`（回 Idle）——绝不去移一棵本就没换出的 current。
    ///
    /// 其余处置一律硬拒（没有东西要恢复，假装恢复就是撒谎）。
    pub fn restore(&self, operation_id: &str) -> DistributeResult<RecoveryFinding> {
        let journal_path = self.paths.journal_path(operation_id);
        let mut journal = UpgradeJournal::load(&journal_path)?;
        let disposition = self.disposition_of(operation_id, &journal);
        match disposition {
            RecoveryDisposition::NeedsDecision => {
                let backup_tree = self.paths.backup_tree(operation_id);
                if !backup_tree.is_dir() {
                    return Err(DistributeError::RollbackFailed(format!(
                        "找到未提交的交换但备份树 `{}` 缺失，无法回滚",
                        backup_tree.display()
                    )));
                }
                restore_backup_tree(&self.paths.install_dir, &backup_tree, operation_id)?;
                let staging = self.paths.staging_dir(operation_id);
                remove_tree_if_exists(&staging).map_err(|e| {
                    DistributeError::RollbackFailed(format!(
                        "回滚已完成且哈希核对通过，但清理 staging `{}` 失败：{e}",
                        staging.display()
                    ))
                })?;
            }
            RecoveryDisposition::StaleStaging => {
                let staging = self.paths.staging_dir(operation_id);
                remove_tree_if_exists(&staging).map_err(|e| {
                    DistributeError::RollbackFailed(format!(
                        "清理 staging 残骸 `{}` 失败：{e}",
                        staging.display()
                    ))
                })?;
                let quarantine =
                    self.paths.install_dir.join(format!("{FAILED_DIR_PREFIX}{operation_id}"));
                remove_tree_if_exists(&quarantine).map_err(|e| {
                    io_fail(format!("清理隔离残骸 `{}` 失败：{e}", quarantine.display()))
                })?;
            }
            other => {
                return Err(DistributeError::RollbackFailed(format!(
                    "操作 `{operation_id}` 当前处置为 {other:?}，没有需要恢复的中断态，拒绝 restore"
                )));
            }
        }
        journal.states.push(UpgradeState::RolledBack);
        journal.current_state = UpgradeState::RolledBack;
        journal.commit_marker = false;
        journal.save(&journal_path)?;
        let reloaded = UpgradeJournal::load(&journal_path)?;
        Ok(self.finding_from(operation_id, &reloaded, RecoveryDisposition::Settled))
    }

    // ── 内部 ──────────────────────────────────────────────────────────

    /// 读 journal 并按其处置（撕裂 → 隔离后返回 `Quarantined` finding）。
    fn classify(&self, operation_id: &str) -> RecoveryFinding {
        let journal_path = self.paths.journal_path(operation_id);
        match UpgradeJournal::load(&journal_path) {
            Ok(journal) => {
                let disposition = self.disposition_of(operation_id, &journal);
                self.finding_from(operation_id, &journal, disposition)
            }
            Err(error) => {
                // 撕裂/解析失败：隔离，不盲动。隔离失败也如实上报（留在报告里，
                // 但不据此对安装树做任何动作）。
                let quarantined = quarantine_corrupt(&journal_path);
                let shown_path = quarantined.unwrap_or_else(|| journal_path.clone());
                RecoveryFinding {
                    operation_id: operation_id.to_string(),
                    journal_path: shown_path.display().to_string(),
                    old_version: None,
                    new_version: String::new(),
                    interrupted_at: UpgradeState::Checking,
                    disposition: RecoveryDisposition::Quarantined,
                    resume_available: false,
                    restore_available: false,
                    detail: format!(
                        "journal 解析失败，已隔离到 `{}`：{error}；不据此改动任何安装树",
                        shown_path.display()
                    ),
                }
            }
        }
    }

    /// 从 journal + 磁盘实况推导处置。判定只用 `current_state` / commit 标记 /
    /// previous 是否存在，绝不猜测。
    fn disposition_of(&self, _operation_id: &str, journal: &UpgradeJournal) -> RecoveryDisposition {
        if journal.commit_marker && journal.current_state == UpgradeState::Committed {
            return RecoveryDisposition::Committed;
        }
        let swapped = self.paths.previous_dir().is_dir();
        match journal.current_state {
            // 交换里程碑之后、未提交：current 已是新树、previous 是旧树。
            UpgradeState::Swapped | UpgradeState::HealthChecked => {
                RecoveryDisposition::NeedsDecision
            }
            // 交换之前：current 未动，最多是 staging 残骸。
            UpgradeState::Checking
            | UpgradeState::Downloaded
            | UpgradeState::Verified
            | UpgradeState::BackedUp
            | UpgradeState::Extracted => RecoveryDisposition::StaleStaging,
            // 失败/回滚/完成：看磁盘是否留下悬挂交换（previous 在但没提交）来定。
            UpgradeState::Failed => {
                if swapped {
                    RecoveryDisposition::NeedsDecision
                } else {
                    RecoveryDisposition::Settled
                }
            }
            UpgradeState::RolledBack | UpgradeState::Completed => RecoveryDisposition::Settled,
            // 任何瞬态（不该落 journal，防御性归为需决策/残骸看 previous）。
            _ => {
                if swapped {
                    RecoveryDisposition::NeedsDecision
                } else {
                    RecoveryDisposition::StaleStaging
                }
            }
        }
    }

    /// 组装一条 finding（detail 用词表，不含判定逻辑）。
    fn finding_from(
        &self,
        operation_id: &str,
        journal: &UpgradeJournal,
        disposition: RecoveryDisposition,
    ) -> RecoveryFinding {
        let (resume_available, restore_available, detail) = match disposition {
            RecoveryDisposition::Committed => (
                false,
                false,
                format!("上次升级已提交到 {}，无需对账", journal.new_version),
            ),
            RecoveryDisposition::NeedsDecision => (
                true,
                true,
                format!(
                    "上次升级中断于 {}（{} → {} 已交换但未提交）：可 resume 补记提交，或 restore 回滚旧版本",
                    journal.current_state,
                    journal.old_version.as_deref().unwrap_or("unknown"),
                    journal.new_version,
                ),
            ),
            RecoveryDisposition::StaleStaging => (
                false,
                true,
                format!(
                    "上次升级中断于 {}（未交换，当前树未被动过）：可 restore 清理 staging 残骸",
                    journal.current_state,
                ),
            ),
            RecoveryDisposition::Settled => (
                false,
                false,
                format!("上次升级以 {} 干净收尾，无需对账", journal.current_state),
            ),
            RecoveryDisposition::Quarantined => (false, false, String::new()),
        };
        RecoveryFinding {
            operation_id: operation_id.to_string(),
            journal_path: self.paths.journal_path(operation_id).display().to_string(),
            old_version: journal.old_version.clone(),
            new_version: journal.new_version.clone(),
            interrupted_at: journal.current_state,
            disposition,
            resume_available,
            restore_available,
            detail,
        }
    }
}

/// 当前毫秒（时钟异常记 0，不 panic）——撕裂日志隔离文件名后缀用，与适配层
/// `quarantine_corrupt` 同款做法（`.json.corrupt-<ms>`）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 把撕裂的 journal 改名隔离，返回隔离后的路径。
fn quarantine_corrupt(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let quarantined = path.with_file_name(format!("{name}.corrupt-{}", now_ms()));
    match fs::rename(path, &quarantined) {
        Ok(()) => Some(quarantined),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Harness {
        tmp: tempfile::TempDir,
        paths: RecoveryPaths,
    }

    impl Harness {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let paths = RecoveryPaths {
                install_dir: tmp.path().join("install"),
                backup_dir: tmp.path().join("backup"),
            };
            fs::create_dir_all(&paths.install_dir).unwrap();
            fs::create_dir_all(&paths.backup_dir).unwrap();
            Self { tmp, paths }
        }

        fn root(&self) -> &Path {
            self.tmp.path()
        }

        fn write_tree(&self, dir: &Path, files: &[(&str, &[u8])]) {
            for (rel, bytes) in files {
                let path = dir.join(rel);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, bytes).unwrap();
            }
        }

        /// 落一份 journal 到 `backup/<op>/journal.json`（用写侧同款 `save`）。
        fn write_journal(
            &self,
            op: &str,
            current_state: UpgradeState,
            commit_marker: bool,
            old: Option<&str>,
            new: &str,
        ) {
            let op_dir = self.paths.backup_dir.join(op);
            fs::create_dir_all(&op_dir).unwrap();
            let journal = UpgradeJournal {
                operation_id: op.to_string(),
                old_version: old.map(str::to_string),
                new_version: new.to_string(),
                package_sha256: None,
                backup_path: op_dir.display().to_string(),
                staging_path: self.paths.staging_dir(op).display().to_string(),
                states: vec![current_state],
                current_state,
                commit_marker,
                error: None,
            };
            journal.save(&op_dir.join(JOURNAL_FILE)).unwrap();
        }

        fn tree(&self, dir: &Path) -> BTreeMap<String, String> {
            tree_hashes(dir).unwrap()
        }
    }

    #[test]
    fn committed_upgrade_is_reported_without_pending_action() {
        let h = Harness::new();
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"new")]);
        h.write_tree(&h.paths.previous_dir(), &[("app.bin", b"old")]);
        h.write_journal("op1", UpgradeState::Committed, true, Some("1.0.0"), "2.0.0");

        let report = UpgradeReconciler::new(h.paths.clone()).scan().unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.committed, 1);
        assert!(!report.has_pending(), "已提交不该被当成挂起项");
        let f = &report.findings[0];
        assert_eq!(f.disposition, RecoveryDisposition::Committed);
        assert!(!f.resume_available && !f.restore_available);
    }

    #[test]
    fn torn_journal_is_quarantined_and_does_not_block_or_touch_trees() {
        let h = Harness::new();
        let current_before = {
            h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"old")]);
            h.tree(&h.paths.install_current_dir())
        };
        let op_dir = h.paths.backup_dir.join("op1");
        fs::create_dir_all(&op_dir).unwrap();
        fs::write(op_dir.join(JOURNAL_FILE), b"{ this is not valid json").unwrap();

        let report = UpgradeReconciler::new(h.paths.clone()).scan().unwrap();
        assert_eq!(report.quarantined, 1);
        assert!(report.has_pending());
        assert_eq!(report.findings[0].disposition, RecoveryDisposition::Quarantined);
        // 撕裂文件被隔离、原名不再存在。
        assert!(!op_dir.join(JOURNAL_FILE).exists(), "撕裂 journal 必须被改名隔离");
        let leftovers: Vec<_> = fs::read_dir(&op_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".corrupt-"))
            .collect();
        assert_eq!(leftovers.len(), 1, "应恰有一个 .corrupt- 隔离文件");
        // 不据此改动任何安装树。
        assert_eq!(h.tree(&h.paths.install_current_dir()), current_before);
    }

    #[test]
    fn resume_finalizes_an_uncommitted_swap_without_moving_files() {
        let h = Harness::new();
        let old = [("app.bin", b"old" as &[u8])];
        let new = [("app.bin", b"new" as &[u8])];
        // 交换已发生：current = 新树，previous = 旧树，备份树 = 旧树（回滚底）。
        h.write_tree(&h.paths.install_current_dir(), &new);
        h.write_tree(&h.paths.previous_dir(), &old);
        h.write_tree(&h.paths.backup_tree("op1"), &old);
        h.write_journal("op1", UpgradeState::Swapped, false, Some("1.0.0"), "2.0.0");

        let rec = UpgradeReconciler::new(h.paths.clone());
        let report = rec.scan().unwrap();
        assert_eq!(report.needs_decision, 1);
        assert!(report.findings[0].resume_available && report.findings[0].restore_available);

        let current_before = h.tree(&h.paths.install_current_dir());
        let finding = rec.resume("op1").unwrap();
        assert_eq!(finding.disposition, RecoveryDisposition::Committed);
        // resume 只补记提交，绝不移动文件：current 保持新树。
        assert_eq!(h.tree(&h.paths.install_current_dir()), current_before);
        let reloaded = UpgradeJournal::load(&h.paths.journal_path("op1")).unwrap();
        assert!(reloaded.commit_marker);
        assert_eq!(reloaded.current_state, UpgradeState::Committed);
    }

    #[test]
    fn restore_rolls_back_an_uncommitted_swap_to_the_backup_tree() {
        let h = Harness::new();
        let old = [("app.bin", b"old" as &[u8]), ("lib/data.txt", b"v1" as &[u8])];
        let new = [("app.bin", b"new" as &[u8])];
        h.write_tree(&h.paths.install_current_dir(), &new);
        h.write_tree(&h.paths.previous_dir(), &old);
        h.write_tree(&h.paths.backup_tree("op1"), &old);
        // 残留的 staging/<op>（应被清）。
        h.write_tree(&h.paths.staging_dir("op1"), &[("leftover", b"x" as &[u8])]);
        h.write_journal("op1", UpgradeState::HealthChecked, false, Some("1.0.0"), "2.0.0");

        let rec = UpgradeReconciler::new(h.paths.clone());
        let finding = rec.restore("op1").unwrap();
        assert_eq!(finding.disposition, RecoveryDisposition::Settled);
        // current 真回滚成旧树（逐文件哈希 = 备份树）。
        assert_eq!(h.tree(&h.paths.install_current_dir()), h.tree(&h.paths.backup_tree("op1")));
        // staging 残骸被清空。
        assert!(!h.paths.staging_dir("op1").exists(), "restore 必须清本操作 staging");
        let reloaded = UpgradeJournal::load(&h.paths.journal_path("op1")).unwrap();
        assert_eq!(reloaded.current_state, UpgradeState::RolledBack);
        assert!(!reloaded.commit_marker);
    }

    #[test]
    fn restore_cleans_stale_staging_without_touching_current() {
        let h = Harness::new();
        // 未交换：current 仍是旧树、无 previous、staging 里是新树残骸。
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"old" as &[u8])]);
        h.write_tree(&h.paths.staging_dir("op1"), &[("app.bin", b"new" as &[u8])]);
        h.write_journal("op1", UpgradeState::Extracted, false, Some("1.0.0"), "2.0.0");

        let rec = UpgradeReconciler::new(h.paths.clone());
        let report = rec.scan().unwrap();
        assert_eq!(report.stale_staging, 1);
        assert!(!report.findings[0].resume_available);
        assert!(report.findings[0].restore_available);

        let current_before = h.tree(&h.paths.install_current_dir());
        rec.restore("op1").unwrap();
        // 未交换态绝不碰 current——旧树原样保留。
        assert_eq!(h.tree(&h.paths.install_current_dir()), current_before);
        assert!(!h.paths.staging_dir("op1").exists());
        let reloaded = UpgradeJournal::load(&h.paths.journal_path("op1")).unwrap();
        assert_eq!(reloaded.current_state, UpgradeState::RolledBack);
    }

    #[test]
    fn disposition_maps_every_journal_state_and_failed_split_on_previous() {
        // 交换前里程碑 → StaleStaging；交换后里程碑 → NeedsDecision。
        for state in [
            UpgradeState::Checking,
            UpgradeState::Downloaded,
            UpgradeState::Verified,
            UpgradeState::BackedUp,
            UpgradeState::Extracted,
        ] {
            let h = Harness::new();
            h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"old" as &[u8])]);
            h.write_journal("op", state, false, Some("1.0.0"), "2.0.0");
            let f = &UpgradeReconciler::new(h.paths.clone()).scan().unwrap().findings[0];
            assert_eq!(f.disposition, RecoveryDisposition::StaleStaging, "state {state:?}");
            assert_eq!(f.interrupted_at, state);
        }
        for state in [UpgradeState::Swapped, UpgradeState::HealthChecked] {
            let h = Harness::new();
            h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"new" as &[u8])]);
            h.write_tree(&h.paths.previous_dir(), &[("app.bin", b"old" as &[u8])]);
            h.write_journal("op", state, false, Some("1.0.0"), "2.0.0");
            let f = &UpgradeReconciler::new(h.paths.clone()).scan().unwrap().findings[0];
            assert_eq!(f.disposition, RecoveryDisposition::NeedsDecision, "state {state:?}");
        }
        // Failed：previous 在 = 悬挂交换未提交（需决策），previous 不在 = 干净终态。
        let h = Harness::new();
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"new" as &[u8])]);
        h.write_tree(&h.paths.previous_dir(), &[("app.bin", b"old" as &[u8])]);
        h.write_journal("op", UpgradeState::Failed, false, None, "2.0.0");
        assert_eq!(
            UpgradeReconciler::new(h.paths.clone()).scan().unwrap().findings[0].disposition,
            RecoveryDisposition::NeedsDecision
        );
        let h = Harness::new();
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"old" as &[u8])]);
        h.write_journal("op", UpgradeState::Failed, false, None, "2.0.0");
        assert_eq!(
            UpgradeReconciler::new(h.paths.clone()).scan().unwrap().findings[0].disposition,
            RecoveryDisposition::Settled
        );
        for state in [UpgradeState::RolledBack, UpgradeState::Completed] {
            let h = Harness::new();
            h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"old" as &[u8])]);
            h.write_journal("op", state, false, None, "2.0.0");
            assert_eq!(
                UpgradeReconciler::new(h.paths.clone()).scan().unwrap().findings[0].disposition,
                RecoveryDisposition::Settled,
                "state {state:?}"
            );
        }
    }

    #[test]
    fn resume_and_restore_refuse_states_with_nothing_to_recover() {
        let h = Harness::new();
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"new" as &[u8])]);
        h.write_journal("op1", UpgradeState::Committed, true, Some("1.0.0"), "2.0.0");
        let rec = UpgradeReconciler::new(h.paths.clone());
        assert!(rec.resume("op1").is_err(), "已提交不该能 resume");
        assert!(rec.restore("op1").is_err(), "已提交不该能 restore");

        let h = Harness::new();
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"old" as &[u8])]);
        h.write_journal("op2", UpgradeState::RolledBack, false, None, "2.0.0");
        let rec = UpgradeReconciler::new(h.paths.clone());
        assert!(rec.restore("op2").is_err(), "终态不该能 restore");
    }

    #[test]
    fn resume_refuses_when_previous_tree_is_missing() {
        // journal 说 Swapped，但磁盘上没有 previous 证明交换真发生 → 拒绝补记提交。
        let h = Harness::new();
        h.write_tree(&h.paths.install_current_dir(), &[("app.bin", b"new" as &[u8])]);
        h.write_journal("op1", UpgradeState::Swapped, false, Some("1.0.0"), "2.0.0");
        let rec = UpgradeReconciler::new(h.paths.clone());
        assert!(rec.resume("op1").is_err());
        let reloaded = UpgradeJournal::load(&h.paths.journal_path("op1")).unwrap();
        assert!(!reloaded.commit_marker, "拒绝时不得写 commit 标记");
    }

    #[test]
    fn empty_backup_dir_is_a_clean_fresh_install_not_an_error() {
        let h = Harness::new();
        // Harness 自建 install/backup 两根；根存在但无 journal → 空报告、非错误。
        assert!(h.root().join("install").is_dir() && h.root().join("backup").is_dir());
        let report = UpgradeReconciler::new(h.paths.clone()).scan().unwrap();
        assert_eq!(report.scanned, 0);
        assert!(!report.has_pending());
    }

    #[test]
    fn report_root_dir_absent_returns_empty() {
        // backup_dir 尚未创建（全新安装）——read_dir NotFound 视作空报告，不报错。
        let tmp = tempfile::tempdir().unwrap();
        let paths = RecoveryPaths {
            install_dir: tmp.path().join("install"),
            backup_dir: tmp.path().join("does-not-exist"),
        };
        let report = UpgradeReconciler::new(paths).scan().unwrap();
        assert_eq!(report.scanned, 0);
    }
}
