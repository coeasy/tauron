// §4.12 设置中心的错误类型。
//
// 本 crate 的错误**不跨 IPC**：设置在宿主进程内被消费，由调用方
// （§4.10 UI / 设置页）翻译成 `tauron_host::error::HostError`。
// 不新增 `ErrorCode`——TS 门禁 `gates.test.ts` 断言两侧错误码同序。

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 设置中心的错误。
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// schema 未注册就读取/写入。
    #[error("插件 `{0}` 未注册 schema")]
    SchemaNotRegistered(String),

    /// schema 编译失败（形态违规、未知扩展键、版本越界等）。
    #[error("schema 编译失败：{0}")]
    SchemaCompile(String),

    /// 单表单字段数超过上限（RJSF 大 schema 会变慢，架构 §6.2 / 计划 §4.12）。
    #[error("插件 `{plugin}` 的表单有 {fields} 个字段，超过上限 {limit}")]
    FieldLimitExceeded { plugin: String, fields: usize, limit: usize },

    /// 写入值未按 schema 校验通过。
    #[error("写入 `{key}` 未通过 schema 校验：{detail}")]
    Validation { key: String, detail: String },

    /// 命名空间越界：插件试图写入自己的命名空间之外。
    #[error("插件 `{writer}` 越界写入命名空间 `{target}`")]
    NamespaceViolation { writer: String, target: String },

    /// 键路径非法（空段、`$` 前缀保留字、含 `.` 歧义）。
    #[error("非法键路径 `{0}`")]
    InvalidPath(String),

    /// 存储层的 I/O 或格式错误。
    #[error("存储错误：{0}")]
    Store(String),
}

pub type SettingsResult<T> = Result<T, SettingsError>;

impl From<tauron_schema::SchemaError> for SettingsError {
    fn from(e: tauron_schema::SchemaError) -> Self {
        SettingsError::SchemaCompile(e.to_string())
    }
}

/// 四层配置的层类型（优先级递增）。
///
/// | 层 | 序 | 含义 |
/// |---|---|---|
/// | Builtin | 0 | 框架内置默认（编译进二进制） |
/// | Brand | 1 | 白标覆盖 |
/// | Plugin | 2 | 插件随包默认 |
/// | User | 3 | 用户写入（最高） |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LayerKind {
    Builtin = 0,
    Brand = 1,
    Plugin = 2,
    User = 3,
}

impl LayerKind {
    pub const ALL: [LayerKind; 4] = [
        LayerKind::Builtin,
        LayerKind::Brand,
        LayerKind::Plugin,
        LayerKind::User,
    ];

    pub fn priority(self) -> u8 {
        self as u8
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LayerKind::Builtin => "builtin",
            LayerKind::Brand => "brand",
            LayerKind::Plugin => "plugin",
            LayerKind::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "builtin" => Some(LayerKind::Builtin),
            "brand" => Some(LayerKind::Brand),
            "plugin" => Some(LayerKind::Plugin),
            "user" => Some(LayerKind::User),
            _ => None,
        }
    }
}

impl std::fmt::Display for LayerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for LayerKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for LayerKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        LayerKind::parse(&s).ok_or_else(|| serde::de::Error::unknown_variant(&s, &[]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_messages_are_specific() {
        assert!(SettingsError::SchemaNotRegistered("p".into()).to_string().contains("p"));
        assert!(SettingsError::FieldLimitExceeded {
            plugin: "p".into(),
            fields: 41,
            limit: 40,
        }
        .to_string()
        .contains("41"));
        assert!(SettingsError::NamespaceViolation {
            writer: "a".into(),
            target: "b".into(),
        }
        .to_string()
        .contains("b"));
    }

    #[test]
    fn layer_kind_priority_is_ascending() {
        let all = LayerKind::ALL;
        for w in all.windows(2) {
            assert!(w[0].priority() < w[1].priority());
        }
    }

    #[test]
    fn layer_kind_serde_roundtrip() {
        for k in LayerKind::ALL {
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(v, json!(k.as_str()));
            assert_eq!(serde_json::from_value::<LayerKind>(v).unwrap(), k);
        }
    }

    #[test]
    fn layer_kind_rejects_unknown() {
        assert!(serde_json::from_value::<LayerKind>(json!("cosmos")).is_err());
        assert!(LayerKind::parse("cosmos").is_none());
    }

    #[test]
    fn schema_error_converts_into_settings_error() {
        let se = tauron_schema::SchemaError::Structure("坏 schema".into());
        let msg = SettingsError::from(se).to_string();
        assert!(msg.contains("坏 schema"));
    }
}
