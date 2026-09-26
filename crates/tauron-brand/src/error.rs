// §4.16 白标的错误类型。
//
// 不跨 IPC：品牌配置在构建时校验，由 CI 或构建脚本消费。

/// 白标错误。
#[derive(Debug, thiserror::Error)]
pub enum BrandError {
    /// 必填字段为空。
    #[error("品牌字段 `{0}` 为空")]
    EmptyField(String),

    /// 快捷键冲突。
    #[error("快捷键 `{combo}` 重复（键 `{key}`）")]
    DuplicateShortcut { key: String, combo: String },

    /// 缺少平台图标。
    #[error("缺少平台 `{platform}` 的图标")]
    MissingIcon { platform: String },

    /// 图标路径为空。
    #[error("平台 `{platform}` 的图标路径为空")]
    EmptyIconPath { platform: String },

    /// 唯一性违规。
    #[error("品牌唯一性违规：字段 `{field}` 值 `{value}` 在 `{brand_a}` 和 `{brand_b}` 中重复")]
    UniquenessViolation { field: String, value: String, brand_a: String, brand_b: String },

    /// 配置解析错误。
    #[error("配置解析错误：{0}")]
    ConfigParse(String),
}

pub type BrandResult<T> = Result<T, BrandError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_specific() {
        assert!(BrandError::EmptyField("identifier".into()).to_string().contains("identifier"));
        assert!(BrandError::MissingIcon { platform: "web".into() }.to_string().contains("web"));
    }
}
