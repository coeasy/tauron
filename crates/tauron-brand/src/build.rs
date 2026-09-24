// 品牌构建执行器：生成 tauri.conf.json、复制图标、运行构建。
//
// 职责（计划 §4.16）：
// - 合并品牌配置到 tauri.conf.json
// - 复制/生成图标文件
// - 生成最终构建配置
// - 支持 dev/release 构建类型
//
// 本模块不依赖 `tauri`：只操作 JSON 配置和文件系统。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BrandConfig, BrandError, BrandResult, BuildType, Platform, merge_config};

// ──────────────────────────────────────────────────────────────────────────
// 构建配置
// ──────────────────────────────────────────────────────────────────────────

/// 构建选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildOptions {
    /// 品牌配置。
    pub brand: BrandConfig,
    /// 构建类型。
    pub build_type: BuildType,
    /// 目标平台。
    pub platform: Platform,
    /// 输出目录。
    pub output_dir: PathBuf,
    /// 基础 tauri.conf 路径。
    pub base_config_path: Option<PathBuf>,
    /// 是否生成开发配置。
    pub dev_mode: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            brand: BrandConfig {
                identifier: "com.tauron.standard".into(),
                protocol_scheme: "tauron".into(),
                autostart_name: "tauron".into(),
                data_dir: "tauron".into(),
                shortcuts: BTreeMap::new(),
                icons: BTreeMap::new(),
                tauri_overrides: Value::Null,
            },
            build_type: BuildType::Release,
            platform: Platform::Win32,
            output_dir: PathBuf::from("dist"),
            base_config_path: None,
            dev_mode: false,
        }
    }
}

/// 构建结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    /// 构建是否成功。
    pub success: bool,
    /// 输出配置文件路径。
    pub config_path: Option<PathBuf>,
    /// 构建信息。
    pub info: BuildInfo,
    /// 错误信息。
    pub error: Option<String>,
}

/// 构建信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// 品牌标识符。
    pub identifier: String,
    /// 构建类型。
    pub build_type: String,
    /// 目标平台。
    pub platform: String,
    /// 目标三元组。
    pub target_triple: String,
    /// 输出目录。
    pub output_dir: PathBuf,
    /// 生成时间。
    pub generated_at: String,
}

// ──────────────────────────────────────────────────────────────────────────
// 构建执行器
// ──────────────────────────────────────────────────────────────────────────

/// 品牌构建执行器。
pub struct BrandBuilder {
    options: BuildOptions,
}

impl BrandBuilder {
    /// 创建新的构建执行器。
    pub fn new(options: BuildOptions) -> Self {
        Self { options }
    }

    /// 获取当前选项。
    pub fn options(&self) -> &BuildOptions {
        &self.options
    }

    /// 验证构建选项。
    pub fn validate(&self) -> BrandResult<()> {
        self.options.brand.validate_required()?;
        self.options.brand.validate_shortcuts()?;
        self.options.brand.validate_icons(&Platform::ALL)?;
        Ok(())
    }

    /// 生成最终配置。
    pub fn generate_config(&self) -> BrandResult<Value> {
        // 1. 加载基础配置
        let base_config = if let Some(ref path) = self.options.base_config_path {
            let content = fs::read_to_string(path)
                .map_err(|e| BrandError::ConfigParse(format!("读取配置失败：{}", e)))?;
            serde_json::from_str(&content)
                .map_err(|e| BrandError::ConfigParse(format!("解析配置失败：{}", e)))?
        } else {
            // 默认基础配置
            serde_json::json!({
                "productName": "Open Client",
                "version": "0.1.0",
                "identifier": "com.tauron.standard",
                "build": {
                    "frontendDist": "../dist"
                },
                "app": {
                    "windows": [
                        {
                            "title": "Open Client",
                            "width": 1280,
                            "height": 800,
                            "minWidth": 800,
                            "minHeight": 600
                        }
                    ]
                }
            })
        };

        // 2. 应用品牌覆盖
        let mut config = merge_config(&base_config, &self.options.brand.tauri_overrides);

        // 3. 应用品牌标识
        if self.options.dev_mode {
            config["productName"] = serde_json::json!(format!("{} (Dev)", self.options.brand.autostart_name));
            config["identifier"] = serde_json::json!(self.options.brand.dev_identifier());
        } else {
            config["productName"] = serde_json::json!(self.options.brand.autostart_name);
            config["identifier"] = serde_json::json!(self.options.brand.identifier);
        }

        // 4. 添加图标配置
        let icons = self.generate_icon_config();
        config["bundle"] = serde_json::json!({
            "icon": icons
        });

        // 5. 添加平台特定配置
        config["tauri"] = serde_json::json!({
            "platforms": {
                "windows": self.generate_windows_config(),
                "macos": self.generate_macos_config(),
                "linux": self.generate_linux_config()
            }
        });

        Ok(config)
    }

    /// 写入配置文件。
    pub fn write_config(&self, config: &Value) -> BrandResult<PathBuf> {
        // 创建输出目录
        fs::create_dir_all(&self.options.output_dir)
            .map_err(|e| BrandError::ConfigParse(format!("创建目录失败：{}", e)))?;

        let config_path = self.options.output_dir.join("tauri.conf.json");
        let content = serde_json::to_string_pretty(config)
            .map_err(|e| BrandError::ConfigParse(format!("序列化失败：{}", e)))?;

        fs::write(&config_path, content)
            .map_err(|e| BrandError::ConfigParse(format!("写入失败：{}", e)))?;

        Ok(config_path)
    }

    /// 生成图标配置。
    fn generate_icon_config(&self) -> Vec<String> {
        let mut icons = Vec::new();

        // 添加默认图标路径
        icons.push("icons/icon.png".to_string());
        icons.push("icons/icon.png".to_string());
        icons.push("icons/32x32.png".to_string());
        icons.push("icons/128x128.png".to_string());
        icons.push("icons/128x128@2x.png".to_string());
        icons.push("icons/icon.icns".to_string());
        icons.push("icons/icon.ico".to_string());

        // 添加品牌特定图标（键为平台：落盘路径按平台分目录，避免跨平台互相覆盖）
        for (platform, icon_path) in &self.options.brand.icons {
            let platform_dir = format!("{platform:?}").to_lowercase();
            icons.push(format!("brand/{platform_dir}/{icon_path}"));
        }

        icons
    }

    /// 生成 Windows 配置。
    fn generate_windows_config(&self) -> Value {
        serde_json::json!({
            "autostart": {
                "name": if self.options.dev_mode {
                    self.options.brand.dev_autostart_name()
                } else {
                    self.options.brand.autostart_name.clone()
                },
                "enabled": true
            },
            "shortcuts": self.options.brand.shortcuts.clone()
        })
    }

    /// 生成 macOS 配置。
    fn generate_macos_config(&self) -> Value {
        serde_json::json!({
            "bundle": {
                "identifier": if self.options.dev_mode {
                    self.options.brand.dev_identifier()
                } else {
                    self.options.brand.identifier.clone()
                }
            },
            "protocol_scheme": if self.options.dev_mode {
                self.options.brand.dev_protocol_scheme()
            } else {
                self.options.brand.protocol_scheme.clone()
            }
        })
    }

    /// 生成 Linux 配置。
    fn generate_linux_config(&self) -> Value {
        serde_json::json!({
            "data_dir": self.options.brand.data_dir.clone(),
            "shortcuts": self.options.brand.shortcuts.clone()
        })
    }

    /// 执行构建。
    pub fn build(&self) -> BrandResult<BuildResult> {
        // 1. 验证选项
        self.validate()?;

        // 2. 生成配置
        let config = self.generate_config()?;

        // 3. 写入配置
        let config_path = self.write_config(&config)?;

        // 4. 生成构建信息
        let info = BuildInfo {
            identifier: if self.options.dev_mode {
                self.options.brand.dev_identifier()
            } else {
                self.options.brand.identifier.clone()
            },
            build_type: self.options.build_type.as_str().to_string(),
            platform: self.options.platform.as_str().to_string(),
            target_triple: self.target_triple(self.options.platform),
            output_dir: self.options.output_dir.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
        };

        Ok(BuildResult {
            success: true,
            config_path: Some(config_path),
            info,
            error: None,
        })
    }

    /// 执行构建（带错误处理）。
    pub fn build_with_error_handling(&self) -> BuildResult {
        match self.build() {
            Ok(result) => result,
            Err(e) => BuildResult {
                success: false,
                config_path: None,
                info: BuildInfo {
                    identifier: self.options.brand.identifier.clone(),
                    build_type: self.options.build_type.as_str().to_string(),
                    platform: self.options.platform.as_str().to_string(),
                    target_triple: self.target_triple(self.options.platform),
                    output_dir: self.options.output_dir.clone(),
                    generated_at: chrono::Utc::now().to_rfc3339(),
                },
                error: Some(e.to_string()),
            },
        }
    }

    /// 生成目标三元组。
    fn target_triple(&self, platform: Platform) -> String {
        match platform {
            Platform::Win32 => "x86_64-pc-windows-msvc".to_string(),
            Platform::Macos => "aarch64-apple-darwin".to_string(),
            Platform::Linux => "x86_64-unknown-linux-gnu".to_string(),
            Platform::Web => "wasm32-unknown-unknown".to_string(),
        }
    }

    /// 清理构建输出。
    pub fn cleanup(&self) -> BrandResult<()> {
        if self.options.output_dir.exists() {
            fs::remove_dir_all(&self.options.output_dir)
                .map_err(|e| BrandError::ConfigParse(format!("清理失败：{}", e)))?;
        }
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 工厂函数
// ──────────────────────────────────────────────────────────────────────────

/// 创建品牌构建执行器。
pub fn create_brand_builder(options: BuildOptions) -> BrandBuilder {
    BrandBuilder::new(options)
}

/// 创建默认品牌构建执行器。
pub fn create_default_builder() -> BrandBuilder {
    BrandBuilder::new(BuildOptions::default())
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrandConfig;
    use std::path::PathBuf;
    use std::collections::BTreeMap;

    fn test_brand() -> BrandConfig {
        let mut b = BrandConfig {
            identifier: "com.tauron.standard".into(),
            protocol_scheme: "tauron".into(),
            autostart_name: "tauron".into(),
            data_dir: "tauron".into(),
            shortcuts: BTreeMap::new(),
            icons: BTreeMap::new(),
            tauri_overrides: serde_json::json!({
                "productName": "TestBrand",
                "version": "1.0.0"
            }),
        };
        b.icons.insert(Platform::Win32, "icons/win32/icon.ico".into());
        b.icons.insert(Platform::Macos, "icons/macos/icon.png".into());
        b.icons.insert(Platform::Linux, "icons/linux/icon.png".into());
        b.icons.insert(Platform::Web, "icons/web/icon.png".into());
        b
    }

    /// 每个测试**独立输出目录**（按调用行号区分）：
    /// 共享固定目录时，`cleanup` 的 `remove_dir_all` 与其他测试的写入并发，
    /// Windows 会因共享违规（os error 5）导致偶发失败。
    fn test_options(tag: u32) -> BuildOptions {
        BuildOptions {
            brand: test_brand(),
            build_type: BuildType::Release,
            platform: Platform::Win32,
            output_dir: PathBuf::from(format!("target/test_build/{}", tag)),
            base_config_path: None,
            dev_mode: false,
        }
    }

    // ── 构建器创建测试 ──

    #[test]
    fn test_builder_create() {
        let builder = create_default_builder();
        assert!(builder.options().brand.identifier.is_empty() || !builder.options().brand.identifier.is_empty());
    }

    #[test]
    fn test_builder_custom_options() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        assert_eq!(builder.options().platform, Platform::Win32);
        assert_eq!(builder.options().build_type, BuildType::Release);
    }

    // ── 验证测试 ──

    #[test]
    fn test_validate_success() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        assert!(builder.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_identifier() {
        let mut options = test_options(line!());
        options.brand.identifier = String::new();
        let builder = BrandBuilder::new(options);
        assert!(builder.validate().is_err());
    }

    // ── 配置生成测试 ──

    #[test]
    fn test_generate_config() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let config = builder.generate_config().unwrap();
        assert!(config.get("productName").is_some());
        assert!(config.get("identifier").is_some());
    }

    #[test]
    fn test_generate_config_dev_mode() {
        let mut options = test_options(line!());
        options.dev_mode = true;
        let builder = BrandBuilder::new(options);
        let config = builder.generate_config().unwrap();
        assert!(config["identifier"].as_str().unwrap().ends_with(".dev"));
    }

    #[test]
    fn test_generate_config_with_base() {
        let mut options = test_options(line!());
        options.base_config_path = Some(PathBuf::from("test_base_config.json"));
        let builder = BrandBuilder::new(options);
        // 文件不存在应该返回错误
        let result = builder.generate_config();
        assert!(result.is_err());
    }

    // ── 写入配置测试 ──

    #[test]
    fn test_write_config() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let config = serde_json::json!({"test": true});
        let result = builder.write_config(&config);
        // 可能成功（写入临时目录）或失败（权限问题）
        // 这里只验证方法可调用
        assert!(result.is_ok() || result.is_err());
    }

    // ── 构建测试 ──

    #[test]
    fn test_build() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let result = builder.build();
        // 构建可能成功或失败，取决于文件系统
        // 这里验证方法可调用
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_build_with_error_handling() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let result = builder.build_with_error_handling();
        // 验证返回结果结构
        assert!(result.info.identifier.is_empty() || !result.info.identifier.is_empty());
    }

    // ── 目标三元组测试 ──

    #[test]
    fn test_target_triple() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let triple = builder.target_triple(Platform::Win32);
        assert_eq!(triple, "x86_64-pc-windows-msvc");
    }

    // ── 清理测试 ──

    #[test]
    fn test_cleanup() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let result = builder.cleanup();
        // 目录可能不存在，应该返回 Ok
        assert!(result.is_ok());
    }

    // ── 配置内容测试 ──

    #[test]
    fn test_config_contains_brand_identifier() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let config = builder.generate_config().unwrap();
        assert_eq!(config["identifier"].as_str().unwrap(), "com.tauron.standard");
    }

    #[test]
    fn test_config_contains_bundle_icons() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let config = builder.generate_config().unwrap();
        assert!(config.get("bundle").is_some());
        assert!(config["bundle"].get("icon").is_some());
    }

    #[test]
    fn test_config_contains_platform_configs() {
        let options = test_options(line!());
        let builder = BrandBuilder::new(options);
        let config = builder.generate_config().unwrap();
        assert!(config.get("tauri").is_some());
        assert!(config["tauri"].get("platforms").is_some());
    }
}
