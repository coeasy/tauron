//! A94：**真实安装流**的内存门禁探针（release CI 的 performance-size job 消费）。
//!
//! 走的生产同一条链：`cmd_registry_install_preview_as` 铸发审批令牌 →
//! `cmd_registry_install_reviewed_as` 消费令牌并提交（`read_verified_package`
//! 流式验签 → 逐 entry 流式解包 → 激活摘要 → 注册表提交）。
//!
//! 打包阶段本身也必须流式：探针在内存里拼一个 96 MiB 载荷的签名 `.tpkg`，
//! 若把大块先攒进 Vec 再写盘，VmHWM/堆高水位会被构建阶段污染，门禁就失去意义。
//!
//! 输出两路内存读数：
//! - `peakHeapKib`：计数全局分配器的高水位（跨平台，本地可断言）；
//! - `peakRssKb`：`/proc/self/status` 的 VmHWM（仅 Linux，CI performance job）。
//!
//! `payloadBytes` 大于 `contracts/performance-budgets.json` 里两个内存上界，
//! 用来证明「装得下大包、内存不随包长大」。门禁脚本
//! `scripts/check-performance-size.mjs` 断言 status / payload floor / 两路读数。
//!
//! 用法（缺省特性下本 example 是空壳，只保证 `cargo test --workspace` 能编译）：
//! `cargo run -p tauron-adapter --example install_stream_probe --features plugin-install`

#[cfg(feature = "plugin-install")]
mod alloc_probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub static PEAK: AtomicUsize = AtomicUsize::new(0);

    pub struct CountingAlloc;

    fn record(size: usize) {
        let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }

    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                record(layout.size());
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
            if !new_ptr.is_null() {
                if new_size >= layout.size() {
                    record(new_size - layout.size());
                } else {
                    LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
                }
            }
            new_ptr
        }
    }
}

#[cfg(feature = "plugin-install")]
#[global_allocator]
static PROBE_ALLOC: alloc_probe::CountingAlloc = alloc_probe::CountingAlloc;

#[cfg(feature = "plugin-install")]
mod probe {
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;

    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    const KID: &str = "probe-key";
    const PLUGIN_ID: &str = "com.install.probe";
    const BLOB_ENTRIES: usize = 3;
    const BLOB_BYTES: u64 = 32 * 1024 * 1024;
    const CHUNK_BYTES: usize = 64 * 1024;

    pub fn run() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("创建临时目录失败：{e}"))?;
        let package_dir = temp.path().join("packages");
        std::fs::create_dir_all(&package_dir).map_err(|e| format!("创建打包目录失败：{e}"))?;
        let install_root = temp.path().join("plugins");

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let (package, payload_bytes, entry_count) =
            build_signed_package(&package_dir, &signing_key)?;
        let package_bytes =
            std::fs::metadata(&package).map_err(|e| format!("读取安装包元数据失败：{e}"))?.len();

        // 与生产同一条装配函数；安装根、可信键、ACL 密钥与 A100 时间源显式给出。
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert(KID.to_string(), verifying_key.to_bytes().to_vec());
        let config = tauron_adapter::AdapterConfig {
            plugin_install_dir: Some(install_root),
            plugin_signing_keys: signing_keys,
            acl_signing_key: Some(vec![0x5a; 32]),
            ..Default::default()
        }
        .with_trusted_time_provider(std::sync::Arc::new(
            tauron_host::SystemTimeProvider::new(tauron_host::TimeTrustState::Trusted),
        ));
        let state = tauron_adapter::PluginRuntimeState::with_adapter_config(config);

        let package_path =
            package.to_str().ok_or_else(|| "安装包路径不是合法 UTF-8".to_string())?;
        let started = std::time::Instant::now();
        let preview = tauron_adapter::cmd_registry_install_preview_as(
            &tauron_adapter::Caller::MainWindow,
            &state,
            package_path,
        )
        .map_err(|e| format!("preview 失败：{}", e.message))?;
        let approved = vec!["store:allow-get".to_string()];
        let installed = tauron_adapter::cmd_registry_install_reviewed_as(
            &tauron_adapter::Caller::MainWindow,
            &state,
            package_path,
            &approved,
            &preview.review_token,
        )
        .map_err(|e| format!("install 提交失败：{}", e.message))?;
        let install_elapsed_ms = started.elapsed().as_millis() as u64;

        let report = serde_json::json!({
            "schemaVersion": 1,
            "status": "installed",
            "pluginId": installed.plugin_id,
            "payloadBytes": payload_bytes,
            "packageBytes": package_bytes,
            "entryCount": entry_count,
            "installElapsedMs": install_elapsed_ms,
            "peakRssKb": peak_rss_kb(),
            "peakHeapKib": crate::alloc_probe::PEAK.load(Ordering::Relaxed) as u64 / 1024,
        });
        println!("{report}");
        Ok(())
    }

    /// 流式拼包 + 签名：manifest / entry / ui 与既有安装夹具同形（保证 manifest
    /// 校验与 UI 激活两条腿都被走到），大载荷按 64 KiB 块生成、边写边哈希。
    fn build_signed_package(
        dir: &Path,
        signing_key: &SigningKey,
    ) -> Result<(PathBuf, u64, usize), String> {
        use zip::write::SimpleFileOptions;
        use zip::CompressionMethod;

        let package = dir.join(format!("{PLUGIN_ID}.tpkg"));
        let file = std::fs::File::create(&package).map_err(|e| format!("创建安装包失败：{e}"))?;
        let mut archive = zip::ZipWriter::new(file);
        // Stored：随机载荷不可压，省掉压缩 CPU，也让「包体 ≈ 载荷」关系简单可核。
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        let mut signed_files: Vec<tauron_market::package_signature::SignedFile> = Vec::new();
        let mut payload_bytes = 0u64;
        let manifest = serde_json::json!({
            "id": PLUGIN_ID,
            "name": "Install Stream Probe",
            "version": "1.0.0",
            "type": "js",
            "entry": { "js": "src/index.js", "ui": "index.html" },
            "permissions": ["store:allow-get"],
            "framework": ">=1.0.0, <2.0.0"
        });
        let small: [(&str, Vec<u8>); 3] = [
            (
                "manifest.json",
                serde_json::to_vec(&manifest).map_err(|e| format!("序列化 manifest 失败：{e}"))?,
            ),
            ("src/index.js", b"export const activate = () => true;".to_vec()),
            ("index.html", b"<!doctype html><html><body>probe</body></html>".to_vec()),
        ];
        for (path, bytes) in &small {
            archive.start_file(*path, options).map_err(|e| format!("写入 zip 条目失败：{e}"))?;
            archive.write_all(bytes).map_err(|e| format!("写入 zip 数据失败：{e}"))?;
            signed_files.push(tauron_market::package_signature::SignedFile {
                path: (*path).to_string(),
                size: bytes.len() as u64,
                hash: hex::encode(Sha256::digest(bytes)),
            });
            payload_bytes += bytes.len() as u64;
        }

        let mut chunk = vec![0u8; CHUNK_BYTES];
        for index in 0..BLOB_ENTRIES {
            let path = format!("data/blob-{index}.bin");
            archive
                .start_file(path.as_str(), options)
                .map_err(|e| format!("写入 zip 条目失败：{e}"))?;
            let mut hasher = Sha256::new();
            let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ (index as u64 + 1);
            let mut written = 0u64;
            while written < BLOB_BYTES {
                for slot in chunk.chunks_exact_mut(8) {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    slot.copy_from_slice(&state.to_le_bytes());
                }
                archive.write_all(&chunk).map_err(|e| format!("写入 zip 数据失败：{e}"))?;
                hasher.update(&chunk);
                written += CHUNK_BYTES as u64;
            }
            signed_files.push(tauron_market::package_signature::SignedFile {
                path,
                size: BLOB_BYTES,
                hash: hex::encode(hasher.finalize()),
            });
            payload_bytes += BLOB_BYTES;
        }
        archive.finish().map_err(|e| format!("收尾 zip 失败：{e}"))?;

        let issued_at = format_iso_utc(now_unix_secs());
        let payload = tauron_market::package_signature::signing_payload(
            "ed25519",
            KID,
            &issued_at,
            &signed_files,
        );
        let signature = signing_key.sign(&payload);
        let sidecar = serde_json::json!({
            "algorithm": "ed25519",
            "kid": KID,
            "issuedAt": issued_at,
            "signature": hex::encode(signature.to_bytes()),
            "files": signed_files,
        });
        std::fs::write(
            format!("{}.sig", package.display()),
            serde_json::to_vec(&sidecar).map_err(|e| format!("序列化 sidecar 失败：{e}"))?,
        )
        .map_err(|e| format!("写入 sidecar 失败：{e}"))?;
        Ok((package, payload_bytes, signed_files_len(&sidecar)))
    }

    fn signed_files_len(sidecar: &serde_json::Value) -> usize {
        sidecar["files"].as_array().map(|files| files.len()).unwrap_or(0)
    }

    fn now_unix_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// 反向 civil（Howard Hinnant）：把 Unix 秒格式化成 `toISOString()` 形态。
    /// 验签侧只接受这一种 UTC `Z` 编码，签名侧必须逐字产出。
    fn format_iso_utc(secs: i64) -> String {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if month <= 2 { year + 1 } else { year };
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z")
    }

    fn peak_rss_kb() -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            let status = std::fs::read_to_string("/proc/self/status").ok()?;
            let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
            line.split_whitespace().nth(1)?.parse().ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}

#[cfg(feature = "plugin-install")]
fn main() {
    if let Err(error) = probe::run() {
        eprintln!("install_stream_probe 失败：{error}");
        println!(
            "{}",
            serde_json::json!({ "schemaVersion": 1, "status": "failed", "error": error })
        );
        std::process::exit(1);
    }
}

#[cfg(not(feature = "plugin-install"))]
fn main() {
    eprintln!("install_stream_probe 需要 --features plugin-install（缺省特性下是空壳）");
    std::process::exit(2);
}
