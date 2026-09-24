//! §4.5 授予集：落盘带版本 + HMAC 防篡改签名。
//!
//! 为什么必须签名：授予集是**授权的唯一事实来源**。如果它可被插件或
//! 第三方写坏（多塞一条 `fs:allow-app-write-recursive`），审批流就形同虚设。
//! 宿主每次加载都验签，签名不匹配按"篡改"处理，宁可全量拒绝也不降级放行。

use std::path::PathBuf;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::Sha256;

use tauron_host::error::{ErrorCode, HostError, HostResult};
use tauron_host::manifest::{Permission, Risk};

type HmacSha256 = Hmac<Sha256>;

/// 授予集 schema 版本。破坏性变更时自增；旧版本读取方必须显式拒绝。
pub const GRANT_SET_SCHEMA_VERSION: u32 = 1;

/// 一条已批准的权限。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GrantEntry {
    /// Tauri 权限标识（词表内，见 [`PermissionIndex`]）。
    pub permission: String,
    /// 风险档位，来自权限词表——**审批 UI 文案与代码共享这一来源**。
    pub risk: Risk,
    /// 人话描述，来自权限词表同一条目。
    pub description: String,
    /// 该权限是否需要 scope。
    pub scoped: bool,
}

/// 一个插件的已批准授予集。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GrantSet {
    pub schema_version: u32,
    /// 授予集修订号：每次变更自增，供 diff 判断新旧。
    pub revision: u32,
    pub plugin_id: String,
    pub plugin_version: String,
    /// manifest 的 `framework` range，作为"框架升级后需重审"的判定依据。
    pub framework: String,
    pub grants: Vec<GrantEntry>,
    /// 权限标识 -> scope 值（与 Tauri capability 的 `scopes` 同形）。
    pub scopes: Map<String, Value>,
    /// 批准时刻（unix 秒）。
    pub approved_at_unix: u64,
    /// 批准者标识（"user" 或白标方管理员账号）。
    pub approver: String,
}

impl GrantSet {
    /// 该权限是否被授予。
    pub fn has_permission(&self, p: &str) -> bool {
        self.grants.iter().any(|g| g.permission == p)
    }

    /// 该权限被授予的 scope。
    pub fn scope_of(&self, p: &str) -> Option<&Value> {
        self.scopes.get(p)
    }

    /// 转成权限字符串切片，便于喂给 `tauron_host::authz::check_grants`。
    pub fn permissions(&self) -> Vec<Permission> {
        self.grants
            .iter()
            .map(|g| Permission::new(g.permission.as_str()))
            .collect()
    }
}

/// 签名后的授予集（落盘形态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SignedGrantSet {
    pub grant_set: GrantSet,
    /// HMAC-SHA256(canonical JSON) 的十六进制。
    pub signature: String,
}

/// 授予集的规范字节序列：紧凑 JSON + 键序稳定（`serde_json::Map` 无
/// `preserve_order` 特性时为 BTreeMap，键按字典序输出）。
///
/// 签名的正确性完全取决于这里：只要序列化顺序在不同机器上不一致，
/// 验签就会随机失败，整个防篡改机制就废了。
pub fn canonical_bytes(grant_set: &GrantSet) -> HostResult<Vec<u8>> {
    serde_json::to_vec(grant_set).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("授予集序列化失败：{e}"),
        )
    })
}

pub fn sign(grant_set: &GrantSet, key: &[u8]) -> HostResult<String> {
    if key.is_empty() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "签名密钥不可为空",
        ));
    }
    let bytes = canonical_bytes(grant_set)?;
    let mut mac = HmacSha256::new_from_slice(key).map_err(|e| {
        HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("签名密钥无效：{e}"))
    })?;
    mac.update(&bytes);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// 验签。失败分两类：
/// - 签名缺失/格式错 → [`ErrorCode::E_INVALID_MANIFEST`]
/// - 签名与内容不符（**被篡改**）→ [`ErrorCode::E_FORBIDDEN_PERMISSION`]
pub fn verify(signed: &SignedGrantSet, key: &[u8]) -> HostResult<()> {
    let bytes = canonical_bytes(&signed.grant_set)?;
    let mut mac = HmacSha256::new_from_slice(key).map_err(|e| {
        HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("签名密钥无效：{e}"))
    })?;
    mac.update(&bytes);
    let expected = hex::encode(mac.finalize().into_bytes());

    if signed.signature.is_empty() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "授予集缺少签名",
        ));
    }
    if !constant_time_eq(signed.signature.as_bytes(), expected.as_bytes()) {
        return Err(HostError::new(
            ErrorCode::E_FORBIDDEN_PERMISSION,
            format!(
                "授予集签名不匹配：授予集被篡改（plugin {}），拒绝加载",
                signed.grant_set.plugin_id
            ),
        ));
    }
    Ok(())
}

/// 恒定时间比较，避免长度/前缀侧信道。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ──────────────────────────────────────────────────────────────────────────
// 落盘存储
// ──────────────────────────────────────────────────────────────────────────

/// 授予集存储：一个文件 = 一个插件的已批准授予集。
///
/// 原子写：先写 `.tmp` 再 `rename`，避免半写状态被当成"合法但截断"的授予集
/// 读进来（计划 §4.14 的幂等/半写测试是同一类问题）。
pub struct AclStore {
    dir: PathBuf,
    key: Vec<u8>,
}

impl AclStore {
    pub fn new(dir: impl Into<PathBuf>, key: Vec<u8>) -> Self {
        Self { dir: dir.into(), key }
    }

    /// 该插件的授予集路径：`<dir>/<plugin_id>.acl.json`。
    ///
    /// 插件 id 含点号（反域名），直接拼进文件名安全（不含路径分隔符）。
    pub fn path_for(&self, plugin_id: &str) -> PathBuf {
        self.dir.join(format!("{plugin_id}.acl.json"))
    }

    pub fn save(&self, grant_set: &GrantSet) -> HostResult<PathBuf> {
        if grant_set.schema_version != GRANT_SET_SCHEMA_VERSION {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "授予集 schema_version {} 与当前支持的 {GRANT_SET_SCHEMA_VERSION} 不符",
                    grant_set.schema_version
                ),
            ));
        }
        if grant_set.grants.is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "授予集不可为空",
            ));
        }
        // 禁止授予清单在授予路径上硬拒（§2.1）。
        tauron_host::authz::check_grants(&grant_set.scopes, &grant_set.permissions())?;

        let signed = SignedGrantSet {
            grant_set: grant_set.clone(),
            signature: sign(grant_set, &self.key)?,
        };
        let bytes = serde_json::to_vec_pretty(&signed).map_err(|e| {
            HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("授予集序列化失败：{e}"))
        })?;

        let path = self.path_for(&grant_set.plugin_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("创建授予集目录失败：{e}"))
            })?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("写入授予集临时文件失败 {}: {e}", tmp.display()),
            )
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("授予集原子替换失败 {}: {e}", path.display()),
            )
        })?;
        Ok(path)
    }

    /// 读取并验签。文件不存在 → `Ok(None)`（尚未审批）。
    pub fn load(&self, plugin_id: &str) -> HostResult<Option<SignedGrantSet>> {
        let path = self.path_for(plugin_id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("读取授予集失败 {}: {e}", path.display()),
            )
        })?;
        let signed: SignedGrantSet = serde_json::from_slice(&bytes).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("授予集 JSON 解析失败 {}: {e}", path.display()),
            )
        })?;
        verify(&signed, &self.key)?;
        Ok(Some(signed))
    }

    /// 读取，**不验签**——仅供"检测到篡改"的诊断路径使用。
    pub fn load_unverified(&self, plugin_id: &str) -> HostResult<Option<SignedGrantSet>> {
        let path = self.path_for(plugin_id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("读取授予集失败 {}: {e}", path.display()),
            )
        })?;
        serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("授予集 JSON 解析失败：{e}"),
            )
        })
    }

    /// 物化为 Tauri capability 形态（§4.5：安装时审批 → 物化为 `Capability`）。
    ///
    /// 字段名与 Tauri 2.x 的 `Capability` 对齐；实际 `add_capability` 调用
    /// 在 Tauri 适配层（§4.23）完成。
    pub fn to_capability(&self, signed: &SignedGrantSet) -> Value {
        let gs = &signed.grant_set;
        Value::Object(Map::from_iter([
            (
                "identifier".to_string(),
                Value::String(format!("plugin-{plugin_id}", plugin_id = gs.plugin_id)),
            ),
            (
                "webviews".to_string(),
                Value::Array(vec![Value::String(format!("plugin-{}", gs.plugin_id))]),
            ),
            (
                "permissions".to_string(),
                Value::Array(
                    gs.grants.iter().map(|g| Value::String(g.permission.clone())).collect(),
                ),
            ),
        ]))
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"test-host-key-000000000000000000000000000000";

    fn gs(revision: u32) -> GrantSet {
        let mut scopes = Map::new();
        scopes.insert("http:allow-fetch".to_string(), Value::Array(vec![Value::String("https://api.example.com/**".into())]));
        GrantSet {
            schema_version: GRANT_SET_SCHEMA_VERSION,
            revision,
            plugin_id: "com.example.formatter".into(),
            plugin_version: "1.2.3".into(),
            framework: ">=2.0 <3.0".into(),
            grants: vec![GrantEntry {
                permission: "http:allow-fetch".into(),
                risk: Risk::Elevated,
                description: "发起网络请求".into(),
                scoped: true,
            }],
            scopes,
            approved_at_unix: 1_700_000_000,
            approver: "user".into(),
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let g = gs(1);
        let sig = sign(&g, KEY).unwrap();
        let ok = SignedGrantSet { grant_set: g, signature: sig };
        assert!(verify(&ok, KEY).is_ok());
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let g = gs(1);
        let sig = sign(&g, KEY).unwrap();
        let bad = SignedGrantSet { grant_set: g, signature: sig };
        assert!(verify(&bad, b"different-key").is_err());
    }

    #[test]
    fn verify_rejects_tampered_grant() {
        let mut g = gs(1);
        let sig = sign(&g, KEY).unwrap();
        // 篡改：多塞一条高危权限。
        g.grants.push(GrantEntry {
            permission: "fs:allow-app-write-recursive".into(),
            risk: Risk::High,
            description: "写应用目录".into(),
            scoped: true,
        });
        let tampered = SignedGrantSet { grant_set: g, signature: sig };
        let e = verify(&tampered, KEY).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("篡改"));
    }

    #[test]
    fn verify_rejects_tampered_scope() {
        let mut g = gs(1);
        let sig = sign(&g, KEY).unwrap();
        // 篡改 scope：从受限域放宽到全网。
        g.scopes.insert(
            "http:allow-fetch".to_string(),
            Value::Array(vec![Value::String("https://*".into())]),
        );
        let tampered = SignedGrantSet { grant_set: g, signature: sig };
        assert!(verify(&tampered, KEY).is_err());
    }

    #[test]
    fn verify_rejects_missing_signature() {
        let g = gs(1);
        let none = SignedGrantSet { grant_set: g, signature: String::new() };
        let e = verify(&none, KEY).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("缺少签名"));
    }

    #[test]
    fn sign_with_empty_key_is_rejected() {
        assert!(sign(&gs(1), b"").is_err());
    }

    #[test]
    fn constant_time_eq_rejects_length_mismatch() {
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
    }

    #[test]
    fn canonical_bytes_are_stable_and_sorted() {
        let a = gs(1);
        let mut b = gs(1);
        b.grants.clone_from(&a.grants);
        assert_eq!(canonical_bytes(&a).unwrap(), canonical_bytes(&b).unwrap());
        // revision 变化必须改变签名输入。
        let c = gs(2);
        assert_ne!(canonical_bytes(&a).unwrap(), canonical_bytes(&c).unwrap());
    }

    #[test]
    fn grant_set_queries() {
        let g = gs(1);
        assert!(g.has_permission("http:allow-fetch"));
        assert!(!g.has_permission("shell:allow-execute"));
        assert!(g.scope_of("http:allow-fetch").is_some());
        assert!(g.scope_of("nope").is_none());
        assert_eq!(g.permissions().len(), 1);
    }

    #[test]
    fn store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let g = gs(1);
        let path = store.save(&g).unwrap();
        assert!(path.exists());
        // 临时文件不应残留。
        assert!(!path.with_extension("json.tmp").exists());
        let loaded = store.load("com.example.formatter").unwrap().unwrap();
        assert!(verify(&loaded, KEY).is_ok());
        assert_eq!(loaded.grant_set.revision, 1);
    }

    #[test]
    fn store_load_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        assert!(store.load("com.example.nobody").unwrap().is_none());
    }

    #[test]
    fn store_rejects_empty_grants() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let mut g = gs(1);
        g.grants.clear();
        let e = store.save(&g).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("不可为空"));
    }

    #[test]
    fn store_rejects_schema_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let mut g = gs(1);
        g.schema_version = 99;
        let e = store.save(&g).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("schema_version"));
    }

    #[test]
    fn store_rejects_forbidden_permission() {
        // §2.1 禁止授予清单在授予路径上硬拒。
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let mut g = gs(1);
        g.grants = vec![GrantEntry {
            permission: "shell:allow-execute".into(),
            risk: Risk::High,
            description: "执行命令".into(),
            scoped: false,
        }];
        let e = store.save(&g).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("走 A 类"), "应给出'请走 A 类'的指引");
    }

    #[test]
    fn store_rejects_wildcard_http_scope() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let mut g = gs(1);
        g.scopes.insert(
            "http:allow-fetch".to_string(),
            Value::Array(vec![Value::String("*".into())]),
        );
        let e = store.save(&g).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("通配") || e.message.contains("全量"));
    }

    #[test]
    fn store_detects_on_disk_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        store.save(&gs(1)).unwrap();

        // 直接改盘上的 JSON：多塞一条权限。
        let path = store.path_for("com.example.formatter");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["grantSet"]["grants"].as_array_mut().unwrap().push(serde_json::json!({
            "permission": "shell:allow-execute",
            "risk": "high",
            "description": "篡改写入",
            "scoped": false
        }));
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();

        // 正常加载路径应拒绝。
        let e = store.load("com.example.formatter").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("篡改"));
        // 诊断路径能看到被篡改的内容。
        let diag = store.load_unverified("com.example.formatter").unwrap().unwrap();
        assert_eq!(diag.grant_set.grants.len(), 2);
    }

    #[test]
    fn store_rejects_garbage_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let path = store.path_for("com.example.formatter");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        let e = store.load("com.example.formatter").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("解析失败"));
    }

    #[test]
    fn to_capability_shape_matches_tauri2() {
        let dir = tempfile::tempdir().unwrap();
        let store = AclStore::new(dir.path(), KEY.to_vec());
        let sig = SignedGrantSet { grant_set: gs(1), signature: sign(&gs(1), KEY).unwrap() };
        let cap = store.to_capability(&sig);
        assert_eq!(cap["identifier"], "plugin-com.example.formatter");
        assert_eq!(cap["webviews"][0], "plugin-com.example.formatter");
        assert_eq!(cap["permissions"][0], "http:allow-fetch");
    }
}
