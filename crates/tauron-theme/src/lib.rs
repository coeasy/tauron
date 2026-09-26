//! §4.25 主题/皮肤插件（外观定制）。
//!
//! 职责：ThemeRegistry + 运行时切换 + CSS 变量生成 + 持久化。
//!
//! 设计参考 Shoelace `data-theme` + CSS 自定义属性模式：
//! - 每个主题是一组 CSS 自定义属性值；
//! - 切换主题 = 修改 `<html data-theme="dark">` + 注入 CSS 变量；
//! - 主题可被插件贡献（contributes.themes）；
//! - 持久化到 settings（用户偏好）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ──────────────────────────────────────────────────────────────────────────
// 错误
// ──────────────────────────────────────────────────────────────────────────

/// 主题错误。
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("主题 ID 为空")]
    EmptyId,
    #[error("主题名称为空")]
    EmptyName,
    #[error("主题 `{0}` 不存在")]
    NotFound(String),
    #[error("主题 `{0}` 已存在")]
    Duplicate(String),
    #[error("主题 `{0}` 是内置主题，不可注销")]
    BuiltinProtected(String),
    #[error("主题 `{0}` 是当前激活主题，不可注销")]
    ActiveProtected(String),
    #[error("CSS 变量名 `{0}` 非法（必须 -- 前缀）")]
    InvalidVariableName(String),
    #[error("JSON 解析失败：{0}")]
    JsonParse(String),
    #[error("文件读写失败：{0}")]
    Io(String),
}

pub type ThemeResult<T> = Result<T, ThemeError>;

// ──────────────────────────────────────────────────────────────────────────
// 主题定义
// ──────────────────────────────────────────────────────────────────────────

/// 单个主题定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// 主题 ID（`light` / `dark` / `com.example.mytheme`）。
    pub id: String,
    /// 显示名称。
    pub name: String,
    /// 是否深色主题。
    pub is_dark: bool,
    /// CSS 自定义属性映射（`--oc-color-primary` → `#2563eb`）。
    pub variables: BTreeMap<String, String>,
}

impl Theme {
    /// 创建新主题。
    pub fn new(id: &str, name: &str, is_dark: bool) -> Self {
        Self { id: id.to_string(), name: name.to_string(), is_dark, variables: BTreeMap::new() }
    }

    /// 设置一个 CSS 变量。
    pub fn set_variable(&mut self, name: &str, value: &str) -> ThemeResult<&mut Self> {
        if !name.starts_with("--") {
            return Err(ThemeError::InvalidVariableName(name.to_string()));
        }
        self.variables.insert(name.to_string(), value.to_string());
        Ok(self)
    }

    /// 获取 CSS 变量值。
    pub fn variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_str())
    }

    /// 校验主题有效性。
    pub fn validate(&self) -> ThemeResult<()> {
        if self.id.trim().is_empty() {
            return Err(ThemeError::EmptyId);
        }
        if self.name.trim().is_empty() {
            return Err(ThemeError::EmptyName);
        }
        for key in self.variables.keys() {
            if !key.starts_with("--") {
                return Err(ThemeError::InvalidVariableName(key.clone()));
            }
        }
        Ok(())
    }

    /// 生成 CSS 文本（`:root { --oc-*: value; }`）。
    pub fn to_css(&self) -> String {
        let mut lines = Vec::with_capacity(self.variables.len() + 2);
        lines.push(":root {".to_string());
        for (key, value) in &self.variables {
            lines.push(format!("  {key}: {value};"));
        }
        lines.push("}".to_string());
        lines.join("\n")
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 内置主题
// ──────────────────────────────────────────────────────────────────────────

/// 内置亮色主题。
pub fn builtin_light() -> Theme {
    let mut t = Theme::new("light", "亮色", false);
    for (k, v) in [
        ("--oc-color-primary", "#2563eb"),
        ("--oc-color-primary-hover", "#1d4ed8"),
        ("--oc-color-primary-active", "#1e40af"),
        ("--oc-color-danger", "#dc2626"),
        ("--oc-color-success", "#16a34a"),
        ("--oc-color-warning", "#d97706"),
        ("--oc-color-text", "#1f2937"),
        ("--oc-color-text-secondary", "#6b7280"),
        ("--oc-color-text-muted", "#9ca3af"),
        ("--oc-color-background", "#ffffff"),
        ("--oc-color-background-secondary", "#f9fafb"),
        ("--oc-color-border", "#e5e7eb"),
        ("--oc-color-border-hover", "#d1d5db"),
        ("--oc-color-disabled", "#f3f4f6"),
        ("--oc-color-disabled-text", "#9ca3af"),
        ("--oc-color-safemode", "#f59e0b"),
        ("--oc-color-focus-ring", "rgba(37, 99, 235, 0.5)"),
    ] {
        t.variables.insert(k.to_string(), v.to_string());
    }
    t
}

/// 内置暗色主题。
pub fn builtin_dark() -> Theme {
    let mut t = Theme::new("dark", "暗色", true);
    for (k, v) in [
        ("--oc-color-primary", "#3b82f6"),
        ("--oc-color-primary-hover", "#60a5fa"),
        ("--oc-color-primary-active", "#93c5fd"),
        ("--oc-color-danger", "#f87171"),
        ("--oc-color-success", "#4ade80"),
        ("--oc-color-warning", "#fbbf24"),
        ("--oc-color-text", "#f9fafb"),
        ("--oc-color-text-secondary", "#d1d5db"),
        ("--oc-color-text-muted", "#9ca3af"),
        ("--oc-color-background", "#111827"),
        ("--oc-color-background-secondary", "#1f2937"),
        ("--oc-color-border", "#374151"),
        ("--oc-color-border-hover", "#4b5563"),
        ("--oc-color-disabled", "#1f2937"),
        ("--oc-color-disabled-text", "#6b7280"),
        ("--oc-color-safemode", "#f59e0b"),
        ("--oc-color-focus-ring", "rgba(59, 130, 246, 0.5)"),
    ] {
        t.variables.insert(k.to_string(), v.to_string());
    }
    t
}

/// 内置主题列表。
pub fn builtin_themes() -> Vec<Theme> {
    vec![builtin_light(), builtin_dark()]
}

// ──────────────────────────────────────────────────────────────────────────
// 主题注册表
// ──────────────────────────────────────────────────────────────────────────

/// 主题注册表：管理所有已注册主题 + 当前激活主题。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeRegistry {
    /// 主题存储（按 ID 索引）。
    themes: BTreeMap<String, Theme>,
    /// 当前激活主题 ID。
    active_id: String,
    /// 注册表版本号。
    pub schema_version: u32,
}

impl Default for ThemeRegistry {
    /// 缺省**含内置主题**（light / dark），而不是空表。
    ///
    /// 为什么不让 `Default` 是空表：[`ThemeRegistry::active`] 的契约是"返回当前
    /// 激活主题"，空表下它只能 panic。`Default` 是外部最容易拿到的构造入口，
    /// 一个"默认构造出来就不可用"的对象不该是缺省行为——这与本仓其余 crate 对
    /// 缺省值的取向一致（缺省要可用；不可用要显式报错，而不是等调用方踩空）。
    fn default() -> Self {
        let mut themes = BTreeMap::new();
        for theme in builtin_themes() {
            themes.insert(theme.id.clone(), theme);
        }
        Self { themes, active_id: "light".to_string(), schema_version: 1 }
    }
}

impl ThemeRegistry {
    /// 创建注册表（含内置主题）。
    ///
    /// 与 [`ThemeRegistry::default`] 等价（`Default` 已含内置主题）。保留此名是
    /// 为了不改动既有调用点与"含内置"这一语义表述。
    pub fn with_builtins() -> Self {
        Self::default()
    }

    /// 注册新主题。
    pub fn register(&mut self, theme: Theme) -> ThemeResult<()> {
        theme.validate()?;
        if self.themes.contains_key(&theme.id) {
            return Err(ThemeError::Duplicate(theme.id));
        }
        self.themes.insert(theme.id.clone(), theme);
        Ok(())
    }

    /// 注销主题（内置主题不可注销）。
    pub fn unregister(&mut self, id: &str) -> ThemeResult<bool> {
        if id == "light" || id == "dark" {
            return Err(ThemeError::BuiltinProtected(id.to_string()));
        }
        if self.active_id == id {
            return Err(ThemeError::ActiveProtected(id.to_string()));
        }
        Ok(self.themes.remove(id).is_some())
    }

    /// 获取主题。
    pub fn get(&self, id: &str) -> Option<&Theme> {
        self.themes.get(id)
    }

    /// 设置激活主题。
    pub fn set_active(&mut self, id: &str) -> ThemeResult<()> {
        if !self.themes.contains_key(id) {
            return Err(ThemeError::NotFound(id.to_string()));
        }
        self.active_id = id.to_string();
        Ok(())
    }

    /// 当前激活主题。
    pub fn active(&self) -> &Theme {
        self.themes.get(&self.active_id).unwrap_or_else(|| self.themes.values().next().unwrap())
    }

    /// 当前激活主题 ID。
    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    /// 列出所有主题。
    pub fn list_all(&self) -> Vec<&Theme> {
        self.themes.values().collect()
    }

    /// 列出所有深色主题。
    pub fn list_dark(&self) -> Vec<&Theme> {
        self.themes.values().filter(|t| t.is_dark).collect()
    }

    /// 列出所有亮色主题。
    pub fn list_light(&self) -> Vec<&Theme> {
        self.themes.values().filter(|t| !t.is_dark).collect()
    }

    /// 主题数量。
    pub fn count(&self) -> usize {
        self.themes.len()
    }

    /// 生成当前激活主题的 CSS 文本。
    pub fn active_css(&self) -> String {
        self.active().to_css()
    }

    /// 生成 data-theme 属性值。
    pub fn data_theme_attribute(&self) -> String {
        format!("data-theme=\"{}\"", self.active_id)
    }

    /// 生成完整的主题 CSS 文本（所有主题，用 `:root[data-theme]` 选择器）。
    pub fn all_css(&self) -> String {
        let mut parts = Vec::with_capacity(self.themes.len());
        for theme in self.themes.values() {
            let selector = if theme.id == "light" {
                ":root".to_string()
            } else {
                format!(":root[data-theme=\"{}\"]", theme.id)
            };
            parts.push(format!(
                "{} {{\n{}\n}}",
                selector,
                theme
                    .variables
                    .iter()
                    .map(|(k, v)| format!("  {k}: {v};"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        parts.join("\n")
    }

    /// 序列化为 JSON（持久化）。
    pub fn to_json(&self) -> ThemeResult<String> {
        serde_json::to_string_pretty(self).map_err(|e| ThemeError::JsonParse(e.to_string()))
    }

    /// 从 JSON 反序列化。
    pub fn from_json(json: &str) -> ThemeResult<Self> {
        serde_json::from_str(json).map_err(|e| ThemeError::JsonParse(e.to_string()))
    }

    /// 从文件加载。
    pub fn from_file(path: &Path) -> ThemeResult<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| ThemeError::Io(e.to_string()))?;
        Self::from_json(&content)
    }

    /// 保存到文件。
    pub fn save_to_file(&self, path: &Path) -> ThemeResult<()> {
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(|e| ThemeError::Io(e.to_string()))
    }

    /// 获取主题贡献列表（用于 manifest contributes）。
    pub fn to_contributes(&self) -> Vec<ThemeContribute> {
        self.themes
            .values()
            .map(|t| ThemeContribute {
                id: t.id.clone(),
                name: t.name.clone(),
                is_dark: t.is_dark,
                preview_colors: vec![
                    t.variable("--oc-color-primary").unwrap_or("#000").to_string(),
                    t.variable("--oc-color-background").unwrap_or("#fff").to_string(),
                    t.variable("--oc-color-text").unwrap_or("#000").to_string(),
                ],
            })
            .collect()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 主题贡献声明
// ──────────────────────────────────────────────────────────────────────────

/// 主题贡献声明（用于 manifest contributes.themes）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeContribute {
    pub id: String,
    pub name: String,
    pub is_dark: bool,
    pub preview_colors: Vec<String>,
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_new() {
        let theme = Theme::new("test", "Test Theme", false);
        assert_eq!(theme.id, "test");
        assert_eq!(theme.name, "Test Theme");
        assert!(!theme.is_dark);
        assert!(theme.variables.is_empty());
    }

    #[test]
    fn test_theme_set_variable() {
        let mut theme = Theme::new("test", "Test", false);
        theme.set_variable("--oc-color-primary", "#2563eb").unwrap();
        assert_eq!(theme.variable("--oc-color-primary"), Some("#2563eb"));
    }

    #[test]
    fn test_theme_invalid_variable_name() {
        let mut theme = Theme::new("test", "Test", false);
        assert!(theme.set_variable("color-primary", "#000").is_err());
    }

    #[test]
    fn test_theme_validate_empty_id() {
        let theme = Theme { id: "".to_string(), ..Theme::new("test", "Test", false) };
        assert!(theme.validate().is_err());
    }

    #[test]
    fn test_theme_validate_empty_name() {
        let theme = Theme { name: "".to_string(), ..Theme::new("test", "Test", false) };
        assert!(theme.validate().is_err());
    }

    #[test]
    fn test_theme_to_css() {
        let mut theme = Theme::new("test", "Test", false);
        theme.set_variable("--oc-color-primary", "#2563eb").unwrap();
        theme.set_variable("--oc-color-text", "#fff").unwrap();
        let css = theme.to_css();
        assert!(css.contains(":root {"));
        assert!(css.contains("--oc-color-primary: #2563eb;"));
        assert!(css.contains("--oc-color-text: #fff;"));
        assert!(css.contains("}"));
    }

    #[test]
    fn test_builtin_themes() {
        let themes = builtin_themes();
        assert_eq!(themes.len(), 2);
        assert_eq!(themes[0].id, "light");
        assert!(!themes[0].is_dark);
        assert!(themes[0].variables.len() > 10);
        assert_eq!(themes[1].id, "dark");
        assert!(themes[1].is_dark);
    }

    #[test]
    fn test_registry_with_builtins() {
        let registry = ThemeRegistry::with_builtins();
        assert_eq!(registry.count(), 2);
        assert_eq!(registry.active_id(), "light");
    }

    /// `Default` 必须可用：缺省构造出的注册表若为空表，`active()` 会 panic
    /// （`values().next().unwrap()`）。修复前只有 `with_builtins()` 装内置主题，
    /// 而 `Default` 是外部最容易拿到的入口。
    #[test]
    fn test_default_registry_contains_builtins_so_active_is_usable() {
        let registry = ThemeRegistry::default();
        assert_eq!(registry.count(), 2, "缺省注册表应含内置主题");
        let active = registry.active(); // 修复前这一行 panic
        assert_eq!(active.id, "light");
        assert_eq!(registry.active_id(), "light");
    }

    #[test]
    fn test_registry_register() {
        let mut registry = ThemeRegistry::with_builtins();
        let theme = Theme::new("custom", "Custom Theme", false);
        registry.register(theme).unwrap();
        assert_eq!(registry.count(), 3);
        assert!(registry.get("custom").is_some());
    }

    #[test]
    fn test_registry_register_duplicate() {
        let mut registry = ThemeRegistry::with_builtins();
        let theme = Theme::new("light", "Duplicate", false);
        assert!(registry.register(theme).is_err());
    }

    #[test]
    fn test_registry_set_active() {
        let mut registry = ThemeRegistry::with_builtins();
        registry.set_active("dark").unwrap();
        assert_eq!(registry.active_id(), "dark");
        assert!(registry.active().is_dark);
    }

    #[test]
    fn test_registry_set_active_not_found() {
        let mut registry = ThemeRegistry::with_builtins();
        assert!(registry.set_active("nonexistent").is_err());
    }

    #[test]
    fn test_registry_list_all() {
        let registry = ThemeRegistry::with_builtins();
        let all = registry.list_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_registry_list_dark() {
        let registry = ThemeRegistry::with_builtins();
        let dark = registry.list_dark();
        assert_eq!(dark.len(), 1);
        assert!(dark[0].is_dark);
    }

    #[test]
    fn test_registry_list_light() {
        let registry = ThemeRegistry::with_builtins();
        let light = registry.list_light();
        assert_eq!(light.len(), 1);
        assert!(!light[0].is_dark);
    }

    #[test]
    fn test_registry_active_css() {
        let mut registry = ThemeRegistry::with_builtins();
        registry.set_active("dark").unwrap();
        let css = registry.active_css();
        assert!(css.contains("--oc-color-background: #111827;"));
    }

    #[test]
    fn test_registry_all_css() {
        let registry = ThemeRegistry::with_builtins();
        let css = registry.all_css();
        assert!(css.contains(":root {"));
        assert!(css.contains(":root[data-theme=\"dark\"] {"));
    }

    #[test]
    fn test_registry_data_theme_attribute() {
        let mut registry = ThemeRegistry::with_builtins();
        registry.set_active("dark").unwrap();
        assert_eq!(registry.data_theme_attribute(), "data-theme=\"dark\"");
    }

    #[test]
    fn test_registry_json_roundtrip() {
        let registry = ThemeRegistry::with_builtins();
        let json = registry.to_json().unwrap();
        let restored = ThemeRegistry::from_json(&json).unwrap();
        assert_eq!(restored.count(), 2);
        assert_eq!(restored.active_id(), "light");
        assert!(restored.get("dark").is_some());
    }

    #[test]
    fn test_registry_to_contributes() {
        let registry = ThemeRegistry::with_builtins();
        let contributes = registry.to_contributes();
        assert_eq!(contributes.len(), 2);
        // BTreeMap 按 key 排序：dark < light
        assert_eq!(contributes[0].id, "dark");
        assert!(contributes[0].is_dark);
        assert_eq!(contributes[1].id, "light");
        assert!(!contributes[1].is_dark);
        assert_eq!(contributes[0].preview_colors.len(), 3);
    }

    #[test]
    fn test_registry_file_roundtrip() {
        let registry = ThemeRegistry::with_builtins();
        let tmp = std::env::temp_dir().join("oc-theme-test.json");
        registry.save_to_file(&tmp).unwrap();
        let restored = ThemeRegistry::from_file(&tmp).unwrap();
        assert_eq!(restored.count(), 2);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_registry_unregister_builtin() {
        let mut registry = ThemeRegistry::with_builtins();
        assert!(registry.unregister("light").is_err());
        assert!(registry.unregister("dark").is_err());
    }

    #[test]
    fn test_registry_unregister_active() {
        let mut registry = ThemeRegistry::with_builtins();
        let theme = Theme::new("custom", "Custom", false);
        registry.register(theme).unwrap();
        registry.set_active("custom").unwrap();
        assert!(registry.unregister("custom").is_err());
    }

    #[test]
    fn test_registry_unregister_custom() {
        let mut registry = ThemeRegistry::with_builtins();
        let theme = Theme::new("custom", "Custom", false);
        registry.register(theme).unwrap();
        assert!(registry.unregister("custom").unwrap());
        assert_eq!(registry.count(), 2);
    }

    #[test]
    fn test_builtin_light_variables() {
        let light = builtin_light();
        assert!(light.variable("--oc-color-primary").is_some());
        assert!(light.variable("--oc-color-background").is_some());
        assert!(light.variable("--oc-color-text").is_some());
        assert!(light.variable("--oc-color-focus-ring").is_some());
    }

    #[test]
    fn test_builtin_dark_variables() {
        let dark = builtin_dark();
        assert!(dark.variable("--oc-color-primary").is_some());
        assert!(dark.variable("--oc-color-background").is_some());
        assert!(dark.variable("--oc-color-text").is_some());
        assert!(dark.variable("--oc-color-focus-ring").is_some());
        let light = builtin_light();
        // 暗色主题的 primary 应该与亮色不同
        assert_ne!(light.variable("--oc-color-primary"), dark.variable("--oc-color-primary"));
    }

    #[test]
    fn test_theme_contribute_serialize() {
        let contribute = ThemeContribute {
            id: "test".to_string(),
            name: "Test".to_string(),
            is_dark: true,
            preview_colors: vec!["#000".to_string(), "#fff".to_string(), "#888".to_string()],
        };
        let json = serde_json::to_string(&contribute).unwrap();
        let restored: ThemeContribute = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "test");
        assert!(restored.is_dark);
    }

    #[test]
    fn test_registry_active_fallback() {
        // 当 active_id 不在 themes 中时，应回退到第一个主题
        let mut registry =
            ThemeRegistry { active_id: "nonexistent".to_string(), ..ThemeRegistry::default() };
        // 没有主题时 active() 会 panic，所以我们测试有主题但 active_id 不匹配
        let theme = Theme::new("custom", "Custom", false);
        registry.register(theme).unwrap();
        // active() 应该回退到第一个
        assert_eq!(registry.active().id, "custom");
    }
}
