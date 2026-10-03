//! 特权管理操作的**结构化审计事实**（V4 §135 A103 / Batch 0-3）。
//!
//! 为什么不是配置里的一个开关位：`admin_audit_available: bool` 由宿主自己写，
//! 于是「声明了审计、实际什么都没记」的宿主照样能通过 production 就绪门——这正是
//! §138.2 里「Production mode secure-by-default」长期停在 ⚠️ 的原因。本模块把
//! 审计变成**可验证的事实**：每条记录带序号 + 链式哈希，落盘后带 durable 校验和，
//! 读取侧（`host_production_doctor`）报的是**真实条数与健康状态**，不是布尔声明。
//!
//! 三条不变量：
//! 1. **唯一记录点**：适配器只允许在 [`crate`] 的 `admin_gate`（特权判定咽喉点）里
//!    写入，且只对本模块 [`AUDITED_ADMIN_COMMANDS`] 登记的命令写入——命令名集合
//!    由 wire-gate 与判定代码对账，新增特权写操作忘记登记即红。
//! 2. **有界**：内存与磁盘都只保留最近 [`MAX_ADMIN_AUDIT_RECORDS`] 条，被裁剪的条数
//!    与裁剪点的哈希都进事实（`pruned` / `pruned_head_hash`），链式校验因此仍然成立。
//! 3. **失败不静默**：落盘失败会记 `write_failures` + 末次错误，并使
//!    [`AdminAuditFacts::healthy`] 变假——production 的 `admin-audit` 检查项直接
//!    由它推导，不再有第二个真值来源。

use crate::durable::{decode_durable, encode_durable, DurableEnvelope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// durable 信封的 schema 标识（`DurableEnvelope` 会把它算进校验和）。
pub const ADMIN_AUDIT_SCHEMA: &str = "admin-audit/1";
/// 审计日志的文件名（放在宿主数据目录下）。
pub const ADMIN_AUDIT_FILE: &str = "admin-audit.json";
/// 同目录临时文件名（原子写的中转）。
const ADMIN_AUDIT_TMP_FILE: &str = "admin-audit.json.tmp";
/// 保留的最近记录条数上限（含磁盘与内存两份视图）。
pub const MAX_ADMIN_AUDIT_RECORDS: usize = 512;

/// **必须留下审计事实**的特权命令。
///
/// 取舍标准是「改变状态 / 授予或撤销权限 / 触达供应链」，而不是「调用方是主窗」：
/// `host_resource_stats`、`host_runtime_health`、`host_events_approvals`、
/// `host_production_doctor` 这类**只读**特权命令刻意不进本表——宿主 UI 每次打开
/// 管理面板都会读它们，把它们记进有界日志会把真正的授权/安装事实挤掉。
/// 判定与登记的对账由 `tauron-adapter` 的 `admin_gate` 单点 + wire-gate 命令名集合
/// 比对共同守住：表里的名字必须真的走 `admin_gate`，走 `admin_gate` 的名字必须在表里。
///
/// `host_market_download` / `host_market_install` **刻意不在表里**：它们是
/// `simulated: true` 的桩（不请求网络、不写文件），而桩的审计等于给未实现的能力
/// 造事实。真实接线时必须连同 `authz` 档位登记一起纳入本表——档位与审计同源。
pub const AUDITED_ADMIN_COMMANDS: &[&str] = &[
    "host_events_approve",
    "host_events_revoke",
    "host_registry_admin",
    "host_registry_install",
    "host_registry_install_preview",
    "host_runtime_spawn",
];

/// 命令是否需要审计。
pub fn audited(command: &str) -> bool {
    AUDITED_ADMIN_COMMANDS.contains(&command)
}

/// 审计结论：授权判定本身的两条出路都要留痕（被拒的特权尝试是首要信号）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdminAuditOutcome {
    Allowed,
    Denied,
}

impl AdminAuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }
}

/// 一条审计记录（camelCase 线形；`hash` 覆盖除自身外的全部字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminAuditRecord {
    /// 宿主内单调序号（从 1 起；重启后从落盘日志续接）。
    pub seq: u64,
    /// 记录时刻（epoch 毫秒）。**排序依据是 `seq`**：本机时钟可能被调，
    /// 而序号由宿主自己的计数器给出，重启续接也不会倒退。
    pub ts_ms: u64,
    pub command: String,
    /// 主体描述：`main-window` 或 `plugin:<插件 id>`。
    pub caller: String,
    pub plugin_id: Option<String>,
    pub outcome: AdminAuditOutcome,
    /// 被拒时的错误码（放行时为 `None`）。
    pub error_code: Option<String>,
    /// 上一条的哈希；日志首条在无裁剪时为 `""`，被裁剪后等于 `pruned_head_hash`。
    pub prev_hash: String,
    pub hash: String,
}

impl AdminAuditRecord {
    /// 本记录的哈希（链式）。
    fn content_hash(prev_hash: &str, fields: &Self) -> String {
        let canonical = serde_json::to_vec(&serde_json::json!({
            "seq": fields.seq,
            "tsMs": fields.ts_ms,
            "command": fields.command,
            "caller": fields.caller,
            "pluginId": fields.plugin_id,
            "outcome": fields.outcome,
            "errorCode": fields.error_code,
            "prevHash": prev_hash,
        }))
        .unwrap_or_default();
        hex::encode(Sha256::digest(canonical))
    }
}

/// 落盘载荷：环形裁剪后仍要保持链可验证，所以记录裁剪点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdminAuditPayload {
    pruned: u64,
    pruned_head_hash: String,
    records: Vec<AdminAuditRecord>,
}

/// 审计日志的读取侧快照（`host_production_doctor` 直接上线这一份）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditFacts {
    /// 是否落盘（`false` = 内存态，重启即丢，production 视为不健康）。
    pub durable: bool,
    /// 当前保留条数（≤ [`MAX_ADMIN_AUDIT_RECORDS`]）。
    pub records: usize,
    /// 本宿主累计写入条数（含被裁剪的）。
    pub total_recorded: u64,
    /// 被环形裁剪掉的条数。
    pub pruned: u64,
    /// 落盘失败次数。
    pub write_failures: u64,
    /// 末次落盘错误（`None` = 从未失败）。
    pub last_write_error: Option<String>,
    /// 链式哈希 + durable 校验和是否成立。
    pub chain_intact: bool,
    /// 末条记录的命令（诊断用）。
    pub last_command: Option<String>,
    /// 末条记录的结论（诊断用）。
    pub last_outcome: Option<String>,
}

impl AdminAuditFacts {
    /// 审计通道是否**真的可用**：必须落盘、链完整、且从未丢过写。
    ///
    /// production 的 `admin-audit` 检查项只看这一个函数。
    pub fn healthy(&self) -> bool {
        self.durable && self.chain_intact && self.write_failures == 0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdminAuditError {
    #[error("admin audit file io failed: {0}")]
    Io(String),
    #[error("admin audit durable envelope is invalid: {0}")]
    Integrity(String),
    #[error("admin audit hash chain is broken at record {0}")]
    ChainBroken(u64),
}

/// 有界、链式、可选落盘的特权操作审计日志。
#[derive(Debug, Default)]
pub struct AdminAuditSink {
    state: Mutex<SinkState>,
    path: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct SinkState {
    records: Vec<AdminAuditRecord>,
    pruned: u64,
    pruned_head_hash: String,
    seq: u64,
    head_hash: String,
    write_failures: u64,
    last_write_error: Option<String>,
}

impl AdminAuditSink {
    /// 纯内存审计（单测与无数据目录的宿主）。`healthy()` 恒假——不假装能持久化。
    pub fn in_memory() -> Self {
        Self { state: Mutex::new(SinkState::default()), path: None }
    }

    /// 打开（或创建）`dir` 下的持久化审计日志。
    ///
    /// 读回时同时校验 durable 校验和与哈希链：任何一条不成立都**拒绝装配**，
    /// 而不是把一份可能被篡改/撕裂的日志当成事实源。
    pub fn open(dir: &Path) -> Result<Self, AdminAuditError> {
        std::fs::create_dir_all(dir).map_err(|e| AdminAuditError::Io(e.to_string()))?;
        let path = dir.join(ADMIN_AUDIT_FILE);
        let sink = Self { state: Mutex::new(SinkState::default()), path: Some(path.clone()) };
        if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| AdminAuditError::Io(e.to_string()))?;
            let envelope: DurableEnvelope<AdminAuditPayload> =
                decode_durable(&bytes).map_err(|e| AdminAuditError::Integrity(e.to_string()))?;
            let payload = envelope.payload;
            verify_records(&payload.pruned_head_hash, &payload.records)?;
            let head_hash = payload
                .records
                .last()
                .map(|r| r.hash.clone())
                .unwrap_or_else(|| payload.pruned_head_hash.clone());
            let seq = payload.records.last().map(|r| r.seq).unwrap_or(0);
            let mut state = sink.state.lock().unwrap_or_else(|e| e.into_inner());
            state.records = payload.records;
            state.pruned = payload.pruned;
            state.pruned_head_hash = payload.pruned_head_hash;
            state.seq = seq;
            state.head_hash = head_hash;
        }
        Ok(sink)
    }

    /// 记录一条特权判定。
    pub fn record(
        &self,
        command: &str,
        caller: &str,
        plugin_id: Option<&str>,
        outcome: AdminAuditOutcome,
        error_code: Option<&str>,
    ) -> AdminAuditRecord {
        let bytes = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.seq += 1;
            let mut record = AdminAuditRecord {
                seq: state.seq,
                ts_ms: now_ms(),
                command: command.to_string(),
                caller: caller.to_string(),
                plugin_id: plugin_id.map(str::to_string),
                outcome,
                error_code: error_code.map(str::to_string),
                prev_hash: state.head_hash.clone(),
                hash: String::new(),
            };
            record.hash = AdminAuditRecord::content_hash(&record.prev_hash, &record);
            state.head_hash = record.hash.clone();
            state.records.push(record.clone());
            while state.records.len() > MAX_ADMIN_AUDIT_RECORDS {
                let dropped = state.records.remove(0);
                state.pruned += 1;
                state.pruned_head_hash = dropped.hash;
            }
            let payload = AdminAuditPayload {
                pruned: state.pruned,
                pruned_head_hash: state.pruned_head_hash.clone(),
                records: state.records.clone(),
            };
            // 编码留在锁内（纯内存），磁盘写放到锁外。
            match DurableEnvelope::seal(ADMIN_AUDIT_SCHEMA, state.seq, payload)
                .and_then(|envelope| encode_durable(&envelope))
            {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    state.write_failures += 1;
                    state.last_write_error = Some(error.to_string());
                    None
                }
            }
        };
        if let (Some(path), Some(bytes)) = (self.path.as_ref(), bytes) {
            if let Err(error) = write_durable(path, &bytes) {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.write_failures += 1;
                state.last_write_error = Some(error.to_string());
            }
        }
        self.record_snapshot().expect("just recorded")
    }

    /// 当前保留的全部记录。
    pub fn records(&self) -> Vec<AdminAuditRecord> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).records.clone()
    }

    /// 读取侧快照（doctor 用它推导 `admin-audit` 检查项）。
    pub fn facts(&self) -> AdminAuditFacts {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let chain_intact = verify_records(&state.pruned_head_hash, &state.records).is_ok()
            && state.seq >= state.records.len() as u64;
        let last = state.records.last();
        AdminAuditFacts {
            durable: self.path.is_some(),
            records: state.records.len(),
            total_recorded: state.seq,
            pruned: state.pruned,
            write_failures: state.write_failures,
            last_write_error: state.last_write_error.clone(),
            chain_intact,
            last_command: last.map(|r| r.command.clone()),
            last_outcome: last.map(|r| r.outcome.as_str().to_string()),
        }
    }

    /// 是否落盘。
    pub fn is_durable(&self) -> bool {
        self.path.is_some()
    }

    fn record_snapshot(&self) -> Option<AdminAuditRecord> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).records.last().cloned()
    }
}

/// 校验一段记录的链式哈希（首条的 `prev_hash` 必须等于裁剪点哈希）。
pub fn verify_records(
    pruned_head_hash: &str,
    records: &[AdminAuditRecord],
) -> Result<(), AdminAuditError> {
    let mut prev = pruned_head_hash.to_string();
    for record in records {
        if record.prev_hash != prev {
            return Err(AdminAuditError::ChainBroken(record.seq));
        }
        if record.hash != AdminAuditRecord::content_hash(&record.prev_hash, record) {
            return Err(AdminAuditError::ChainBroken(record.seq));
        }
        prev = record.hash.clone();
    }
    Ok(())
}

/// 校验落盘文件（独立于 sink 的离线取证入口）。
pub fn verify_file(path: &Path) -> Result<AdminAuditFacts, AdminAuditError> {
    let bytes = std::fs::read(path).map_err(|e| AdminAuditError::Io(e.to_string()))?;
    let envelope: DurableEnvelope<AdminAuditPayload> =
        decode_durable(&bytes).map_err(|e| AdminAuditError::Integrity(e.to_string()))?;
    verify_records(&envelope.payload.pruned_head_hash, &envelope.payload.records)?;
    let last = envelope.payload.records.last();
    Ok(AdminAuditFacts {
        durable: true,
        records: envelope.payload.records.len(),
        total_recorded: envelope.generation,
        pruned: envelope.payload.pruned,
        write_failures: 0,
        last_write_error: None,
        chain_intact: true,
        last_command: last.map(|r| r.command.clone()),
        last_outcome: last.map(|r| r.outcome.as_str().to_string()),
    })
}

/// 原子写（临时文件 + `sync_all` + rename），与恢复/设置域同一套做法。
fn write_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(ADMIN_AUDIT_TMP_FILE);
    use std::io::Write;
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audited_command_list_is_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for command in AUDITED_ADMIN_COMMANDS {
            assert!(command.starts_with("host_"), "{command} 不是宿主命令名");
            assert!(seen.insert(*command), "{command} 重复登记");
        }
        // 只读特权命令**不得**进表（见 `audited` 的取舍说明）。
        for read_only in [
            "host_resource_stats",
            "host_runtime_health",
            "host_events_approvals",
            "host_production_doctor",
        ] {
            assert!(!audited(read_only), "{read_only} 是只读特权命令，不该挤占审计日志");
        }
    }

    #[test]
    fn records_chain_in_order_and_verify() {
        let sink = AdminAuditSink::in_memory();
        let first = sink.record(
            "host_registry_admin",
            "main-window",
            None,
            AdminAuditOutcome::Allowed,
            None,
        );
        let second = sink.record(
            "host_events_revoke",
            "plugin-com.a",
            Some("com.a"),
            AdminAuditOutcome::Denied,
            Some("E_AUTH_DENIED"),
        );
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev_hash, first.hash);
        assert_eq!(first.prev_hash, "");
        assert!(verify_records("", &sink.records()).is_ok());
        let facts = sink.facts();
        assert_eq!(facts.records, 2);
        assert_eq!(facts.total_recorded, 2);
        assert_eq!(facts.last_command.as_deref(), Some("host_events_revoke"));
        assert_eq!(facts.last_outcome.as_deref(), Some("denied"));
        // 内存态不假装可持久化。
        assert!(!facts.healthy());
        assert!(!facts.durable);
    }

    #[test]
    fn tampering_with_a_record_breaks_the_chain() {
        let sink = AdminAuditSink::in_memory();
        sink.record("host_runtime_spawn", "main-window", None, AdminAuditOutcome::Allowed, None);
        sink.record(
            "host_registry_admin",
            "plugin-com.a",
            Some("com.a"),
            AdminAuditOutcome::Denied,
            Some("E_AUTH_DENIED"),
        );
        let mut forged = sink.records();
        forged[0].outcome = AdminAuditOutcome::Denied;
        assert!(matches!(verify_records("", &forged), Err(AdminAuditError::ChainBroken(1))));
    }

    #[test]
    fn ring_keeps_the_newest_records_and_still_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let sink = AdminAuditSink::open(dir.path()).unwrap();
        for _ in 0..(MAX_ADMIN_AUDIT_RECORDS + 7) {
            sink.record(
                "host_events_approve",
                "main-window",
                None,
                AdminAuditOutcome::Allowed,
                None,
            );
        }
        let facts = sink.facts();
        assert_eq!(facts.records, MAX_ADMIN_AUDIT_RECORDS);
        assert_eq!(facts.pruned, 7);
        assert_eq!(facts.total_recorded, (MAX_ADMIN_AUDIT_RECORDS + 7) as u64);
        assert!(facts.chain_intact);
        assert_eq!(facts.write_failures, 0);
        assert!(facts.healthy());

        // 重新打开：序号续接、链仍成立、被裁剪的头哈希仍是链头依据。
        drop(sink);
        let reopened = AdminAuditSink::open(dir.path()).unwrap();
        let reopened_facts = reopened.facts();
        assert_eq!(reopened_facts.records, MAX_ADMIN_AUDIT_RECORDS);
        assert_eq!(reopened_facts.pruned, 7);
        assert!(reopened_facts.chain_intact);
        let next = reopened.record(
            "host_registry_admin",
            "main-window",
            None,
            AdminAuditOutcome::Allowed,
            None,
        );
        assert_eq!(next.seq, (MAX_ADMIN_AUDIT_RECORDS + 8) as u64);
    }

    #[test]
    fn durable_sink_is_read_back_from_disk_and_verifiable_offline() {
        let dir = tempfile::tempdir().unwrap();
        let sink = AdminAuditSink::open(dir.path()).unwrap();
        assert!(sink.is_durable());
        let written = sink.record(
            "host_registry_admin",
            "main-window",
            None,
            AdminAuditOutcome::Denied,
            Some("E_STATE_INVALID_TRANSITION"),
        );
        let facts = verify_file(&dir.path().join(ADMIN_AUDIT_FILE)).unwrap();
        assert_eq!(facts.records, 1);
        assert_eq!(facts.last_command.as_deref(), Some("host_registry_admin"));
        assert_eq!(written.outcome, AdminAuditOutcome::Denied);
        assert_eq!(written.error_code.as_deref(), Some("E_STATE_INVALID_TRANSITION"));
    }

    #[test]
    fn a_torn_or_edited_audit_file_refuses_to_open() {
        let dir = tempfile::tempdir().unwrap();
        AdminAuditSink::open(dir.path()).unwrap().record(
            "host_registry_admin",
            "main-window",
            None,
            AdminAuditOutcome::Allowed,
            None,
        );
        let file = dir.path().join(ADMIN_AUDIT_FILE);
        let text = std::fs::read_to_string(&file).unwrap();
        // 篡改 payload（把 allowed 换成 denied）而不重算校验和 → durable 校验失败。
        let forged = text.replace("\"outcome\":\"allowed\"", "\"outcome\":\"denied\"");
        assert_ne!(forged, text, "测试前提：payload 里应有 outcome 字段");
        std::fs::write(&file, forged).unwrap();
        let err = AdminAuditSink::open(dir.path()).unwrap_err();
        assert!(matches!(err, AdminAuditError::Integrity(_)), "篡改必须落在完整性错误上：{err:?}");
    }
}
