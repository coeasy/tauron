// tauron-adapter · 商城（宿主更新）命令族（T-7 逐字节纯搬移 · 十七片之三）。
//
// 本模块由 crate 根 lib.rs 的命令段整体纯搬移而来（旧 lib 6572–6813 行，一段连续 242 行），
// 未改动任何一行代码——函数体、doc、注释、线形 struct 全部逐字节保真，仅把归属文件从 lib.rs 换到
// market.rs。内容：两条 camelCase 线形返回值 `MarketCheckResult` / `MarketUpdateResult`，三条不过
// 身份的核心 `cmd_market_check`（有意保持的宿主桩，reason 指向真通道）/ `cmd_market_download` /
// `cmd_market_install`（后两条按 `upgrade_installer.native_supported()` 分派到真/模拟双胞胎），四个
// 私有助手腿 `cmd_market_download_wired` / `_simulated` / `cmd_market_install_wired` / `_simulated`
// （仅族内可见、不外泄、不再导出），以及三条带身份判定的 wire 版 `cmd_market_check_as`（裸
// require_main_window）/ `cmd_market_download_as` / `cmd_market_install_as`（后两条走 admin_gate，
// 判定 + 结构化审计同源，供应链入口级特权）。
//
// 刻意留在 lib.rs 的是更广的更新通道基础设施与装配：`UpdaterSink` / `UpdaterCheckOutcome` /
// `UpdaterStatus`、`ShellExtState`（进程内 update_state / update_state_simulated 账本）、
// `UpgradeInstaller` seam 与其装配、审计咽喉 `admin_gate` / `require_main_window`、恢复对账
// `reconcile_recovery_phase`，以及 lib.rs 内联的 `round40_market_wiring_tests` 观察测试模块（门禁
// `check-simulated-never-commits` 的 A2c 段按名字钉它）。本族经 `use super::*;` 原样可见上述项，
// 零可见性放宽、纯归属搬移。命名区分：本文件是 tauron-**adapter** 的商城**命令族**（宿主命令面），
// 与 tauron-**market** crate 的 `api.rs`（进程内 `MarketplaceApi` mock）是两回事。

use super::*;

/// `host_market_check` 的返回值（camelCase 线形，**有类型**）。
///
/// R8 之前这里是裸 `serde_json::json!({ "available": false })`：`simulated` 这件事
/// 只写在注释里，前端**没有任何字段**可以据此判断"这是模拟结果"。现在它是线字段。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCheckResult {
    /// 是否有可用更新。当前恒 `false`（没有更新源）。
    pub available: bool,
    /// **是否模拟结果**：当前恒 `true`——本命令不做任何真实可用性探测
    /// （无 `tauri-plugin-updater`，也不请求任何 endpoint）。
    pub simulated: bool,
    /// 可用版本号；无则 `null`。
    pub version: Option<String>,
    /// 为什么不可用 / 为什么是模拟结果；成功且非模拟时为 `null`。
    pub reason: Option<String>,
}

/// `host_market_download` / `host_market_install` 的返回值（camelCase 线形）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketUpdateResult {
    /// 本次命令是否执行成功（**不是**"更新是否真的落地"——那看 `simulated`）。
    pub ok: bool,
    /// **是否模拟结果**：缺省装配恒 `true`——没有下载任何字节、没有验签、没有
    /// 替换文件；升级装配腿（[`DistributeUpgradeInstaller`]）注入后为 `false`
    /// （真实效果已发生并落账）。
    pub simulated: bool,
    /// 目标版本号。模拟路径缺省入参时为 `null`；真路径恒为**宿主清单**版本。
    pub version: Option<String>,
    /// 为什么是模拟结果；真实效果成功后为 `null`。
    pub reason: Option<String>,
}

/// `host_market_check`：检查更新。
///
/// ⚠️ **这是桩，而且现在会如实说出来**：返回值里 `simulated: true` +
/// `reason` 写明"未接入更新源"，并且**指到真通道去**。
///
/// 说清楚一件事（轮 28 复核、轮 29 收口、轮 40 更新）：本仓的更新能力**不是**没有
/// 真实现——真的检查是 [`cmd_updater_check`]（宿主注入 `EndpointClient` 后直接跑
/// `tauron_distribute::check_for_update`：清单校验 + 灰度 + 签名）；真的下载/安装是
/// [`cmd_market_download`] / [`cmd_market_install`]（宿主装配
/// [`DistributeUpgradeInstaller`] 后为真，缺省如实模拟）。**唯独本命令**（check）刻意
/// 是桩：它**不**去读调用方下发的 `endpoints` / `pubkey`——那两个参数今天
/// 一个都不用，将来也不用——让 webview 指定宿主去哪取更新清单，等于把宿主的更新通道
/// 交给调用方（供应链投毒的第一步，见 [`cmd_market_check_as`] 的危害原文）。
/// 端点的权威来源只能是宿主装配。
///
/// 对外 SDK 的 `AutoUpdateClient.checkUpdate()` 自轮 29 起读的是真通道，不再把本桩
/// 当可用性结论。
///
/// **主窗专属（轮 11 第二批）**：商城命令操作的是**宿主级产物**（更新源 / 安装
/// 包），插件不得触发；wire 层转调 [`cmd_market_check_as`] 判定，本函数是
/// **不过身份**的核心。
pub fn cmd_market_check(_state: &SubstrateState) -> HostResult<MarketCheckResult> {
    guard("market_check", || {
        Ok(MarketCheckResult {
            available: false,
            simulated: true,
            version: None,
            reason: Some(
                "未接入更新源：本命令是宿主本地桩，不做任何真实可用性探测（没有请求任何 \
                 endpoint，调用方下发的 endpoints/pubkey 一律不用）；真检查在 \
                 host_updater_check（宿主注入端点即为真），真下载/替换在 \
                 host_market_download / host_market_install（宿主装配 UpgradeInstaller \
                 后为真，缺省如实模拟）"
                    .to_string(),
            ),
        })
    })?
}

/// `host_market_download`：下载并校验更新包（真实路径在装配腿注入后成立）。
///
/// **两条路，线形同形，`simulated` 如实区分**：
/// - 缺省（[`NoUpgradeInstaller`]）：模拟——只把进程内 `update_state` 推到
///   `downloaded:<version>`（版本取调用方入参），`simulated: true` + `reason`；
/// - 装配腿注入（[`DistributeUpgradeInstaller`]）：真下载——字节落盘 + SHA-256 +
///   验签通过后才推账本（版本取宿主清单，**不读**调用方入参），`simulated: false`。
///
/// 两条路的失败语义一致：**零账本**——失败时 `update_state` /
/// `update_state_simulated` 原样不动（下载失败不得表现为"已下载"）。
///
/// **主窗专属（轮 11 第二批；轮 40 起带审计）**：wire 层转调
/// [`cmd_market_download_as`]——判定 + 结构化审计留痕（供应链入口级特权）；
/// 本函数是**不过身份**的核心。
pub fn cmd_market_download(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    guard("market_download", || {
        if state.upgrade_installer.native_supported() {
            return cmd_market_download_wired(state);
        }
        cmd_market_download_simulated(state, version)
    })?
}

/// 模拟下载腿（缺省装配；原桩实现原样保留，账本如实标注 simulated）。
fn cmd_market_download_simulated(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("downloaded:{}", version.unwrap_or("unknown")));
    ext.update_state_simulated = true;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: true,
        version: version.map(str::to_string),
        reason: Some(
            "模拟下载：**没有**下载任何字节、没有写入任何文件、没有校验签名；\
             只是把进程内 updateState 推进到 downloaded"
                .to_string(),
        ),
    })
}

/// 真下载腿（装配腿已注入）：seam 成功后账本推进为**真实**（`simulated: false`）。
///
/// 失败路径零账本：seam 返回 `Err` 时账本两个字段原样不动。
fn cmd_market_download_wired(state: &SubstrateState) -> HostResult<MarketUpdateResult> {
    let staged = state.upgrade_installer.download()?;
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("downloaded:{}", staged.version));
    ext.update_state_simulated = false;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: false,
        version: Some(staged.version),
        reason: None,
    })
}

/// `host_market_install`：安装已 staged 的更新包（真实路径在装配腿注入后成立）。
///
/// 与 [`cmd_market_download`] 同构的两条路：缺省模拟（推 `installed:<version>`，
/// `simulated: true`）；装配腿注入后经完整 `UpgradeRunner`（备份 → 解压 → 原子
/// 交换 → 健康检查 → 提交日志 → 重启/自动回滚），`simulated: false`。
///
/// 失败路径同样**零账本**：含健康检查失败后的自动回滚——回滚不改账本，账本仍
/// 停在 `downloaded:<v>`（与磁盘真相同步），runner 的错误上浮为类型化拒绝。
///
/// **主窗专属（轮 11 第二批；轮 40 起带审计）**：wire 层转调
/// [`cmd_market_install_as`]（同 [`cmd_market_download`] 的判定与审计口径）；
/// 本函数是**不过身份**的核心。
pub fn cmd_market_install(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    guard("market_install", || {
        if state.upgrade_installer.native_supported() {
            return cmd_market_install_wired(state);
        }
        cmd_market_install_simulated(state, version)
    })?
}

/// 模拟安装腿（缺省装配；原桩实现原样保留）。
fn cmd_market_install_simulated(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("installed:{}", version.unwrap_or("unknown")));
    ext.update_state_simulated = true;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: true,
        version: version.map(str::to_string),
        reason: Some(
            "模拟安装：**没有**替换任何二进制/文件、没有触发任何重启流程；\
             只是把进程内 updateState 推进到 installed"
                .to_string(),
        ),
    })
}

/// 真安装腿（装配腿已注入）：runner 提交成功后才推账本（`simulated: false`）。
fn cmd_market_install_wired(state: &SubstrateState) -> HostResult<MarketUpdateResult> {
    let installed = state.upgrade_installer.install()?;
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("installed:{}", installed.version));
    ext.update_state_simulated = false;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: false,
        version: Some(installed.version),
        reason: None,
    })
}

/// `host_market_check` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这三条是主窗专属（危害原文）
///
/// 商城命令操作的是**宿主级产物**，不是某个插件的东西：
///
/// - `check` 会拿调用方下发的**更新源 endpoint / 公钥**去探更新（接入后）：插件能
///   借此把宿主指向自己的更新源（供应链投毒的第一步），或当成内网探测跳板；
/// - `download` / `install` 自轮 40 起是**真供应链操作**（装配腿注入后）：写宿主
///   磁盘、替换应用自身二进制——没有任何"某个插件"能成为这类操作的主体，让插件
///   webview 调它等于把宿主自身的更新通道交给插件；
/// - 三条都不收身份参数，所以判定只能落在**整条命令**上（同码 `E_AUTH_DENIED`）。
///
/// **判定与审计（轮 40 起）**：`check` 仍是桩（不请求网络、不写文件），走
/// [`require_main_window`] 且不进审计表；`download` / `install` 已接线，按
/// `tauron_host::admin_audit` 的口径说明**连同 `authz` 档位登记**一起走
/// [`admin_gate`]（判定 + 结构化审计留痕——被拒的提权尝试同样留痕）。拒绝路径
/// **零副作用**：`updateState` 不被写（有测试断言）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_market_check_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<MarketCheckResult> {
    require_main_window(caller, "host_market_check")?;
    cmd_market_check(state)
}

/// `host_market_download` 的**带身份判定 + 审计**版本（见 [`cmd_market_check_as`] 的
/// 危害说明；轮 40 接线后与注册表安装同级，判定与审计同源）。
pub fn cmd_market_download_as(
    caller: &Caller,
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    admin_gate(state, caller, "host_market_download")?;
    cmd_market_download(state, version)
}

/// `host_market_install` 的**带身份判定 + 审计**版本（见 [`cmd_market_check_as`] 的
/// 危害说明；轮 40 接线后与注册表安装同级，判定与审计同源）。
pub fn cmd_market_install_as(
    caller: &Caller,
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    admin_gate(state, caller, "host_market_install")?;
    cmd_market_install(state, version)
}
