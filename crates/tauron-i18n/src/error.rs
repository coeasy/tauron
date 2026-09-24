// §4.20 i18n 的错误类型。
//
// 不跨 IPC：i18n 在宿主进程内消费，由 UI 层翻译成结构化错误。

#[derive(Debug, thiserror::Error)]
pub enum I18nError {
    /// 非法语言代码。
    #[error("非法语言代码：{0}")]
    InvalidLocale(String),

    /// 资源包格式错误。
    #[error("资源包格式错误：{0}")]
    BundleFormat(String),

    /// 资源包缺失。
    #[error("资源包 `{0}` 不存在")]
    BundleMissing(String),
}

pub type I18nResult<T> = Result<T, I18nError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(I18nError::InvalidLocale("xx-XX-XXX".into())
            .to_string()
            .contains("xx-XX-XXX"));
    }
}
