//! §4.13 schema 管线的结构化错误。
//!
//! 管线错误**不跨 IPC**：它在构建期/安装期/写入前被消费，由调用方
//! （§4.12 设置中心）翻译成 `tauron_host::error::HostError`。
//! 因此本 crate 有独立的错误类型，不占用 `ErrorCode` 的冻结位。

use thiserror::Error;

/// schema 管线的全部错误。
#[derive(Debug, Error)]
pub enum SchemaError {
    /// `$ref` 指向不存在的位置。
    #[error("未解析的 $ref `{0}`：在 schema 文档内找不到该位置")]
    UnresolvedRef(String),

    /// `$ref` 展开时检测到环。
    ///
    /// JSON Schema 允许 schema 自引用，但 RJSF 侧的 #4666/#4505
    /// 意味着无限展开的产物渲染不出来——所以直接拒绝而不是静默截断。
    #[error("循环 $ref `{0}`：schema 自引用无法展开成平面结构")]
    RefCycle(String),

    /// 展开后仍出现嵌套 `oneOf`/`anyOf`（门禁 §8-12）。
    #[error("禁止形态：嵌套的 {0}（门禁 §8-12），位置 {1}")]
    NestedCombo(String, String),

    /// 展开后仍残留 `$ref`（门禁 §8-12）。
    #[error("禁止形态：未展开的 $ref `{0}`（门禁 §8-12）")]
    UnexpandedRef(String),

    /// `x-tauron` 里出现未登记的键。
    ///
    /// 计划 §4.13："未知扩展键报错（不静默忽略）"——否则拼写漂移
    /// 会让 UI 意图悄悄失效，用户只看到"表单不太对"。
    #[error("未知的 x-tauron 键 `{0}`（位置 {1}）：未登记的 UI 意图会被忽略，故拒绝")]
    UnknownExtension(String, String),

    /// `x-tauron.version` 超出支持范围。
    #[error("x-tauron 版本 {0} 不受支持（当前支持 {1}，位置 {2}）")]
    UnsupportedVersion(u32, u32, String),

    /// `x-tauron` 的字段类型不对。
    #[error("x-tauron.{0} 期望 {1}，实际 {2}（位置 {3}）")]
    ExtensionType(String, String, String, String),

    /// schema 结构本身不合法。
    #[error("schema 结构错误：{0}")]
    Structure(String),

    /// 值不满足 schema（写入前校验，§4.12 消费）。
    #[error("值不满足 schema：{0}")]
    Validation(String),
}

/// 管线结果别名。
pub type SchemaResult<T> = Result<T, SchemaError>;

impl SchemaError {
    /// 该错误是否为可预期的输入问题（区别于内部 bug）。
    pub fn is_input_error(&self) -> bool {
        !matches!(self, Self::Structure(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_readable() {
        let cases = [
            SchemaError::UnresolvedRef("#/$defs/Missing".into()),
            SchemaError::RefCycle("#/$defs/Node".into()),
            SchemaError::NestedCombo("oneOf".into(), "/properties/level".into()),
            SchemaError::UnexpandedRef("#/$defs/A".into()),
            SchemaError::UnknownExtension("lbel".into(), "/properties/name".into()),
            SchemaError::UnsupportedVersion(99, 1, "/".into()),
            SchemaError::ExtensionType("widget".into(), "string".into(), "object".into(), "/".into()),
            SchemaError::Structure("root 不是对象".into()),
            SchemaError::Validation("/duration 超出范围".into()),
        ];
        for e in &cases {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "{e:?} 的展示不应为空");
        }
    }

    #[test]
    fn only_structure_is_not_an_input_error() {
        assert!(SchemaError::UnresolvedRef("x".into()).is_input_error());
        assert!(SchemaError::NestedCombo("a".into(), "b".into()).is_input_error());
        assert!(!SchemaError::Structure("boom".into()).is_input_error());
    }

    #[test]
    fn unknown_extension_reports_the_position() {
        let e = SchemaError::UnknownExtension("ordr".into(), "/properties/volume".into());
        let s = e.to_string();
        assert!(s.contains("ordr"));
        assert!(s.contains("/properties/volume"));
    }
}
