use std::collections::HashMap;

/// 配置层标识（设计文档 §4.6）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigLayer {
    Default,
    User,
    Plugin,
    Session,
}

/// 配置管理器（四层配置，设计文档 §4.6）
///
/// 优先级：session > plugin > user > default
pub struct ConfigManager {
    /// Layer 1: 全局默认
    defaults: HashMap<String, serde_json::Value>,
    /// Layer 2: 用户配置
    user_config: HashMap<String, serde_json::Value>,
    /// Layer 3: 插件覆盖
    plugin_overrides: HashMap<String, HashMap<String, serde_json::Value>>,
    /// Layer 4: 会话覆盖
    session_overrides: HashMap<String, serde_json::Value>,
}

impl ConfigManager {
    /// 创建空的配置管理器
    pub fn new() -> Self {
        Self {
            defaults: HashMap::new(),
            user_config: HashMap::new(),
            plugin_overrides: HashMap::new(),
            session_overrides: HashMap::new(),
        }
    }

    /// 设置默认值（Layer 1）
    pub fn set_defaults(&mut self, defaults: HashMap<String, serde_json::Value>) {
        self.defaults = defaults;
    }

    /// 设置用户配置（Layer 2）
    pub fn set_user_config(&mut self, config: HashMap<String, serde_json::Value>) {
        self.user_config = config;
    }

    /// 设置插件覆盖（Layer 3）
    pub fn set_plugin_override(
        &mut self,
        plugin_id: &str,
        overrides: HashMap<String, serde_json::Value>,
    ) {
        self.plugin_overrides.insert(plugin_id.to_string(), overrides);
    }

    /// 移除插件覆盖
    pub fn remove_plugin_override(&mut self, plugin_id: &str) {
        self.plugin_overrides.remove(plugin_id);
    }

    /// 设置会话覆盖（Layer 4）
    pub fn set_session_override(&mut self, key: &str, value: serde_json::Value) {
        self.session_overrides.insert(key.to_string(), value);
    }

    /// 移除会话覆盖
    pub fn remove_session_override(&mut self, key: &str) {
        self.session_overrides.remove(key);
    }

    /// 获取配置值（优先级：session > plugin > user > default）
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        // Layer 4: session
        if let Some(v) = self.session_overrides.get(key) {
            return Some(v);
        }

        // Layer 3: plugin overrides
        for overrides in self.plugin_overrides.values() {
            if let Some(v) = overrides.get(key) {
                return Some(v);
            }
        }

        // Layer 2: user
        if let Some(v) = self.user_config.get(key) {
            return Some(v);
        }

        // Layer 1: default
        self.defaults.get(key)
    }

    /// 获取所有配置（合并后）
    pub fn get_all(&self) -> HashMap<String, serde_json::Value> {
        let mut result = self.defaults.clone();
        result.extend(self.user_config.clone());
        for overrides in self.plugin_overrides.values() {
            result.extend(overrides.clone());
        }
        result.extend(self.session_overrides.clone());
        result
    }

    /// 清空所有配置
    pub fn clear(&mut self) {
        self.defaults.clear();
        self.user_config.clear();
        self.plugin_overrides.clear();
        self.session_overrides.clear();
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_val(v: serde_json::Value) -> serde_json::Value {
        v
    }

    #[test]
    fn test_default_get() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("theme".to_string(), json_val(serde_json::json!("dark")));
        defaults.insert("volume".to_string(), json_val(serde_json::json!(50)));
        cm.set_defaults(defaults);

        assert_eq!(cm.get("theme").unwrap(), "dark");
        assert_eq!(cm.get("volume").unwrap(), &50);
        assert!(cm.get("nonexistent").is_none());
    }

    #[test]
    fn test_user_overrides_default() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("theme".to_string(), json_val(serde_json::json!("dark")));
        cm.set_defaults(defaults);

        let mut user = HashMap::new();
        user.insert("theme".to_string(), json_val(serde_json::json!("light")));
        cm.set_user_config(user);

        assert_eq!(cm.get("theme").unwrap(), "light");
    }

    #[test]
    fn test_plugin_overrides_user() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("theme".to_string(), json_val(serde_json::json!("dark")));
        cm.set_defaults(defaults);

        let mut user = HashMap::new();
        user.insert("theme".to_string(), json_val(serde_json::json!("light")));
        cm.set_user_config(user);

        let mut plugin = HashMap::new();
        plugin.insert("theme".to_string(), json_val(serde_json::json!("blue")));
        cm.set_plugin_override("p1", plugin);

        assert_eq!(cm.get("theme").unwrap(), "blue");
    }

    #[test]
    fn test_session_overrides_all() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("theme".to_string(), json_val(serde_json::json!("dark")));
        cm.set_defaults(defaults);

        cm.set_session_override("theme", json_val(serde_json::json!("green")));

        assert_eq!(cm.get("theme").unwrap(), "green");
    }

    #[test]
    fn test_priority_chain() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("v".to_string(), json_val(serde_json::json!(50)));
        cm.set_defaults(defaults);

        let mut user = HashMap::new();
        user.insert("v".to_string(), json_val(serde_json::json!(75)));
        cm.set_user_config(user);

        let mut plugin = HashMap::new();
        plugin.insert("v".to_string(), json_val(serde_json::json!(60)));
        cm.set_plugin_override("p1", plugin);

        cm.set_session_override("v", json_val(serde_json::json!(90)));

        assert_eq!(cm.get("v").unwrap(), &90);

        cm.remove_session_override("v");
        assert_eq!(cm.get("v").unwrap(), &60);

        cm.remove_plugin_override("p1");
        assert_eq!(cm.get("v").unwrap(), &75);

        cm.set_user_config(HashMap::new());
        assert_eq!(cm.get("v").unwrap(), &50);
    }

    #[test]
    fn test_get_all_merged() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("a".to_string(), json_val(serde_json::json!(1)));
        defaults.insert("b".to_string(), json_val(serde_json::json!(2)));
        defaults.insert("c".to_string(), json_val(serde_json::json!(3)));
        cm.set_defaults(defaults);

        let mut user = HashMap::new();
        user.insert("b".to_string(), json_val(serde_json::json!(20)));
        user.insert("d".to_string(), json_val(serde_json::json!(4)));
        cm.set_user_config(user);

        let mut plugin = HashMap::new();
        plugin.insert("c".to_string(), json_val(serde_json::json!(30)));
        cm.set_plugin_override("p1", plugin);

        cm.set_session_override("d", json_val(serde_json::json!(40)));

        let all = cm.get_all();
        assert_eq!(all["a"], serde_json::json!(1));
        assert_eq!(all["b"], serde_json::json!(20));
        assert_eq!(all["c"], serde_json::json!(30));
        assert_eq!(all["d"], serde_json::json!(40));
    }

    #[test]
    fn test_clear() {
        let mut cm = ConfigManager::new();
        let mut defaults = HashMap::new();
        defaults.insert("a".to_string(), json_val(serde_json::json!(1)));
        cm.set_defaults(defaults);
        cm.clear();

        assert!(cm.get("a").is_none());
    }
}

