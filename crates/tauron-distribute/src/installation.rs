// §9.1 InstallationIdentityProvider：灰度分桶必须基于本机稳定安装身份，
// 不允许固定 hash（V4 §33 R2-8 的修复主体）。
//
// 卸载/重装策略（§9.1 要求"显式定义"）：
// - id 文件随宿主数据目录（`AdapterConfig::recovery_data_dir`）持久化；
// - 卸载时若数据目录被清除，重装随机生成新 id（进入新灰度桶）；
// - 升级/覆盖安装保留数据目录 → id 与灰度桶不变。
//
// 无 PII：仅 UUIDv4 随机数，不含用户名、机器名、MAC、磁盘序列号等任何
// 可关联到自然人的信息。

use std::io::Write;
use std::path::Path;

use crate::error::{DistributeError, DistributeResult};

/// 持久化文件名（位于宿主数据目录之下）。
const ID_FILE: &str = "installation.id";

/// 本机安装身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationIdentity {
    id: String,
    durable: bool,
}

impl InstallationIdentity {
    /// 从数据目录加载；首次使用时随机生成并持久化（并发安全：`create_new`，
    /// 竞争失败方读回先写入的 id，两端得到同一身份）。
    ///
    /// 已存在但损坏/为空的文件**拒绝启动分桶**而非静默重置——静默重新生成会
    /// 让同一安装实例跳桶，破坏灰度停留语义。
    pub fn load_or_create(dir: &Path) -> DistributeResult<Self> {
        let path = dir.join(ID_FILE);
        if let Some(id) = read_valid(&path)? {
            return Ok(Self { id, durable: true });
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(id.as_bytes()).map_err(|e| io_err(&path, e))?;
                Ok(Self { id, durable: true })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // 并发首启：另一进程已写入，读回同一身份。
                if let Some(id) = read_valid(&path)? {
                    Ok(Self { id, durable: true })
                } else {
                    Err(DistributeError::InstallationIdentity(format!(
                        "安装身份文件 `{}` 在竞争窗口内被清空",
                        path.display()
                    )))
                }
            }
            Err(e) => Err(io_err(&path, e)),
        }
    }

    /// **非持久**随机身份：仅用于测试或无数据目录场景。进程重启后 id 变化 →
    /// 灰度桶不稳定，禁止作为正式分发路径。
    pub fn ephemeral() -> Self {
        Self { id: uuid::Uuid::new_v4().simple().to_string(), durable: false }
    }

    /// 由已知 id 重建（受管部署/回滚验证场景）。id 必须通过格式校验。
    pub fn from_id(id: &str) -> DistributeResult<Self> {
        if !is_valid_id(id) {
            return Err(DistributeError::InstallationIdentity(format!(
                "安装身份 `{id}` 不合法（需 12–64 位字母数字或 `-`）"
            )));
        }
        Ok(Self { id: id.to_string(), durable: false })
    }

    /// 安装 id 本身（§9.1 `stableInstallationId()`）。
    pub fn installation_id(&self) -> &str {
        &self.id
    }

    /// 是否来自持久化文件（`ephemeral`/`from_id` 为 false）。
    pub fn is_durable(&self) -> bool {
        self.durable
    }

    /// 灰度分桶用的稳定非负标识（FNV-1a 64 折叠到 u32，无跨进程随机性）。
    pub fn user_hash(&self) -> u32 {
        Self::hash_id(&self.id)
    }

    /// 纯函数哈希：同一 id 在任何进程/平台得到同一 u32。
    pub fn hash_id(id: &str) -> u32 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in id.as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        ((h >> 32) ^ (h & 0xffff_ffff)) as u32
    }
}

fn read_valid(path: &Path) -> DistributeResult<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let id = raw.trim().to_string();
            if is_valid_id(&id) {
                Ok(Some(id))
            } else {
                Err(DistributeError::InstallationIdentity(format!(
                    "安装身份文件 `{}` 损坏或为空，拒绝静默重置灰度桶",
                    path.display()
                )))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(path, e)),
    }
}

fn io_err(path: &Path, e: std::io::Error) -> DistributeError {
    DistributeError::InstallationIdentity(format!(
        "安装身份文件 `{}` 读写失败：{e}",
        path.display()
    ))
}

fn is_valid_id(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_ok() -> crate::UpdateManifest {
        crate::UpdateManifest {
            version: "2.0.0".into(),
            url: "https://example.com/app.zip".into(),
            signature: "abc123def456".into(),
            release_date: "2026-09-21T00:00:00Z".into(),
            platform_notes: Default::default(),
        }
    }

    struct AvailableEndpoint;
    impl crate::EndpointClient for AvailableEndpoint {
        fn fetch_manifest(
            &self,
            _current_version: &str,
        ) -> DistributeResult<Option<crate::UpdateManifest>> {
            Ok(Some(manifest_ok()))
        }
    }

    fn batch_policy(current: crate::GrayscaleBatch) -> crate::GrayscalePolicy {
        crate::GrayscalePolicy { current, ..Default::default() }
    }

    #[test]
    fn load_or_create_is_stable_within_one_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let first = InstallationIdentity::load_or_create(dir.path()).unwrap();
        let second = InstallationIdentity::load_or_create(dir.path()).unwrap();
        assert_eq!(first, second, "同一数据目录必须返回同一身份");
        assert!(first.is_durable());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(ID_FILE)).unwrap().trim(),
            first.installation_id()
        );
    }

    #[test]
    fn distinct_data_dirs_get_distinct_ids() {
        let a = InstallationIdentity::load_or_create(tempfile::tempdir().unwrap().path()).unwrap();
        let b = InstallationIdentity::load_or_create(tempfile::tempdir().unwrap().path()).unwrap();
        assert_ne!(a.installation_id(), b.installation_id());
    }

    #[test]
    fn corrupted_identity_file_is_rejected_not_reset() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ID_FILE), "  \n").unwrap();
        let e = InstallationIdentity::load_or_create(dir.path()).unwrap_err();
        assert!(e.to_string().contains("损坏或为空"), "{e}");
        // 拒绝时不得覆盖原文件（保留取证现场）。
        assert_eq!(std::fs::read_to_string(dir.path().join(ID_FILE)).unwrap(), "  \n");
    }

    #[test]
    fn from_id_validates_and_is_reusable() {
        let id = "0123456789abcdef0123456789abcdef";
        let a = InstallationIdentity::from_id(id).unwrap();
        let b = InstallationIdentity::from_id(id).unwrap();
        assert_eq!(a.user_hash(), b.user_hash());
        assert!(!a.is_durable());
        assert!(matches!(
            InstallationIdentity::from_id("nope"),
            Err(DistributeError::InstallationIdentity(_))
        ));
    }

    #[test]
    fn user_hash_is_deterministic_and_distinguishes_ids() {
        let base = InstallationIdentity::from_id("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let other = InstallationIdentity::from_id("baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(base.user_hash(), InstallationIdentity::hash_id(base.installation_id()));
        assert_ne!(base.user_hash(), other.user_hash());
    }

    #[test]
    fn buckets_are_spread_across_installations_and_stable_in_check() {
        // R2-8 Gate：不同 installation id 分桶不同且稳定。
        let mut buckets = std::collections::BTreeSet::new();
        let ids: Vec<InstallationIdentity> =
            (0..200).map(|_| InstallationIdentity::ephemeral()).collect();
        for id in &ids {
            buckets.insert(id.user_hash() % 100);
        }
        assert!(buckets.len() > 1, "200 个随机安装身份必须落在多个灰度桶");

        let client = AvailableEndpoint;
        let policy = batch_policy(crate::GrayscaleBatch::Batch5);
        let inside = ids.iter().find(|i| i.user_hash() % 100 < 5).unwrap();
        let outside = ids.iter().find(|i| i.user_hash() % 100 >= 5).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                crate::check_for_update(&client, "1.0.0", &policy, inside.user_hash()),
                Ok(crate::UpdateCheckResult::UpdateAvailable(_))
            ));
            assert!(matches!(
                crate::check_for_update(&client, "1.0.0", &policy, outside.user_hash()),
                Ok(crate::UpdateCheckResult::NotInGrayscale)
            ));
        }
    }

    #[test]
    fn ephemeral_ids_differ_and_are_not_durable() {
        let a = InstallationIdentity::ephemeral();
        let b = InstallationIdentity::ephemeral();
        assert_ne!(a.installation_id(), b.installation_id());
        assert!(!a.is_durable());
    }
}
