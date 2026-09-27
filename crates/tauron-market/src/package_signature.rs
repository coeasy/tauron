//! Ed25519 verification for the `.tpkg.sig` envelope emitted by `tauron pack --sign`.
//!
//! # 签名载荷 v2（1.0-W6 修复 P0-8）
//!
//! 载荷是 UTF-8 的**规范化文本**，由 [`signing_payload`] 单一生成，
//! 签名侧（`@tauron/app-cli` 的 `signPayload`）与验签侧共用同一份定义：
//!
//! ```text
//! tauron-tpkg-sig-v2
//! algorithm=<algorithm>
//! kid=<kid>
//! issuedAt=<issuedAt>
//! <path>\t<size>\t<hash>      ← 每个文件一行，顺序即 sidecar 声明顺序
//! ```
//!
//! **v1 的缺陷**：旧载荷只拼接文件哈希，`algorithm` / `kid` / `issuedAt`
//! 仅做格式校验、**未进签名**——`issuedAt` 可被改写以绕过有效期，
//! `kid`（指向哪把密钥）也不受签名保护。v2 把元数据与 `path`/`size` 一并纳入。
//!
//! 路径不得含 `\t` / `\n` / `\r`（会与分隔符歧义），验签与签名两侧都拒绝。

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauron_host::manifest::{embedded_permission_index, PluginManifest};

use crate::{
    error::{MarketError, MarketResult},
    sanitize_entry_path, validate_zip_constants, ZipEntryInfo, MAX_ENTRIES, MAX_SINGLE_FILE_MB,
    MAX_UNPACKED_MB,
};

const SPKI_ED25519_PREFIX: [u8; 12] =
    [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

/// 签名有效期上限（天）。超过即判定过期——默认 10 年，足以覆盖长期发布，
/// 但能挡住"签发时间被改成任意值"这类篡改（配合下面的时钟偏移校验）。
pub const MAX_SIGNATURE_AGE_DAYS: i64 = 3650;

/// 允许的时钟偏移（秒）：签发时间不得比本机时间超前超过该值。
pub const MAX_CLOCK_SKEW_SECS: i64 = 24 * 3600;

/// 载荷版本头（两侧必须逐字一致）。
pub const SIGNING_PAYLOAD_HEADER: &str = "tauron-tpkg-sig-v2";

/// 构造**规范化签名载荷 v2**（签名与验签的单一事实源）。
///
/// 两侧（Rust 验签 / TS 签名）必须产生**逐字节相同**的输出：
/// 行分隔 `\n`、键值用 `=`、文件行用 `\t`、末尾带换行。
pub fn signing_payload(
    algorithm: &str,
    kid: &str,
    issued_at: &str,
    files: &[SignedFile],
) -> Vec<u8> {
    let mut out = String::with_capacity(64 + files.len() * 96);
    out.push_str(SIGNING_PAYLOAD_HEADER);
    out.push('\n');
    out.push_str("algorithm=");
    out.push_str(algorithm);
    out.push('\n');
    out.push_str("kid=");
    out.push_str(kid);
    out.push('\n');
    out.push_str("issuedAt=");
    out.push_str(issued_at);
    out.push('\n');
    for file in files {
        out.push_str(&file.path);
        out.push('\t');
        out.push_str(&file.size.to_string());
        out.push('\t');
        out.push_str(&file.hash);
        out.push('\n');
    }
    out.into_bytes()
}

/// 路径是否可用于签名载荷（不含会与分隔符歧义的字符）。
pub fn path_is_payload_safe(path: &str) -> bool {
    !path.is_empty() && !path.contains(['\t', '\n', '\r'])
}

/// 解析 `YYYY-MM-DDTHH:MM:SS[.fff]Z`（`Date.prototype.toISOString()` 的形态）
/// 为 Unix 秒。刻意只接受 UTC `Z` 形态：签名侧的 `toISOString()` 恒产出该形态，
/// 接受其他偏移会引入"同一时刻多种编码"的载荷歧义。
pub fn parse_issued_at_utc_secs(input: &str) -> Option<i64> {
    let s = input.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // 小数秒（可选）必须被 `Z` 收尾。
    let rest = s.get(19..)?;
    let rest = match rest.strip_prefix('.') {
        Some(frac) => {
            let digits = frac.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                return None;
            }
            frac.get(digits..)?
        }
        None => rest,
    };
    if rest != "Z" {
        return None;
    }
    // days_from_civil（Howard Hinnant 算法）：无依赖地把日期换算成天数。
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageSignature {
    pub algorithm: String,
    pub kid: String,
    pub issued_at: String,
    pub signature: String,
    pub files: Vec<SignedFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedFile {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

/// Verify a CLI archive and its sidecar, including archive metadata and each actual file hash.
pub fn verify_tpkg(
    archive: &[u8],
    sidecar_json: &str,
    trusted_public_key: &[u8],
) -> MarketResult<(PackageSignature, PluginManifest)> {
    use std::io::Read;
    let signature = verify_package_signature(sidecar_json, trusted_public_key)?;
    let cursor = std::io::Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(cursor)
        .map_err(|e| MarketError::ManifestFormat(format!("ZIP 解析失败：{e}")))?;
    if zip.is_empty() || zip.len() > MAX_ENTRIES || zip.len() != signature.files.len() {
        return Err(MarketError::ManifestFormat("ZIP 条目数与签名文件清单不一致".into()));
    }
    let mut total_unpacked = 0u64;
    let mut manifest_bytes = None;
    let mut seen = std::collections::HashSet::with_capacity(zip.len());
    // 中央目录元数据留一份，循环后交给 `validate_zip_constants` 做**聚合策略**
    // 断言（条目数 / 解压总量 / 单文件 / 压缩比）。循环里那几条同形检查是
    // **提前退出**用的——必须在读字节之前判，否则一个 200MB+ 的包会先被整个
    // 读进内存再拒绝。两者不是重复实现：一处是"早退"，一处是"策略单一真相源"。
    let mut metas: Vec<ZipEntryInfo> = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let mut file = zip
            .by_index(index)
            .map_err(|e| MarketError::ManifestFormat(format!("读取 ZIP 条目失败：{e}")))?;
        let name = file.name().to_string();
        if !seen.insert(name.clone()) {
            return Err(MarketError::ManifestFormat(format!("ZIP 路径重复：{name}")));
        }
        let mode = file.unix_mode().unwrap_or(0);
        let is_symlink = mode & 0o170000 == 0o120000;
        let is_regular = mode == 0 || mode & 0o170000 == 0o100000;
        let entry = ZipEntryInfo {
            name: name.clone(),
            is_symlink,
            is_file: is_regular,
            uncompressed_size: file.size(),
            compressed_size: file.compressed_size(),
        };
        sanitize_entry_path(&name, &entry)?;
        total_unpacked = total_unpacked
            .checked_add(file.size())
            .ok_or_else(|| MarketError::ManifestFormat("ZIP 解压大小溢出".into()))?;
        if total_unpacked > MAX_UNPACKED_MB * 1024 * 1024 {
            return Err(MarketError::UnpackedSizeExceeded {
                actual_mb: total_unpacked / (1024 * 1024),
                limit_mb: MAX_UNPACKED_MB,
            });
        }
        if file.size() >= MAX_SINGLE_FILE_MB * 1024 * 1024 {
            return Err(MarketError::FileTooLarge {
                name,
                size_mb: file.size() / (1024 * 1024),
                limit_mb: MAX_SINGLE_FILE_MB,
            });
        }
        metas.push(entry);
        let expected = signature
            .files
            .iter()
            .find(|signed| signed.path == name)
            .ok_or_else(|| MarketError::ManifestFormat(format!("ZIP 文件未被签名：{name}")))?;
        if expected.size != file.size() {
            return Err(MarketError::ManifestFormat(format!("文件大小与签名不符：{name}")));
        }
        let limit = expected
            .size
            .checked_add(1)
            .ok_or_else(|| MarketError::ManifestFormat("文件大小溢出".into()))?;
        let mut bytes = Vec::with_capacity(usize::try_from(expected.size).unwrap_or(0));
        (&mut file)
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|e| MarketError::ManifestFormat(format!("读取文件失败 `{name}`：{e}")))?;
        if bytes.len() as u64 != expected.size {
            return Err(MarketError::ManifestFormat(format!("解压文件大小不符：{name}")));
        }
        let actual = hex_lower(&Sha256::digest(&bytes));
        if actual != expected.hash {
            return Err(MarketError::HashMismatch { expected: expected.hash.clone(), actual });
        }
        if name == "manifest.json" {
            manifest_bytes = Some(bytes);
        }
    }
    // 聚合策略断言：这是 `validate_zip_constants` 的**唯一**生产调用点。
    // 它此前"有实现、有测试、零调用"——循环里的早退检查与它同形，于是没人发现
    // 它从未被调用；而它独有的**压缩比**检查（`MAX_COMPRESSION_RATIO`）也就
    // 从未生效过。放在这里而不是循环里：它是策略，不是早退。
    validate_zip_constants(&metas)?;
    let manifest_bytes = manifest_bytes
        .ok_or_else(|| MarketError::ManifestFormat("包内缺少 manifest.json".into()))?;
    let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| MarketError::ManifestFormat(format!("manifest.json 无效：{e}")))?;
    manifest
        .validate(&embedded_permission_index())
        .map_err(|e| MarketError::ManifestFormat(e.message))?;
    Ok((signature, manifest))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Verify a CLI signature sidecar using an explicitly trusted SPKI DER or raw Ed25519 key.
/// This function checks declared metadata and signature authenticity; callers must still
/// hash the archive's actual file bytes and compare them to `files` before installation.
///
/// 1.0-W6：载荷改为 v2（[`signing_payload`]），元数据与 `path`/`size` 一并纳入签名；
/// 并做**有效期**与**时钟偏移**校验（此前 `issuedAt` 不受签名保护、也无人校验）。
pub fn verify_package_signature(
    sidecar_json: &str,
    trusted_public_key: &[u8],
) -> MarketResult<PackageSignature> {
    let sidecar: PackageSignature = serde_json::from_str(sidecar_json)
        .map_err(|e| MarketError::ManifestFormat(e.to_string()))?;
    if sidecar.algorithm != "ed25519" {
        return Err(MarketError::SignatureInvalid("仅支持 ed25519".into()));
    }
    if sidecar.kid.trim().is_empty() || sidecar.issued_at.trim().is_empty() {
        return Err(MarketError::ManifestFormat("kid/issuedAt 不能为空".into()));
    }
    // 签发时间：必须是受支持的 UTC 形态，且落在 [now - MAX_AGE, now + SKEW]。
    // 这两条校验只有在 `issuedAt` **进了签名**（v2）之后才有意义——v1 里它可以
    // 被随意改写，校验等于没校验。
    let issued = parse_issued_at_utc_secs(&sidecar.issued_at).ok_or_else(|| {
        MarketError::ManifestFormat(format!(
            "issuedAt 必须是 `YYYY-MM-DDTHH:MM:SS[.fff]Z` 形态的 UTC 时间：{}",
            sidecar.issued_at
        ))
    })?;
    let now = now_epoch_secs();
    if issued > now + MAX_CLOCK_SKEW_SECS {
        return Err(MarketError::SignatureInvalid(format!(
            "签发时间 `{}` 在未来（超出允许的时钟偏移 {} 秒）",
            sidecar.issued_at, MAX_CLOCK_SKEW_SECS
        )));
    }
    if now - issued > MAX_SIGNATURE_AGE_DAYS * 86400 {
        return Err(MarketError::SignatureInvalid(format!(
            "签名已过期（签发于 `{}`，超过 {MAX_SIGNATURE_AGE_DAYS} 天）",
            sidecar.issued_at
        )));
    }
    if sidecar.files.len() > MAX_ENTRIES {
        return Err(MarketError::EntryCountExceeded {
            actual: sidecar.files.len(),
            limit: MAX_ENTRIES,
        });
    }
    if sidecar.files.is_empty() {
        return Err(MarketError::ManifestFormat("签名文件清单不能为空".into()));
    }

    let mut seen_paths = std::collections::HashSet::with_capacity(sidecar.files.len());
    for file in &sidecar.files {
        if !seen_paths.insert(file.path.as_str()) {
            return Err(MarketError::ManifestFormat(format!("签名文件路径重复：{}", file.path)));
        }
        if file.path == "manifest.json.sig" {
            return Err(MarketError::ManifestFormat("签名 sidecar 不能自引用".into()));
        }
        // 路径含 `\t` / `\n` / `\r` 会让载荷行产生歧义（分隔符注入）——拒绝。
        if !path_is_payload_safe(&file.path) {
            return Err(MarketError::ManifestFormat(format!(
                "签名文件路径含控制字符（\\t / \\n / \\r）：{}",
                file.path
            )));
        }
        let entry = ZipEntryInfo {
            name: file.path.clone(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: file.size,
            compressed_size: file.size,
        };
        sanitize_entry_path(&file.path, &entry)?;
        if file.size >= MAX_SINGLE_FILE_MB * 1024 * 1024 {
            return Err(MarketError::FileTooLarge {
                name: file.path.clone(),
                size_mb: file.size / (1024 * 1024),
                limit_mb: MAX_SINGLE_FILE_MB,
            });
        }
        if file.hash.len() != 64
            || !file.hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(MarketError::ManifestFormat(format!(
                "文件 `{}` 的 SHA-256 必须为 64 位小写十六进制",
                file.path
            )));
        }
    }

    // v2 载荷：元数据 + 逐文件的 path/size/hash（顺序敏感）。
    let payload =
        signing_payload(&sidecar.algorithm, &sidecar.kid, &sidecar.issued_at, &sidecar.files);

    let signature_bytes = decode_hex(&sidecar.signature)
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(|| MarketError::SignatureInvalid("签名必须为 64 字节十六进制".into()))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|e| MarketError::SignatureInvalid(e.to_string()))?;
    let key_bytes = if trusted_public_key.len() == 32 {
        trusted_public_key
    } else if trusted_public_key.len() == 44 && trusted_public_key[..12] == SPKI_ED25519_PREFIX {
        &trusted_public_key[12..]
    } else {
        return Err(MarketError::SignatureInvalid(
            "公钥必须是 32 字节原始密钥或 Ed25519 SPKI DER".into(),
        ));
    };
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| MarketError::SignatureInvalid("Ed25519 公钥长度无效".into()))?;
    let key = VerifyingKey::from_bytes(&key_array)
        .map_err(|e| MarketError::SignatureInvalid(e.to_string()))?;
    key.verify_strict(&payload, &signature)
        .map_err(|e| MarketError::SignatureInvalid(e.to_string()))?;
    Ok(sidecar)
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    // 保留 chunks_exact：clippy 建议的 `as_chunks::<2>()` 依赖不稳定 API，不为此引入不稳定特性。
    #[allow(clippy::chunks_exact_to_as_chunks)]
    for pair in input.as_bytes().chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY_SPKI_HEX: &str =
        "302a300506032b6570032100d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    /// v2 载荷（元数据 + path/size/hash）在 RFC 8032 测试密钥下的签名。
    /// 由 `signing_payload` 的定义唯一决定——载荷一旦改动，本向量必须同步重算。
    const SIGNATURE: &str = "9e977069d87a4c0e0c1b7acde76e2e47e2f71d756551f558c5633c75a387a8730b577a6a3eedfa01758859dc8ab18762dc6a6316a2ce25d94544562c88b83706";

    fn fixture() -> String {
        format!(
            r#"{{"algorithm":"ed25519","kid":"kid-rfc8032","issuedAt":"2026-09-26T00:00:00.000Z","signature":"{SIGNATURE}","files":[{{"path":"manifest.json","size":128,"hash":"{}"}},{{"path":"dist/index.js","size":2048,"hash":"{}"}}]}}"#,
            "aa".repeat(32),
            "bb".repeat(32)
        )
    }

    #[test]
    fn verifies_cli_fixed_file_vector_with_spki_and_raw_key() {
        let spki = decode_hex(PUBLIC_KEY_SPKI_HEX).unwrap();
        let result = verify_package_signature(&fixture(), &spki).unwrap();
        assert_eq!(result.kid, "kid-rfc8032");
        assert!(verify_package_signature(&fixture(), &spki[12..]).is_ok());
    }

    #[test]
    fn rejects_tampering_bad_paths_and_unknown_fields() {
        let spki = decode_hex(PUBLIC_KEY_SPKI_HEX).unwrap();
        let tampered = fixture().replace(&"aa".repeat(32), &format!("{}ab", "aa".repeat(31)));
        assert!(matches!(
            verify_package_signature(&tampered, &spki),
            Err(MarketError::SignatureInvalid(_))
        ));
        let traversal = fixture().replace("manifest.json", "../manifest.json");
        assert!(matches!(
            verify_package_signature(&traversal, &spki),
            Err(MarketError::PathTraversal(_))
        ));
        let unknown = fixture().replace("\"kid\":", "\"extra\":true,\"kid\":");
        assert!(matches!(
            verify_package_signature(&unknown, &spki),
            Err(MarketError::ManifestFormat(_))
        ));
    }

    // ── 1.0-W6：元数据必须进签名 + 有效期/时钟偏移 ──────────────────────

    /// 载荷的**形状**是两侧契约：版本头 + 三行元数据 + 每文件一行 `path\tsize\thash`。
    #[test]
    fn signing_payload_shape_is_canonical() {
        let files = vec![
            SignedFile { path: "manifest.json".into(), size: 128, hash: "a".repeat(64) },
            SignedFile { path: "dist/i.js".into(), size: 7, hash: "b".repeat(64) },
        ];
        let payload = signing_payload("ed25519", "kid-1", "2026-09-26T00:00:00.000Z", &files);
        let text = String::from_utf8(payload).unwrap();
        assert_eq!(
            text,
            format!(
                "tauron-tpkg-sig-v2\nalgorithm=ed25519\nkid=kid-1\nissuedAt=2026-09-26T00:00:00.000Z\nmanifest.json\t128\t{}\ndist/i.js\t7\t{}\n",
                "a".repeat(64),
                "b".repeat(64)
            )
        );
    }

    /// **门禁（P0-8）**：篡改 `issuedAt` 必须让验签失败——v1 里它不在签名内，
    /// 改它验签照样通过（这正是"可绕过有效期"的成因）。
    #[test]
    fn tampering_issued_at_breaks_verification() {
        let spki = decode_hex(PUBLIC_KEY_SPKI_HEX).unwrap();
        let rewritten = fixture().replace("2026-09-26T00:00:00.000Z", "2026-09-25T00:00:00.000Z");
        assert!(matches!(
            verify_package_signature(&rewritten, &spki),
            Err(MarketError::SignatureInvalid(_))
        ));
    }

    /// 篡改 `kid`（指向哪把密钥）同样必须失败。
    #[test]
    fn tampering_kid_breaks_verification() {
        let spki = decode_hex(PUBLIC_KEY_SPKI_HEX).unwrap();
        let rewritten = fixture().replace("kid-rfc8032", "kid-other");
        assert!(matches!(
            verify_package_signature(&rewritten, &spki),
            Err(MarketError::SignatureInvalid(_))
        ));
    }

    /// 篡改文件 `size` 也必须失败（v1 的载荷不含 size，改 size 不受影响）。
    #[test]
    fn tampering_file_size_breaks_verification() {
        let spki = decode_hex(PUBLIC_KEY_SPKI_HEX).unwrap();
        let rewritten = fixture().replace("\"size\":128", "\"size\":129");
        assert!(matches!(
            verify_package_signature(&rewritten, &spki),
            Err(MarketError::SignatureInvalid(_))
        ));
    }

    /// 过期签名必须被拒（用远超上限的过去时间构造，签名由同一次调用现算）。
    #[test]
    fn expired_issued_at_is_rejected() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let spki = key.verifying_key().to_bytes();
        let files =
            vec![SignedFile { path: "manifest.json".into(), size: 1, hash: "c".repeat(64) }];
        let issued = "2000-01-01T00:00:00.000Z"; // 远早于上限
        let payload = signing_payload("ed25519", "kid-x", issued, &files);
        let sig = ed25519_dalek::Signer::sign(&key, &payload);
        let sidecar = format!(
            r#"{{"algorithm":"ed25519","kid":"kid-x","issuedAt":"{issued}","signature":"{}","files":[{{"path":"manifest.json","size":1,"hash":"{}"}}]}}"#,
            hex_lower(&sig.to_bytes()),
            "c".repeat(64)
        );
        assert!(matches!(
            verify_package_signature(&sidecar, &spki),
            Err(MarketError::SignatureInvalid(_))
        ));
    }

    /// 签发时间在未来（超出允许偏移）必须被拒。
    #[test]
    fn future_issued_at_is_rejected() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let spki = key.verifying_key().to_bytes();
        let files =
            vec![SignedFile { path: "manifest.json".into(), size: 1, hash: "d".repeat(64) }];
        let issued = "2099-01-01T00:00:00.000Z";
        let payload = signing_payload("ed25519", "kid-y", issued, &files);
        let sig = ed25519_dalek::Signer::sign(&key, &payload);
        let sidecar = format!(
            r#"{{"algorithm":"ed25519","kid":"kid-y","issuedAt":"{issued}","signature":"{}","files":[{{"path":"manifest.json","size":1,"hash":"{}"}}]}}"#,
            hex_lower(&sig.to_bytes()),
            "d".repeat(64)
        );
        assert!(matches!(
            verify_package_signature(&sidecar, &spki),
            Err(MarketError::SignatureInvalid(_))
        ));
    }

    /// 时间解析：接受 `toISOString()` 形态，拒绝其他偏移/形态（避免载荷歧义）。
    #[test]
    fn issued_at_parsing_accepts_only_utc_z_form() {
        let epoch = parse_issued_at_utc_secs("1970-01-01T00:00:00.000Z").unwrap();
        assert_eq!(epoch, 0);
        assert_eq!(
            parse_issued_at_utc_secs("2026-09-26T00:00:00Z").unwrap(),
            parse_issued_at_utc_secs("2026-09-26T00:00:00.000Z").unwrap()
        );
        assert!(parse_issued_at_utc_secs("2026-09-26T00:00:00+08:00").is_none());
        assert!(parse_issued_at_utc_secs("2026-09-26").is_none());
        assert!(parse_issued_at_utc_secs("").is_none());
    }

    /// 路径里的控制字符会让载荷行歧义（分隔符注入）——必须拒绝。
    #[test]
    fn path_with_control_chars_is_rejected() {
        assert!(path_is_payload_safe("dist/index.js"));
        assert!(!path_is_payload_safe("dist/evil\tname.js"));
        assert!(!path_is_payload_safe("dist/evil\nname.js"));
        assert!(!path_is_payload_safe(""));
    }
}
