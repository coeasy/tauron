//! 插件注册表（**legacy / 非 canonical**，R2-a）。
//!
//! ⚠️ canonical 的注册表是 `tauron_host::registry::Registry`（32 个公开方法：
//! 在途调用、TTL GC、流式句柄表、身份绑定、safemode 过滤）。本模块是旧一代实现
//! （10 个公开方法），按实测决策冻结：**不得再添加功能**（wire-gate 会比对方法集
//! 指纹）。决策依据与迁移路线见 `docs/architecture/canonical-owners.md`。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 插件状态（设计文档 §4.3）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PluginState {
    Discovered,
    Installing,
    Installed,
    Enabling,
    Enabled,
    Disabling,
    Disabled,
    Errored,
    Uninstalling,
    Upgrading,
}

/// 插件类型（4+1 形态，设计文档 §4）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Rust,
    Js,
    Process,
    Wasm,
}

/// 插件注册表条目
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub plugin_id: String,
    pub state: PluginState,
    pub plugin_type: PluginType,
    pub manifest: serde_json::Value,
}

/// 状态迁移表（表驱动，设计文档 §4.3）
pub fn transitions(state: PluginState) -> &'static [PluginState] {
    match state {
        PluginState::Discovered => &[PluginState::Installing],
        PluginState::Installing => &[PluginState::Installed, PluginState::Errored],
        PluginState::Installed => &[PluginState::Enabling, PluginState::Uninstalling],
        PluginState::Enabling => &[PluginState::Enabled, PluginState::Errored],
        PluginState::Enabled => {
            &[PluginState::Disabling, PluginState::Errored, PluginState::Upgrading]
        }
        PluginState::Disabling => &[PluginState::Disabled],
        PluginState::Disabled => &[PluginState::Enabling, PluginState::Uninstalling],
        PluginState::Errored => {
            &[PluginState::Enabling, PluginState::Uninstalling, PluginState::Upgrading]
        }
        PluginState::Uninstalling => &[],
        PluginState::Upgrading => &[PluginState::Installed, PluginState::Errored],
    }
}

/// 验证状态迁移是否合法
pub fn is_valid_transition(from: PluginState, to: PluginState) -> bool {
    transitions(from).contains(&to)
}

/// 插件注册表（设计文档 §4.4）
pub struct PluginRegistry {
    entries: HashMap<String, RegistryEntry>,
}

impl PluginRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// 注册插件
    pub fn register(
        &mut self,
        plugin_id: String,
        plugin_type: PluginType,
        manifest: serde_json::Value,
    ) -> bool {
        if self.entries.contains_key(&plugin_id) {
            return false;
        }
        self.entries.insert(
            plugin_id.clone(),
            RegistryEntry { plugin_id, state: PluginState::Discovered, plugin_type, manifest },
        );
        true
    }

    /// 迁移插件状态
    pub fn transition(&mut self, plugin_id: &str, to: PluginState) -> bool {
        let Some(entry) = self.entries.get_mut(plugin_id) else { return false };
        let from = entry.state;
        if !is_valid_transition(from, to) {
            tracing::error!(plugin_id, from = ?from, to = ?to, "Invalid transition");
            return false;
        }
        entry.state = to;
        true
    }

    /// 获取插件状态
    pub fn get_state(&self, plugin_id: &str) -> Option<PluginState> {
        self.entries.get(plugin_id).map(|e| e.state)
    }

    /// 获取插件条目
    pub fn get_entry(&self, plugin_id: &str) -> Option<&RegistryEntry> {
        self.entries.get(plugin_id)
    }

    /// 列出所有插件 ID
    pub fn list_plugin_ids(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// 注销插件
    pub fn unregister(&mut self, plugin_id: &str) -> bool {
        let Some(entry) = self.entries.get(plugin_id) else { return false };
        if entry.state != PluginState::Uninstalling && entry.state != PluginState::Disabled {
            return false;
        }
        self.entries.remove(plugin_id);
        true
    }

    /// 清空注册表
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 获取注册表大小
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_plugin() {
        let mut reg = PluginRegistry::new();
        let result = reg.register(
            "com.example.test".to_string(),
            PluginType::Js,
            serde_json::json!({"id": "com.example.test"}),
        );
        assert!(result);
        assert_eq!(reg.get_state("com.example.test"), Some(PluginState::Discovered));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_register_duplicate_rejected() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));
        let result = reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));
        assert!(!result);
    }

    #[test]
    fn test_valid_transition() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));

        assert!(reg.transition("test", PluginState::Installing));
        assert_eq!(reg.get_state("test"), Some(PluginState::Installing));

        assert!(reg.transition("test", PluginState::Installed));
        assert_eq!(reg.get_state("test"), Some(PluginState::Installed));
    }

    #[test]
    fn test_invalid_transition_rejected() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));

        // DISCOVERED cannot go directly to ENABLED
        assert!(!reg.transition("test", PluginState::Enabled));
        assert_eq!(reg.get_state("test"), Some(PluginState::Discovered));
    }

    #[test]
    fn test_uninstalling_is_terminal() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));
        reg.transition("test", PluginState::Installing);
        reg.transition("test", PluginState::Installed);
        reg.transition("test", PluginState::Uninstalling);

        // No transitions from UNINSTALLING
        assert!(transitions(PluginState::Uninstalling).is_empty());
        // Cannot transition to ENABLED from UNINSTALLING
        assert!(!reg.transition("test", PluginState::Enabled));
    }

    #[test]
    fn test_uninstall_active_plugin_rejected() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));

        // Cannot unregister a DISCOVERED plugin
        assert!(!reg.unregister("test"));
    }

    #[test]
    fn test_uninstall_disabled_plugin() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));
        reg.transition("test", PluginState::Installing);
        reg.transition("test", PluginState::Installed);
        reg.transition("test", PluginState::Enabling);
        reg.transition("test", PluginState::Enabled);
        reg.transition("test", PluginState::Disabling);
        reg.transition("test", PluginState::Disabled);

        assert!(reg.unregister("test"));
        assert!(reg.is_empty());
    }

    #[test]
    fn test_list_plugin_ids() {
        let mut reg = PluginRegistry::new();
        reg.register("plugin-a".to_string(), PluginType::Js, serde_json::json!({}));
        reg.register("plugin-b".to_string(), PluginType::Wasm, serde_json::json!({}));

        let ids = reg.list_plugin_ids();
        assert!(ids.contains(&"plugin-a".to_string()));
        assert!(ids.contains(&"plugin-b".to_string()));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_clear() {
        let mut reg = PluginRegistry::new();
        reg.register("test".to_string(), PluginType::Js, serde_json::json!({}));
        reg.clear();
        assert!(reg.is_empty());
    }

    #[test]
    fn test_transition_table_completeness() {
        // Every state should have at least one valid transition (except terminal states)
        let states = [
            PluginState::Discovered,
            PluginState::Installing,
            PluginState::Installed,
            PluginState::Enabling,
            PluginState::Enabled,
            PluginState::Disabling,
            PluginState::Disabled,
            PluginState::Errored,
            PluginState::Uninstalling,
            PluginState::Upgrading,
        ];
        for state in &states {
            if *state == PluginState::Uninstalling {
                assert!(transitions(*state).is_empty());
            } else {
                assert!(
                    !transitions(*state).is_empty(),
                    "State {:?} should have transitions",
                    state
                );
            }
        }
    }

    #[test]
    fn test_is_valid_transition() {
        assert!(is_valid_transition(PluginState::Discovered, PluginState::Installing));
        assert!(!is_valid_transition(PluginState::Discovered, PluginState::Enabled));
        assert!(is_valid_transition(PluginState::Enabled, PluginState::Disabling));
    }
}
