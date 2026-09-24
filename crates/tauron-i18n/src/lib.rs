// §4.20 i18n 桥：语言状态单一来源、前端资源加载、原生菜单热切换、
// 插件文案命名空间。
//
// 关键约束（计划 §4.20）：
// - **一份文案源同时喂两侧**（Rust 侧 + 前端侧）；
// - 切语言 → `set_locale` IPC → Rust 重建 `MenuBuilder` 并 `set_menu`；
// - **缺失键回落默认语言并计数**（可观测）；
// - 插件文案命名空间：`plugin:<id>.oc.<key>`，插件卸载时清理；
// - 文案 key 不得散落硬编码（lint）——本 crate 提供 `KEY_PREFIX` 常量。
//
// 本 crate 不依赖 `tauri`：`MenuBuilder` 由适配层根据 locale 变化重建。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod error;

pub use error::{I18nError, I18nResult};

/// 默认回退语言。
pub const DEFAULT_LOCALE: &str = "en-US";

/// 文案 key 前缀（lint 门禁检查前缀一致性）。
pub const KEY_PREFIX: &str = "oc.";

/// RTL（从右到左）语言集合。
pub const RTL_LOCALES: &[&str] = &["ar", "he", "fa", "ur"];

/// 语言代码（BCP 47 子集：language[-script[-region]]）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Locale(String);

impl Locale {
    pub fn new(code: &str) -> Self {
        Self(code.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 语言代码（如 `zh-CN` → `zh`）。
    pub fn language(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }

    /// 区域代码（如 `zh-CN` → `CN`，无区域则空串）。
    pub fn region(&self) -> &str {
        let parts: Vec<&str> = self.0.split('-').collect();
        if parts.len() >= 2 {
            parts.last().unwrap()
        } else {
            ""
        }
    }

    /// 是否为 RTL 语言。
    pub fn is_rtl(&self) -> bool {
        RTL_LOCALES.contains(&self.language())
    }

    /// 构建回退链：`zh-CN` → `zh` → `en-US`（默认）。
    /// 默认语言本身只有 `["en-US"]`（不再回退到 `en`）。
    pub fn fallback_chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        chain.push(self.0.clone());
        // 非默认语言：加入短形式。
        if self.0 != DEFAULT_LOCALE && self.0.contains('-') {
            let short = self.language().to_string();
            if !chain.contains(&short) {
                chain.push(short);
            }
        }
        // 非默认语言：加入默认语言。
        if self.0 != DEFAULT_LOCALE {
            chain.push(DEFAULT_LOCALE.to_string());
        }
        chain
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Locale {
    type Err = I18nError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(I18nError::InvalidLocale("空语言代码".into()));
        }
        if s.len() > 35 {
            return Err(I18nError::InvalidLocale(
                "语言代码过长（上限 35 字符）".into(),
            ));
        }
        Ok(Self(s.to_string()))
    }
}

/// 资源包：locale → key → 文案。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceBundle {
    pub locale: String,
    pub texts: BTreeMap<String, String>,
}

impl ResourceBundle {
    pub fn new(locale: &str) -> Self {
        Self {
            locale: locale.to_string(),
            texts: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: &str, text: &str) {
        self.texts.insert(key.to_string(), text.to_string());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.texts.get(key).map(|s| s.as_str())
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.texts.remove(key)
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

/// 缺失键记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingKey {
    pub key: String,
    pub locale: String,
    pub ts: u64,
}

/// i18n 引擎。
pub struct I18nEngine {
    /// 当前语言。
    locale: Locale,
    /// 资源包集合：locale → 资源包。
    bundles: BTreeMap<String, ResourceBundle>,
    /// 默认语言资源包（始终存在）。
    default_bundle: ResourceBundle,
    /// 缺失键计数：locale → key → 次数。
    missing_counts: BTreeMap<String, BTreeMap<String, u32>>,
    /// 缺失键记录（最近 N 条）。
    missing_log: Vec<MissingKey>,
    /// 缺失日志上限。
    missing_log_capacity: usize,
}

impl Default for I18nEngine {
    fn default() -> Self {
        Self::new(DEFAULT_LOCALE)
    }
}

impl I18nEngine {
    pub fn new(default_locale: &str) -> Self {
        let locale = Locale::new(default_locale);
        let default_bundle = ResourceBundle::new(default_locale);
        Self {
            locale: locale.clone(),
            bundles: BTreeMap::new(),
            default_bundle,
            missing_counts: BTreeMap::new(),
            missing_log: Vec::new(),
            missing_log_capacity: 200,
        }
    }

    /// 当前语言。
    pub fn locale(&self) -> &Locale {
        &self.locale
    }

    /// 切换语言。
    pub fn set_locale(&mut self, locale: &str) -> I18nResult<()> {
        let new_locale = Locale::new(locale);
        self.locale = new_locale;
        Ok(())
    }

    /// 当前语言是否为 RTL。
    pub fn is_rtl(&self) -> bool {
        self.locale.is_rtl()
    }

    /// 回退链。
    pub fn fallback_chain(&self) -> Vec<String> {
        self.locale.fallback_chain()
    }

    /// 添加资源包。
    pub fn add_resource_bundle(&mut self, bundle: ResourceBundle) {
        self.bundles.insert(bundle.locale.clone(), bundle);
    }

    /// 取资源包。
    pub fn get_bundle(&self, locale: &str) -> Option<&ResourceBundle> {
        self.bundles.get(locale)
    }

    /// 移除某语言的资源包（卸载时清理）。
    pub fn remove_resource_bundle(&mut self, locale: &str) -> Option<ResourceBundle> {
        self.bundles.remove(locale)
    }

    /// **插件文案命名空间**：`plugin:<id>.oc.<key>`。
    pub fn plugin_key(plugin_id: &str, key: &str) -> String {
        format!("plugin:{plugin_id}.{KEY_PREFIX}{key}")
    }

    /// 插件卸载时清理其全部文案。
    pub fn cleanup_plugin(&mut self, plugin_id: &str) -> usize {
        let prefix = format!("plugin:{plugin_id}.");
        let mut removed = 0;
        // 从所有资源包中删除该前缀的 key。
        for bundle in self.bundles.values_mut() {
            let keys_to_remove: Vec<String> = bundle
                .texts
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect();
            for key in keys_to_remove {
                bundle.remove(&key);
                removed += 1;
            }
        }
        // 从默认包中删除。
        let default_keys: Vec<String> = self
            .default_bundle
            .texts
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in default_keys {
            self.default_bundle.remove(&key);
            removed += 1;
        }
        // 从缺失计数中删除。
        for counts in self.missing_counts.values_mut() {
            let keys_to_remove: Vec<String> = counts
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect();
            for key in keys_to_remove {
                counts.remove(&key);
            }
        }
        removed
    }

    /// 翻译 key。
    ///
    /// 回退链：当前语言 → 短形式 → 默认语言。
    /// 缺失时回落到默认语言并计数。
    pub fn t(&mut self, key: &str) -> String {
        self.t_inner(key)
    }

    /// 带参数替换的翻译。
    ///
    /// 参数格式：`{{param}}`。
    /// 例：`t_params("greeting", &[("name", "Alice")])` → `Hello, Alice!`
    pub fn t_params(&mut self, key: &str, params: &[(&str, &str)]) -> String {
        let text = self.t_inner(key);
        let mut result = text;
        for (param, value) in params {
            let placeholder = format!("{{{{{param}}}}}");
            result = result.replace(&placeholder, value);
        }
        result
    }

    fn t_inner(&mut self, key: &str) -> String {
        // 1. 当前语言。
        if let Some(bundle) = self.bundles.get(self.locale.as_str()) {
            if let Some(text) = bundle.get(key) {
                return text.to_string();
            }
        }
        // 2. 回退链中的短形式。
        for fallback in self.locale.fallback_chain().iter() {
            if fallback == self.locale.as_str() {
                continue;
            }
            if let Some(bundle) = self.bundles.get(fallback) {
                if let Some(text) = bundle.get(key) {
                    return text.to_string();
                }
            }
        }
        // 3. 默认语言。
        let default_text = self.default_bundle.get(key).map(|s| s.to_string());
        if let Some(text) = default_text {
            if self.locale.as_str() != DEFAULT_LOCALE {
                self.record_missing(key);
            }
            return text;
        }
        // 4. 全部缺失：返回 key 本身。
        self.record_missing(key);
        key.to_string()
    }

    /// 记录缺失键。
    fn record_missing(&mut self, key: &str) {
        let locale = self.locale.as_str().to_string();
        *self
            .missing_counts
            .entry(locale.clone())
            .or_default()
            .entry(key.to_string())
            .or_insert(0) += 1;
        self.missing_log.push(MissingKey {
            key: key.to_string(),
            locale: locale.clone(),
            ts: 0, // 调用方可覆盖
        });
        if self.missing_log.len() > self.missing_log_capacity {
            self.missing_log.remove(0);
        }
    }

    /// 缺失键计数：locale → key → 次数。
    pub fn missing_counts(&self) -> &BTreeMap<String, BTreeMap<String, u32>> {
        &self.missing_counts
    }

    /// 缺失键总数。
    pub fn missing_total(&self) -> usize {
        self.missing_counts
            .values()
            .flat_map(|m| m.values())
            .map(|&n| n as usize)
            .sum()
    }

    /// 最近缺失记录。
    pub fn missing_log(&self) -> &[MissingKey] {
        &self.missing_log
    }

    /// 所有已注册的语言。
    pub fn registered_locales(&self) -> Vec<String> {
        let mut locales: Vec<String> = self.bundles.keys().cloned().collect();
        locales.push(DEFAULT_LOCALE.to_string());
        locales.sort();
        locales.dedup();
        locales
    }

    /// 序列化（持久化资源包集合）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "locale": self.locale.as_str(),
            "bundles": self.bundles,
            "default_bundle": self.default_bundle,
            "missing_counts": self.missing_counts,
        })
    }

    /// 反序列化。
    pub fn from_json(v: &serde_json::Value) -> I18nResult<Self> {
        let locale = Locale::new(v["locale"].as_str().unwrap_or(DEFAULT_LOCALE));
        let bundles = serde_json::from_value(v["bundles"].clone()).map_err(|e| I18nError::BundleFormat(e.to_string()))?;
        let default_bundle = serde_json::from_value(v["default_bundle"].clone()).map_err(|e| I18nError::BundleFormat(e.to_string()))?;
        let missing_counts = serde_json::from_value(v["missing_counts"].clone()).map_err(|e| I18nError::BundleFormat(e.to_string()))?;
        Ok(Self {
            locale,
            bundles,
            default_bundle,
            missing_counts,
            missing_log: Vec::new(),
            missing_log_capacity: 200,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn default_engine() -> I18nEngine {
        let mut e = I18nEngine::new(DEFAULT_LOCALE);
        e.default_bundle.insert("oc.app.title", "Open Client");
        e.default_bundle.insert("oc.menu.file", "File");
        e.default_bundle.insert("oc.menu.edit", "Edit");
        e.default_bundle.insert("oc.menu.view", "View");
        e.default_bundle.insert("oc.menu.help", "Help");
        e.default_bundle.insert("oc.settings.title", "Settings");
        e.default_bundle.insert("oc.plugin.uninstall", "Uninstall");
        e.default_bundle
            .insert("oc.greeting", "Hello, {{name}}!");
        e
    }

    fn zh_cn_bundle() -> ResourceBundle {
        let mut b = ResourceBundle::new("zh-CN");
        b.insert("oc.app.title", "开放客户端");
        b.insert("oc.menu.file", "文件");
        b.insert("oc.menu.edit", "编辑");
        b.insert("oc.menu.view", "视图");
        b.insert("oc.menu.help", "帮助");
        b.insert("oc.settings.title", "设置");
        b.insert("oc.plugin.uninstall", "卸载");
        b.insert("oc.greeting", "你好，{{name}}！");
        b
    }

    // ── Locale ───────────────────────────────────────────────────────

    #[test]
    fn locale_language_and_region() {
        let l = Locale::new("zh-CN");
        assert_eq!(l.language(), "zh");
        assert_eq!(l.region(), "CN");
        let l2 = Locale::new("en-US");
        assert_eq!(l2.language(), "en");
        assert_eq!(l2.region(), "US");
        let l3 = Locale::new("fr");
        assert_eq!(l3.language(), "fr");
        assert_eq!(l3.region(), "");
    }

    #[test]
    fn locale_fallback_chain() {
        let l = Locale::new("zh-CN");
        let chain = l.fallback_chain();
        assert_eq!(chain, vec!["zh-CN", "zh", "en-US"]);
        let l2 = Locale::new("en-US");
        let chain2 = l2.fallback_chain();
        assert_eq!(chain2, vec!["en-US"]);
    }

    #[test]
    fn locale_rtl_detection() {
        assert!(Locale::new("ar-SA").is_rtl());
        assert!(Locale::new("he-IL").is_rtl());
        assert!(!Locale::new("zh-CN").is_rtl());
        assert!(!Locale::new("en-US").is_rtl());
    }

    #[test]
    fn locale_rejects_empty() {
        assert!(Locale::from_str("").is_err());
    }

    #[test]
    fn locale_rejects_very_long() {
        assert!(Locale::from_str(&"a".repeat(36)).is_err());
    }

    #[test]
    fn locale_serde_roundtrip() {
        let l = Locale::new("zh-CN");
        let v = serde_json::to_value(&l).unwrap();
        let l2 = serde_json::from_value::<Locale>(v).unwrap();
        assert_eq!(l, l2);
    }

    // ── 基本翻译 ─────────────────────────────────────────────────────

    #[test]
    fn t_returns_default_when_no_bundle() {
        let mut e = default_engine();
        e.set_locale("zh-CN").unwrap();
        assert_eq!(e.t("oc.app.title"), "Open Client", "无中文包时回落默认");
        assert!(e.missing_total() >= 1, "缺失应被计数");
    }

    #[test]
    fn t_returns_localized_when_bundle_exists() {
        let mut e = default_engine();
        e.add_resource_bundle(zh_cn_bundle());
        e.set_locale("zh-CN").unwrap();
        assert_eq!(e.t("oc.app.title"), "开放客户端");
        assert_eq!(e.missing_total(), 0);
    }

    #[test]
    fn t_returns_key_when_missing_everywhere() {
        let mut e = default_engine();
        e.set_locale("zh-CN").unwrap();
        assert_eq!(e.t("oc.nonexistent"), "oc.nonexistent");
        assert!(e.missing_total() >= 1);
    }

    #[test]
    fn t_fallback_to_short_form() {
        // zh-CN 没有，但 zh 有。
        let mut e = default_engine();
        let mut zh_bundle = ResourceBundle::new("zh");
        zh_bundle.insert("oc.app.title", "中文");
        e.add_resource_bundle(zh_bundle);
        e.set_locale("zh-CN").unwrap();
        assert_eq!(e.t("oc.app.title"), "中文", "回落短形式 zh");
    }

    #[test]
    fn t_default_locale_no_missing_count() {
        // 默认语言不需要计数。
        let mut e = default_engine();
        e.set_locale(DEFAULT_LOCALE).unwrap();
        e.t("oc.app.title");
        assert_eq!(e.missing_total(), 0);
    }

    // ── 参数替换 ─────────────────────────────────────────────────────

    #[test]
    fn t_params_substitutes_variables() {
        let mut e = default_engine();
        let result = e.t_params("oc.greeting", &[("name", "Alice")]);
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn t_params_localized_substitution() {
        let mut e = default_engine();
        e.add_resource_bundle(zh_cn_bundle());
        e.set_locale("zh-CN").unwrap();
        let result = e.t_params("oc.greeting", &[("name", "小明")]);
        assert_eq!(result, "你好，小明！");
    }

    #[test]
    fn t_params_unsubstituted_placeholder() {
        // 参数名不匹配时保留占位符。
        let mut e = default_engine();
        let result = e.t_params("oc.greeting", &[("wrong", "x")]);
        assert!(result.contains("{{name}}"), "未匹配参数保留占位符");
    }

    #[test]
    fn t_params_multiple_params() {
        let mut e = default_engine();
        let bundle = {
            let mut b = ResourceBundle::new(DEFAULT_LOCALE);
            b.insert("oc.greeting", "Hello {{name}}, you are {{age}}!");
            b
        };
        e.add_resource_bundle(bundle);
        let result = e.t_params(
            "oc.greeting",
            &[("name", "Bob"), ("age", "30")],
        );
        assert_eq!(result, "Hello Bob, you are 30!");
    }

    // ── 缺失键计数 ───────────────────────────────────────────────────

    #[test]
    fn missing_key_counted_per_locale() {
        let mut e = default_engine();
        e.set_locale("zh-CN").unwrap();
        e.t("oc.nonexistent1");
        e.t("oc.nonexistent2");
        e.t("oc.nonexistent1"); // 重复
        let counts = e.missing_counts();
        let zh_counts = counts.get("zh-CN").unwrap();
        assert_eq!(zh_counts["oc.nonexistent1"], 2);
        assert_eq!(zh_counts["oc.nonexistent2"], 1);
        assert_eq!(e.missing_total(), 3);
    }

    #[test]
    fn missing_log_is_bounded() {
        let mut e = default_engine();
        e.set_locale("zh-CN").unwrap();
        for i in 0..300 {
            e.t(&format!("oc.missing.{i}"));
        }
        assert!(e.missing_log().len() <= 200);
    }

    // ── 插件命名空间 ────────────────────────────────────────────────

    #[test]
    fn plugin_key_prefix() {
        assert_eq!(I18nEngine::plugin_key("p.audio", "settings"), "plugin:p.audio.oc.settings");
    }

    #[test]
    fn cleanup_plugin_removes_only_that_plugin() {
        let mut e = default_engine();
        let mut bundle = ResourceBundle::new(DEFAULT_LOCALE);
        bundle.insert("plugin:p.audio.oc.settings", "Audio Settings");
        bundle.insert("plugin:p.video.oc.settings", "Video Settings");
        bundle.insert("oc.app.title", "App");
        e.add_resource_bundle(bundle);
        let removed = e.cleanup_plugin("p.audio");
        assert_eq!(removed, 1);
        assert!(e.get_bundle(DEFAULT_LOCALE).unwrap().get("plugin:p.audio.oc.settings").is_none());
        assert!(e.get_bundle(DEFAULT_LOCALE).unwrap().get("plugin:p.video.oc.settings").is_some());
        assert!(e.get_bundle(DEFAULT_LOCALE).unwrap().get("oc.app.title").is_some());
    }

    #[test]
    fn cleanup_plugin_clears_missing_counts() {
        let mut e = default_engine();
        e.set_locale("zh-CN").unwrap();
        let key = I18nEngine::plugin_key("p.audio", "settings");
        e.t(&key); // 缺失
        assert!(e.missing_counts().get("zh-CN").is_some());
        e.cleanup_plugin("p.audio");
        let counts = e.missing_counts().get("zh-CN").unwrap();
        assert!(!counts.keys().any(|k| k.starts_with("plugin:p.audio.")));
    }

    // ── 语言切换 ─────────────────────────────────────────────────────

    #[test]
    fn set_locale_changes_current() {
        let mut e = default_engine();
        e.add_resource_bundle(zh_cn_bundle());
        assert_eq!(e.locale().as_str(), "en-US");
        e.set_locale("zh-CN").unwrap();
        assert_eq!(e.locale().as_str(), "zh-CN");
        assert_eq!(e.t("oc.app.title"), "开放客户端");
        // 切回英文。
        e.set_locale("en-US").unwrap();
        assert_eq!(e.t("oc.app.title"), "Open Client");
    }

    #[test]
    fn registered_locales_lists_all() {
        let mut e = default_engine();
        e.add_resource_bundle(zh_cn_bundle());
        let locales = e.registered_locales();
        assert!(locales.contains(&"en-US".to_string()));
        assert!(locales.contains(&"zh-CN".to_string()));
    }

    // ── RTL ────────────────────────────────────────────────────────

    #[test]
    fn engine_is_rtl_reflects_locale() {
        let mut e = default_engine();
        assert!(!e.is_rtl());
        e.set_locale("ar-SA").unwrap();
        assert!(e.is_rtl());
    }

    // ── 序列化 / 反序列化 ───────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let mut e = default_engine();
        e.add_resource_bundle(zh_cn_bundle());
        e.set_locale("zh-CN").unwrap();
        e.t("oc.nonexistent"); // 缺失
        let json = e.to_json();
        let e2 = I18nEngine::from_json(&json).unwrap();
        assert_eq!(e2.locale().as_str(), "zh-CN");
        assert!(e2.get_bundle("zh-CN").is_some());
        assert_eq!(e2.missing_total(), 1);
    }

    #[test]
    fn from_json_rejects_corrupt() {
        let json = serde_json::json!({"bundles": "not_an_object"});
        assert!(I18nEngine::from_json(&json).is_err());
    }

    // ── ResourceBundle ─────────────────────────────────────────────

    #[test]
    fn bundle_insert_and_get() {
        let mut b = ResourceBundle::new("zh-CN");
        b.insert("oc.key", "值");
        assert_eq!(b.get("oc.key"), Some("值"));
        assert_eq!(b.get("oc.other"), None);
        assert_eq!(b.len(), 1);
        assert!(!b.is_empty());
    }

    #[test]
    fn bundle_remove() {
        let mut b = ResourceBundle::new("zh-CN");
        b.insert("oc.key", "值");
        assert_eq!(b.remove("oc.key"), Some("值".to_string()));
        assert_eq!(b.remove("oc.key"), None);
        assert!(b.is_empty());
    }

    // ── 端到端 ──────────────────────────────────────────────────────

    /// 计划 §4.20 测试项：热切换矩阵（含托盘）。
    #[test]
    fn end_to_end_locale_switching() {
        let mut e = default_engine();
        e.add_resource_bundle(zh_cn_bundle());
        e.add_resource_bundle({
            let mut b = ResourceBundle::new("ja-JP");
            b.insert("oc.app.title", "オープンクライアント");
            b.insert("oc.menu.file", "ファイル");
            b
        });

        // 英文。
        e.set_locale("en-US").unwrap();
        assert_eq!(e.t("oc.app.title"), "Open Client");
        assert!(!e.is_rtl());

        // 中文。
        e.set_locale("zh-CN").unwrap();
        assert_eq!(e.t("oc.app.title"), "开放客户端");
        assert!(!e.is_rtl());

        // 日文（部分文案，其余回落英文）。
        e.set_locale("ja-JP").unwrap();
        assert_eq!(e.t("oc.app.title"), "オープンクライアント");
        assert_eq!(e.t("oc.menu.file"), "ファイル");
        assert_eq!(e.t("oc.menu.edit"), "Edit", "日文缺失回落英文");
        assert!(e.missing_total() >= 1, "缺失键被计数");

        // 阿拉伯语（RTL）。
        e.set_locale("ar-SA").unwrap();
        assert!(e.is_rtl(), "阿拉伯语是 RTL");
        assert_eq!(e.t("oc.app.title"), "Open Client", "阿拉伯语缺失回落英文");
    }

    /// 计划 §4.20 测试项：缺失键。
    #[test]
    fn end_to_end_missing_keys() {
        let mut e = default_engine();
        e.set_locale("zh-CN").unwrap();
        // 触发缺失。
        let _ = e.t("oc.nonexistent1");
        let _ = e.t("oc.nonexistent2");
        let _ = e.t("oc.nonexistent1");
        assert_eq!(e.missing_total(), 3);
        let counts = e.missing_counts().get("zh-CN").unwrap();
        assert_eq!(counts["oc.nonexistent1"], 2);
        assert_eq!(counts["oc.nonexistent2"], 1);
        // 缺失日志。
        assert!(e.missing_log().len() >= 3);
    }

    /// 计划 §4.20 测试项：RTL。
    #[test]
    fn end_to_end_rtl_matrix() {
        let mut e = default_engine();
        // RTL 语言。
        for rtl in ["ar-SA", "he-IL", "fa-IR", "ur-PK"] {
            e.set_locale(rtl).unwrap();
            assert!(e.is_rtl(), "{rtl} 应是 RTL");
        }
        // 非 RTL 语言。
        for ltr in ["en-US", "zh-CN", "ja-JP", "ko-KR"] {
            e.set_locale(ltr).unwrap();
            assert!(!e.is_rtl(), "{ltr} 不应是 RTL");
        }
    }

    /// 计划 §4.20：插件卸载后文案清理。
    #[test]
    fn end_to_end_plugin_cleanup() {
        let mut e = default_engine();
        // 多语言插件文案。
        for locale in ["en-US", "zh-CN"] {
            let mut b = ResourceBundle::new(locale);
            b.insert(
                &I18nEngine::plugin_key("p.audio", "settings"),
                if locale == "zh-CN" { "音频设置" } else { "Audio Settings" },
            );
            b.insert(
                &I18nEngine::plugin_key("p.video", "settings"),
                if locale == "zh-CN" { "视频设置" } else { "Video Settings" },
            );
            e.add_resource_bundle(b);
        }
        assert_eq!(e.get_bundle("zh-CN").unwrap().len(), 2);
        // 卸载 p.audio。
        let removed = e.cleanup_plugin("p.audio");
        assert_eq!(removed, 2, "两个语言的 p.audio 文案都应被删除");
        assert!(e.get_bundle("zh-CN").unwrap().get(
            &I18nEngine::plugin_key("p.audio", "settings")
        ).is_none());
        assert!(e.get_bundle("zh-CN").unwrap().get(
            &I18nEngine::plugin_key("p.video", "settings")
        ).is_some());
    }
}
