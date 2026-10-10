// T-7 拆分：从 `lib.rs` 逐字节纯搬移的品牌信息命令（二十片）。
//
// `use super::*;` 让子模块看到 crate 根的全部项——含私有助手（`guard`、
// `unsupported_body`、`load_brand_config_from_env`、`brand_info_from_raw`）与公共类型
// （`SubstrateState` / `ProviderResult` / `HostResult`），因此搬来的命令体可原样调用，
// 零 `pub(crate)` 放宽。品牌 provider 的内部件（两条环境变量 const `BRAND_CONFIG_JSON_ENV`
// / `BRAND_CONFIG_ENV`、`brand_err`、`brand_info_from_raw`、`load_brand_config_from_env`）
// 与跨族消费的私有助手 `brand_configured`（`capabilities.rs` 经 `use super::*;` 调用它推导
// brand 域可用性）刻意留在 `lib.rs`：本族命令 `cmd_brand_info` 搬出后仍从父模块解析这些
// 助手，无需放宽可见性；`BrandInfo` 是 provider 内部件 `brand_info_from_raw` 的返回线形，
// 由 crate 根 `pub use brand::BrandInfo` 再导出后 lib.rs 照常按裸名解析。

use super::*;

/// `host_brand_info` 的线形返回（camelCase）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrandInfo {
    /// 应用标识符（如 `com.tauron.standard`）。
    pub identifier: String,
    /// 自定义协议名。
    pub protocol_scheme: String,
    /// 自启项名称。
    pub autostart_name: String,
    /// 数据目录名。
    pub data_dir: String,
    /// 快捷键绑定（键名 → 按键序列）。
    pub shortcuts: std::collections::BTreeMap<String, String>,
    /// 图标路径（平台字符串 → 相对路径）。
    pub icons: std::collections::BTreeMap<String, String>,
}

/// `host_brand_info`：品牌信息（**真实实现**，接孤儿 crate `tauron-brand`）。
///
/// 此前是恒 `UnsupportedBody` 的桩；现在未配置来源时仍如实降级，配置了则走
/// `tauron-brand` 的校验（必填字段 / 快捷键唯一性）——**不返回裸桩**。
///
/// （`host_market_check` 的实现在"壳扩展"一节，与 download/install 放在一起——
/// R8 把三条商城命令的线形统一成有类型的结构，三个定义不该分散在两处。）
pub fn cmd_brand_info(_state: &SubstrateState) -> HostResult<ProviderResult<BrandInfo>> {
    guard("brand_info", || match load_brand_config_from_env()? {
        None => Ok(ProviderResult::Unsupported(unsupported_body(
            "brand provider is not configured",
            Some("设置环境变量 TAURON_BRAND_CONFIG_JSON 或 TAURON_BRAND_CONFIG"),
        ))),
        Some(cfg) => Ok(ProviderResult::Value(brand_info_from_raw(&cfg)?)),
    })?
}
