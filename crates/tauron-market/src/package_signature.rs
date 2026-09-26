//! Ed25519 verification for the `.tpkg.sig` envelope emitted by `tauron pack --sign`.
//! The signed message is the UTF-8 concatenation of each file's lowercase SHA-256 hex,
//! in the sidecar's declared file order (the CLI wire format).

use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauron_host::manifest::{embedded_permission_index, PluginManifest};

use crate::{
    error::{MarketError, MarketResult},
    sanitize_entry_path, ZipEntryInfo, MAX_ENTRIES, MAX_SINGLE_FILE_MB, MAX_UNPACKED_MB,
};

const SPKI_ED25519_PREFIX: [u8; 12] =
    [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageSignature {
    pub algorithm: String,
    pub kid: String,
    pub issued_at: String,
    pub signature: String,
    pub files: Vec<SignedFile>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
    if zip.len() == 0 || zip.len() > MAX_ENTRIES || zip.len() != signature.files.len() {
        return Err(MarketError::ManifestFormat("ZIP 条目数与签名文件清单不一致".into()));
    }
    let mut total_unpacked = 0u64;
    let mut manifest_bytes = None;
    let mut seen = std::collections::HashSet::with_capacity(zip.len());
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
    if sidecar.files.len() > MAX_ENTRIES {
        return Err(MarketError::EntryCountExceeded {
            actual: sidecar.files.len(),
            limit: MAX_ENTRIES,
        });
    }
    if sidecar.files.is_empty() {
        return Err(MarketError::ManifestFormat("签名文件清单不能为空".into()));
    }

    let mut payload = Vec::with_capacity(sidecar.files.len() * 64);
    let mut seen_paths = std::collections::HashSet::with_capacity(sidecar.files.len());
    for file in &sidecar.files {
        if !seen_paths.insert(file.path.as_str()) {
            return Err(MarketError::ManifestFormat(format!("签名文件路径重复：{}", file.path)));
        }
        if file.path == "manifest.json.sig" {
            return Err(MarketError::ManifestFormat("签名 sidecar 不能自引用".into()));
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
        payload.extend_from_slice(file.hash.as_bytes());
    }

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
    if input.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
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
    const SIGNATURE: &str = "3d74e24e341612ca59d1190f2caefed1471384e542ef0e843cbe537102e6e865b295bdb98f3ff21d8ae8aff12bcc730f49c0920f4fc8979e64868545a01aff0b";

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
}
