# Changelog

本文件记录 tauron 的显著变更。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> **关于「已知债务」**：本仓库刻意区分「真实实现」与「接口占位」。下面每一节
> 的「已知债务」都是**实测**结果（附复现命令），不是推测。请勿在发布公告里
> 把债务当已完成项宣传。

---

## [1.1.0] - 2026-10-01

### Added

- **V4 生产就绪线并入（三条 feature 分支合并到 main）**：`DeploymentMode` /
  `ProductionReadiness` / `production_doctor`（A109）与 **fail-closed 启动门**——
  `AdapterConfig::production()` 下 readiness 不达标时宿主直接拒绝启动（Development/Test
  行为不变）；进程沙箱接入时按 A97 `hard` 强制再对账一次。
- **`host_production_doctor`（第 8 条主窗特权命令）**：机器可读的生产就绪自检报告
  （`deploymentMode` + 逐项 `checks[]` + `productionSafe`），TS 侧
  `AdminClient#productionDoctor()`；与启动门**同源**，不会出现「自检通过但启动拒绝」。
- **Event 审批三命令（`host_events_approve` / `revoke` / `approvals`）**：跨主体
  订阅审批事实的唯一落点；审批只接受**已被插件声明的主题**，审批表上限
  `MAX_APPROVALS = 4096`（达限 `E_SUBSCRIPTION_FULL`，不静默扩张）。
- **`host_stream_grant`**：流式通道的有界 byte credit 补充（插件运行时命令，
  self 档）。
- **V4 P0 落地（§33 差异回扫，本轮结清 5 项）**：
  - **安装身份与灰度分桶（R2-8 / §9.1）**：新增 `tauron-distribute::InstallationIdentity`
    ——首用随机 UUIDv4（`installation.id` 落数据目录，`.simple()` 形态，创建竞态安全，
    文件损坏**拒绝静默重置**而非重铸），FNV-1a 64→u32 折叠哈希出灰度桶，**不含 PII**；
    卸载即重置（身份是每安装一份，不是每用户一份）。
  - **设置变更观察（R2-4 / W6）**：宿主在设置落盘提交后把 `{ key, value, source, revision }`
    镜像到宿主所有的私有 topic `host:settings:changed`（`event` 通道，慢消费者丢最旧）；
    SDK 侧 `onSettingsChanged` 从此有真实投递路径（此前是**只在类型里存在的孤儿钩子**），
    并按插件命名空间过滤——跨插件的观察不投递。键是**线形键**（可直接回查/回写），
    不是 Store 的编码点路径。见 `docs/api/plugin-development-guide.md`「设置变更观察」。
  - **能力协商 fail-closed（R1-4 / §8.1）**：`TauriBackend` 启动只认 bootstrap 命令
    （`host_capabilities`），运行期协商结果经 `adoptCapabilities` 回填；空集合与
    不含 `host_capabilities` 的集合**一律拒绝采纳**（unknown = unsupported，不再拿
    静态全集误报能力）。`examples/minimal-app` 的协商失败改为保持 fail-closed +
    指数退避重试，不再回落全量表。
- **一次性安装评审链（V4）**：`host_registry_install_preview` 铸造 nonce 键控的
  `InstallReviewToken`（TTL 600s、一次性消费、表满按过期优先驱逐），
  `host_registry_install` 校验 token 与包哈希一致后才落盘——审批过的内容与实际
  安装内容之间的断链被闭合。
- **命令面**：底座 `tauron_substrate_handler!` **57 → 61**，全量
  `tauron_plugin_handler!` **78 → 83**，默认特性（含 `plugin-install` 2 条）
  **80 → 85**；`authz` 登记档位命令 **22 → 27**（特权 4 → 8）。口径由 wire-gate
  与 `contracts/public-surface-ledger.json`（85 条）逐名锁定。

### Fixed

- **灰度分桶此前恒为 0**：`DistributeUpdaterSink::check()` 把用户桶硬编码成 `0`，
  于是「按桶灰度」在真实链路上等价于「只灰度 0 号桶」。现由注入的
  `InstallationIdentity::user_hash()` 提供桶值，且 `with_endpoint(client, installation)`
  **强制**调用方给身份（不给就编译不过），未配置端点的 `unconfigured()` 走
  临时身份（不落盘、不参与灰度）。
- **资源预算补齐 count + bytes（R3-5）**：EventBus 队列、Settings 观察队列与设置值
  此前**只按条数**限额，大 payload 可以在条数不越界的情况下吃穿内存。现两侧都受
  字节预算约束：单帧上限 256 KiB、队列 2 MiB（`event` 溢出丢最旧、`request` 溢出得
  `E_CALL_PENDING_FULL`）、设置值上限 64 KiB、观察队列 1 MiB。超限一律**零副作用
  拒绝**（在 schema 校验之前、在状态快照被清之前），既有快照与队列不受扰动。
  通知标题/正文同样按字节预算拒绝（512 B / 4 KiB）。
- `host_events_approve` 的内层拒绝此前会被 guard 包装吞掉（错误丢失、返回恒成功），
  现原样上线。
- 注册表 pending 调用此前只在容量压力下 GC；现在诊断读
  （`host_resource_stats`）也顺带 `gc_expired()`，避免「只读不回收」。
- 崩溃投递与恢复引擎**同路径**（`RecoveryEngine::record_boot_failure`）：
  `consecutiveFailures` 与安全模式判定不再可能两套口径。
- 三轮全链路审计的接缝修复：`plugin-install` feature 矩阵下 5 处测试与实现脱节
  （生产就绪配置缺安装信任/可信时间材料、评审安装断言与实现文案不一致）已按
  实现真相对齐；CI 的 feature 矩阵门禁本地全绿。
- 文档与示例计数漂移清零（README / installation / app-layer-wire /
  incremental-adoption / plugin-development-guide / host README 等统一到
  61/83/85 口径）；`examples/minimal-app` 增第 8 条链路（AdminClient 消费
  `productionDoctor` + `eventsApprovals`，闭合 A109 孤儿命令面）；修正插件 SDK
  文档中的幻影命令名 `host_report_call_result`（实为 `host_call_result`）；
  `@tauron/host` 补导出 `HealthReport`。
- **轮 4 链路自检（本轮新落地代码自身）**：
  - 设置的两路投递（Store 提交口 + 消息面镜像）此前**写在每个调用方里**——新增写路径
    就可能只发其中一路（「主窗知道、插件不知道」的半接线复发）。现收成适配器内唯一
    提交口 `commit_settings_change`，并由**两侧结构门禁**钉住：Rust 侧
    `settings_commit_has_single_mirror_site` 数 `publish_committed_change` 出现次数
    必须为 1，TS 侧 wire-gate 的 R7 断言改指「写路径 → 单一提交口」的接线。
  - 镜像帧的 `key` 不再依赖调用方手传的原始键，改由 `settings_wire_key()` 从 Store
    编码路径反解——任何调用方都自动给出线形键（编码形态泄漏给订阅方会对不上账）。
  - **安装身份此前只在测试里是持久化的**：生产装配走临时身份，灰度桶跨进程漂移。
    现在只要配置了 `recovery_data_dir` 就落盘 `installation.id` 并暴露为
    `SubstrateState::installation_identity`，更新链路的桶值与它同源。
- **轮 5 前后端贯通与孤儿扫描**：命令名两侧（handler 宏 ↔ TS 调用点 ↔
  `host_capabilities` 静态表）逐名比对**无断链**；修的是口径与交代：
  - 「进程内 watcher」不再被描述成主窗的线上能力——Store 观察队列是 **Rust 嵌入方
    扩展点**，线上没有 `host_settings_watch`（接口文档、架构文档、crate 文档三处统一）。
  - 明确并**行为化**一条边界：`host_settings_adopt_legacy` / `host_settings_migrate`
    是整份文档级操作，**不扇出镜像帧、也不推进 revision**，订阅方必须重读
    `host_settings_get`（新增 `settings_bulk_ops_fan_out_no_frames_and_advance_no_revision`，
    含按键写的对照组，防「总线本来就静默」的假绿）。
  - `docs/architecture/overview.md` 第 14 条从「运行期订阅审批尚未接线」更正为
    **已接线**（三条特权命令 + `AdminClient`，仍缺键粒度授权）；`@tauron/host`
    README 的主窗客户端命令计数 78/80 → **83/85**。
  - 无消费者的公开 API 全部登记并标注归属（V4 文档新增「轮 5 未接线公开 API 台账」）：
    `tauron-distribute` 的 `upgrade` **执行侧**是留给装配方的集成点（宿主只消费检查侧，
    `host_market_*` 的下载/安装是 `simulated` 的进程内状态推进）、设置观察队列与
    流式额度探针标为嵌入方扩展点（后者补了与 `grant` 返回值同事实的断言）、
    `tauron-ffi` 补进 README 项目结构树、`@tauron/host` README 新增「库级 API」表。

## [1.0.2] - 2026-09-28

### Fixed

- 修复 npm create 在 Windows 下的启动器冲突，确保 `npm create tauron-app@1.0.2 -- <dir>` 能直接生成 Tauri 工程。
- 将公共 npm 包、Rust crates 与示例客户端同步到 `1.0.2`；发布门禁包含全新目录安装与构建验收。

## [1.0.1] - 2026-09-28

公共 npm 包与 Rust crates 曾以 `1.0.1` 上传。Windows 干净消费者验收发现 `npm create` 启动器冲突，因此该版本未建立 GitHub Release；请使用修复后的版本。

## [1.0.0] - 2026-09-27

首个版本。框架层（`@tauron/types|core|plugin-sdk|adapter-*|cli|market|shell-matrix|dual-world|contract-tests` + `tauron-shell`）
与应用层（`@tauron/host|framework|ui|ui-primitives|app-*` + `tauron-host`/`tauron-adapter`）双栈成型。

### Added

**集成就绪收口（R9，2026-09-27）** —— 目标是把「一键/快速被第三方集成」从「路径已通」
推进到「核心能力不缺、发布门禁是硬的、孤儿全部接通」。每项都以可复现命令为判据，
不是「代码里看起来有了」。

- **孤儿 crate 全部接通（原「已知债务」第 9 条，本轮结清）**：
  `crates/tauron-adapter/Cargo.toml` 新增四个依赖——`tauron-brand`（`host_brand_info`
  的真实实现）、`tauron-theme`（`host_theme_*` 主题定义表）、`tauron-distribute`
  （`host_updater_*` 更新通道：灰度 / 崩溃门禁 / 清单校验）、`tauron-wasm`（WASM
  插件形态的**状态层**校验：配置 / ABI / 崩溃预算；执行层仍无运行时，继续**诚实失败**）。
  `tauron-shell` 保持孤儿是**有意为之**（框架层门面，供外部宿主直接依赖），
  已在债务条目里登记为「有意」，不再算待激活。
- **五域宿主能力命令（原「已知债务」第 8 条，本轮结清）**：新增 menu（3）/ tray（3）/
  fs（6）/ http（1）/ updater（2）五域共 **15** 条，另加主题域 theme（3）共 **18** 条，
  **全部主窗专属**（代码层 `require_main_window`）。命令面因此变为：底座
  `tauron_substrate_handler!` **39 → 57**，全量 `tauron_plugin_handler!` **60 → 78**，
  默认特性（含 `plugin-install` 2 条）**80 条**。口径不靠人眼——wire-gate 断言
  底座集合 ⊂ 全量集合、且与 `SUBSTRATE_COMMANDS` 逐条一致。
- **`host_http_request` 诚实降级**：本仓库离线依赖闭包内**无** hyper-tls / hyper-rustls，
  故 HTTP 域不引入 TLS 栈，改为**可注入 `HttpSink`**——缺省 `UnavailableHttpSink`
  恒返回 `UnsupportedBody`（如实说不可用，不伪报发起过请求）；接入方注入自己的
  `HttpSink` 即真跑。
- **能力表 `families` / `unsupported` 改运行期推导（P2-4 已修）**：`cmd_host_capabilities`
  此前**硬编码**两列，注入的 sink 变化后与实际脱节；现改为按已注入的 sink **推导**
  且两列互斥，新增 `brand_configured()` 辅助与单测
  `host_capabilities_derives_domains_from_injected_sinks`。
- **发布就绪脚本与 CI 门禁（原「已知债务」第 5 / 6 条，本轮结清主体）**：19 个
  `packages/*` 去 `private` 并补发布元数据（仅 `@tauron/contract-tests` 是内部测试包，
  保留 `private`）；新增 `scripts/publish-npm.mjs`（逐包 `pnpm pack` + **解包**校验：
  含 `dist/`、入口齐全、无 `workspace:` 残留——**必须用 pnpm**，`npm pack` 会把
  `workspace:*` 原样写进 tarball 导致 registry 侧 `EUNSUPPORTEDPROTOCOL`）与
  `scripts/publish-crates.mjs`（按依赖拓扑排定的 `cargo publish` 序列）。两者默认
  **只校验**（`--check`），真发布需显式 `--publish` + token；CI 新增 `publish:npm --check`
  与 `cargo package --workspace` 两道发布就绪门禁。
- **代码卫生转硬门禁（原「已知债务」第 1 条，本轮结清）**：一次性 `cargo fmt --all`
  落地并验证编译 / 测试；`cargo clippy --workspace --all-targets --locked -- -D warnings`
  **零告警**（真修 5 处 + 2 处带中文原因的 `#[allow]`，无 crate 级放行）。
  `ci.yml` 的 `fmt` / `clippy` 两个 job 删除 `continue-on-error` 转硬门禁；
  `rustfmt.toml` / `clippy.toml` 的「尚未规范化」段落同步改写。
- **ESLint 转硬门禁（原「已知债务」第 3 条，本轮结清）**：81 条 warning 清零，
  `pnpm lint`（`eslint . --max-warnings 0`）**0 error / 0 warning**，已是硬门禁。

**框架层**

- 单命令信封协议：`plugin_invoke` / `plugin_cancel` / `plugin_emit`
- **Js 型隔离来自 webview 边界 + 逐命令身份判定**（非进程内沙箱）；
  `@tauron/dual-world` 的进程内沙箱是 fail-closed 模拟（`SANDBOX_UNAVAILABLE`），
  进程内 WASM 运行时仍为路线图项
- 插件形态以 `PluginType` 枚举为准：Js / Process 有生产执行器，
  Rust / Wasm 返回 `E_PLUGIN_TYPE_NO_RUNTIME`（**诚实失败**，不伪报可用）。
  **代码里不存在「B+ 混合模式」**——该宣称已删除（原写作「4+1 形态」，为未兑现宣称）
- 双层 ACL：外层 Tauri 静态 ACL + 内层框架动态 ACL
- 每插件隔离的事件总线队列，`Event` 溢出丢最旧 / `Request` 硬失败 / `State` 只留最新；
  `MAX_QUEUE = 1000`，连续溢出 3 次熔断
- 配置 4 层合并（session > plugin > user > default）
- Shell 矩阵 4 形态（local / local-server / remote-url / sub-webview）
- 插件市场：**Ed25519** 验签（`verify(data, signature, publicKey)`，非对称——私钥签发、
  公钥公开验证）、注册表搜索 / 发布
- CLI 工具链：`create` / `plugin new|dev|test|pack|sign|publish` / `doctor`
- React / Vue / Svelte 适配层

**应用层**

- **全量架构重评估（1.0，2026-09-26）**：按「入口可达性」判据（不是「函数存在」）
  对 15 crate + 20 包重测，产出
  [full-architecture-refactor-plan.md](./docs/architecture/full-architecture-refactor-plan.md)
  （含 W1–W10 工作流与轮 30–41 编排）。要点：
  - **0.4 方案状态更正**：只有 **A1–A3 落地**（轮 20–22），A4–A8 与轮 23–29 未开工。
    此前提交 `08156f9` 的信息「A4–A8 按方案落地」**不准确**，以重评估文档为准
    （按决策不改写已公开历史）。
  - **主体链路**：12 跳中 10 跳真通（调用投递 L8 与插件自举 L6 已由轮 20/22 修复），
    剩 L3 安装默认不可达、扩展点 UI 未驱动。
  - **新发现 P0**：唯一的可运行 app 里插件管理 UI **完全惰性**——示例只
    `import '@tauron/ui/wc'`，而该入口只注册 `oc-toast`（构建产物佐证），
    `oc-plugin-manager` 等 6 个组件永不 upgrade。
  - **新发现 P0**：`tauron-proc` spawner 三处缺陷（`closed`/`sinks` 两锁非原子的
    sink 泄漏竞态、`write_frame` 持全局锁阻塞、`lines()` 无行长上限）。
  - **新发现 P0**：安装签名 payload 只覆盖文件哈希，`algorithm`/`kid`/`issued_at`
    未进签名（可篡改绕过有效期）。
  - **孤儿重测**：`tauron-acl`/`tauron-market` 已非孤儿（在 adapter 依赖表内）；
    真孤儿为 brand / theme / wasm / distribute / shell（合计 7536 行）。
  - **文档更正**：`docs/architecture/README.md` 的「唯一的前瞻计划」、
    「4+1 插件形态」、「双世界隔离（QuickJS-WASM）」三处漂移已修；
    本 CHANGELOG 同段的两条框架层宣称一并更正。

- **1.0 W 系列落地（第一批，2026-09-26）**：按上述方案实施，本轮完成
  **W2 / W6 / W7 / W3 / P1-10**，并部分完成 **W1-b（TS 侧单线收敛）/ W10（文档派生）**。
  每项都以「可复现命令输出」为判据（不是「代码里看起来有了」）：
  - **W2 端到端可运行**：`@tauron/ui-primitives/wc` 补 4 条副作用导入 → 示例构建产物
    注册的自定义元素 **1 → 10**（含 `oc-plugin-manager`）。新增两道门禁
    （「源码定义的标签必须已注册」「`@tauron/ui/wc` 必须注册壳组件」）。
  - **W7 运行时硬化**：spawner 的 `SinkTable` 单锁消除 TOCTOU 泄漏、写帧改两级锁
    （不再全局阻塞）、单帧 `MAX_FRAME_BYTES` 上限、EOF 主动回收；proc **17 → 22** tests。
  - **W6 安装默认可达 + 签名强化**：`tauron-adapter` 的 `default = ["plugin-install"]`；
    签名载荷升级 **v2**（`algorithm`/`kid`/`issuedAt` + `path`/`size`/`hash` 全进签名）
    + 有效期与时钟偏移校验 + 路径控制字符拒绝；market **63 → 73** tests。
  - **W3 扩展点闭环**：`ShellController` 接 `oc-command-select` 真投递；新增
    `UNWIRED_EVENTS` 显式登记表（替代「写句注释就放行」的门禁漏洞）；**新增
    `host_contributes_reconcile`（self 档）+ `E_CONTRIBUTES_DRIFT`**——比对 manifest
    声明与 activate 期注册，分叉报错并点名缺哪条，SDK 激活后真消费它。
    命令面：插件面 **17 → 18**，adapter 全量 **59 → 60**（底座 39 不变）。
  - **P1-10 档位表生产自检**：`validate_command_registry` 不再只在测试里调用——
    `SubstrateState` 装配期跑一次（`OnceLock` 缓存、可断言），失败即 panic。
  - **W1-b（部分）**：**删除死编排器 `bootstrap()`**——它是 P1-1「活线依赖死线」的
    载体（import `@tauron/core` 三个**运行时类**，而本包 `sideEffects:false`，
    真实构建里这些类不存在，一用即崩）。`@tauron/host` 移除 `@tauron/core` 依赖，
    P1-5 的「TS `maxPlugins: 32` vs Rust `max_plugins: 8`」第二事实源随之消失。
  - **W10（部分）**：命令面计数（59→60）在 7 份文档中同步；新增/改造 5 条门禁。
  - **实测绿**：`cargo check --workspace --all-targets` 0 警告；
    `cargo test --workspace --locked --lib --tests` 15 个测试二进制全绿
    （adapter **221** / host **284** / market 73 / proc 22）；`pnpm -r typecheck` 全绿；
    `eslint` 0 error；wire-gate **142** tests 全绿。

- **发布收口三轮审计（0.4，2026-09-26）**：按「主体流程全联通、核心链路无断链、
  无孤儿逻辑、无死循环」对 Rust 15 crate + TS 20 包做了三轮全量审计，修复：
  - **删除孤儿模拟执行器 `ProcRunner`**（`tauron-proc/src/spawn.rs`，约 470 行）：
    全仓零生产调用方（真实链路 = `CommandSpawner` + `tauron-host` 的
    `RuntimeTable`），且 `spawn` 不真起进程却返回 `Running`、RPC 是回显模拟——
    与真实执行器并存极易误用。仅被它使用的 `RpcConfig` / `HeartbeatTracker` /
    `ConcurrencyTracker` / `ProcPluginConfig` / `validate_plugin_config` 等与
    `ProcError` 的九个零产生点变体一并删除（**诚实边界**：真实链路的心跳监控
    尚未实现，见 capability-closure-plan A3 注记——是"未实现"，不是"在别处"）。
  - **孤儿死代码清除**：`RuntimeTable::has_reaper`（仅测试调用）、
    `tauri.rs::parse_stream_kind`（零调用方）删除。
  - **`host_contributes_list` 补登记 authz 档位**（scoped-read）：`ShellClient.
    contributesList()` 一直在调、handler 已注册，但不在 `COMMANDS` /
    `ADMIN_COMMANDS`——R7 收口同类缺口。TS `CAPABILITIES` 镜像同步（22→23 条）。
  - **能力真相采纳链路接通**（0.4-A2 收尾）：`ShellClient.refreshCapabilities()`
    此前有实现无入口；示例 app 启动即拉取 `host_capabilities` 回填传输层，
    并用 `supports('host_registry_install')` 对 feature-gated 命令做明确拒绝。
  - **`CommandSpawner` 加固**：spawn 的 stdin/stdout 管道句柄获取失败路径现在
    kill+wait 回收子进程（不再留孤儿进程）；`register_frame_sink` 拒绝进程
    EOF 后的迟到注册（closed 集合），消除重复"崩溃→重启"下 sink 表无界累积。
  - **锁纪律修复**：`cmd_registry_admin` 的卸载回收块此前持 bus 锁嵌套获取
    其余五把锁（无反向路径、不成环，但属未声明嵌套序），改为 bus 锁只覆盖
    两个 dispose 调用。
  - **契约钉补齐**：`@tauron/app-contract-kit` 的错误码全集测试补
    `E_CALL_ALREADY_SETTLED`（0.4-A1 漏网的第二阶钉子）。
  - **文档数字全量对账**：命令面 54/56/58 等历史口径统一为实测
    **60 条（底座 39 + 插件运行时 21；`plugin-install` 另加 2 条）**，涉及
    overview / app-layer-wire / installation / incremental-adoption /
    competitive-analysis / 示例 README；wire-gate 计数 110→140。
- **发布收口三轮审计（1.0，2026-09-27）**：按同一套判据（主体流程全联通、
  核心链路无断链、无孤儿逻辑、无死循环、前后端全贯通）再做**三轮**全量审计，
  每轮修完全部问题后进下一轮。本轮发现的缺陷**类型集中在「已修的事实没同步到
  声明处」与「门禁假绿」**，而非新的功能缺口——这正是 1.0 收口期的典型形态。
  - **生产入口补线（R1-1）**：`ShellController`（`W3` 引入的壳层编排器，
    接 `oc-command-select` → `callPlugin` → `callTakeResult` 真投递链）
    **在生产里零实例化**——只有测试构造它。示例 app 的启动序列补上装配，
    这条链才真正可达；只修测试不算修（「入口可达性」判据）。
  - **运行时竞态修复（R2-1）**：`@tauron/host` 的 `rpc.ts` 事件泵 `tick()`
    逐订阅者无隔离——任一订阅者回调抛错会打断同 tick 的其余订阅者并让订阅表
    与宿主侧 topic 失同步；改为逐订阅者 try/catch 隔离，且 `subscribe()` 改为
    **宿主订阅成功后才登记 topic**（原先先登记后订阅，宿主拒订阅时留下孤儿 topic）。
  - **超时资源泄漏修复（R2-1c）**：`lazy-plugin-loader.ts` 的 `_loadWithTimeout`
    在 `Promise.race` 分支下未清理定时器；补 `try/finally` + `clearTimeout`。
  - **可选字段契约修复（R2-2）**：`auto-update-client.ts` 的 `UpdateInfo.currentVersion`
    改为可选（宿主在「无已安装版本」时本就不回填它，必填会让消费方按错误前提编码）。
  - **孤立状态如实披露（R2-3/R2-4）**：`tauron-adapter` 的 `notifications` /
    `window_rect` / `update_state`，与 `tauron-recovery` 的 `RecoveryAction`，
    均在源码处以 ⚠️ 标注真实接入状态——**存在状态容器不等于有链路**。
  - **门禁假绿加固（R3-1）**：`unwired-events.test.ts` 的事件名解析若漏项会
    「解析到空 = 一致」地假绿；新增断言「解析出的键数 == `SHELL_EVENTS` 声明键数」。
  - **文档漂移全量对账（R3-2/R3-3 与文档批）**：`plugin-install` **已进默认特性**
    （`crates/tauron-adapter/Cargo.toml:21`，1.0-W6）这一事实**未同步到大量声明处**——
    多份文档与两处代码注释仍写「`default = []`、默认构建不注册」。
    逐处更正：`tauron-host/src/tauri-backend.ts`（`OPTIONAL_FRAMEWORK_COMMANDS`
    头注）、`contract-tests/src/wire-gate.test.ts`（0.4-A2 门禁注释）两处代码注释，
    以及 installation / app-layer-wire / multi-plugin-substrate-roadmap /
    competitive-analysis / capability-closure-plan / full-architecture-refactor-plan
    六份架构与竞品文档；`@tauron/market` README 的签名算法从 `HMAC-SHA256`
    更正为 **Ed25519**（`verify(data, signature, publicKey)`，第三参是**公钥**）；
    `docs/api/plugin-development-guide.md` 补 Rust 形态整行、新增「插件安装命令
    （2 条，feature-gated 且已进默认特性）」小节（用户重点要求的使用/接口文档）；
    `tauron-host/README.md` 命令面表重写为 22 行 + `plugin-install` 默认特性注。
  - **过度宣称剔除**：`capability-closure-plan.md` 与 `full-architecture-refactor-plan.md`
    的记分卡基线快照补「2026-09-27 复核」横幅，把已闭环项（插件调用 ✅、安装
    ⚠️→默认可达、扩展点对账 🟡）与仍未做项（W1-a / W4 / W5 / W8 / W9）分开标注，
    **不做「全部完成」表述**。
  - **实测绿**：`cargo test --workspace --locked --lib --tests` 全绿
    （adapter 224 / host 290 / market 74 / proc 46 / settings 99 / schema 131 / types 67）；
    `pnpm -r build` 成功；`pnpm -r typecheck` 20 包全 Done；
    `pnpm -r test` 全绿（contract-tests **147** / wire-gate **126** / host 376 /
    ui 54 / app-cli 288 / app-contract-kit 91 / app-plugin-sdk 37 / framework 40）；
    `pnpm lint` **0 error / 81 warning**。
- **跨主体调用示例切换主推 SDK（0.4-A2，轮 22）**：示例 app 插件页
  `examples/minimal-app/src/plugin/first.ts` 从 legacy iframe SDK 改用
  `@tauron/app-plugin-sdk`（`createPlugin` + `createPluginContext` 接宿主 RPC 面，
  注册声明式命令即开执行泵）；旧实现保留为 `legacy-first.ts` 并标注 deprecated。
  新增 `plugin-window.html` 构建入口（插件面板窗口页，经 `host_window_create`
  依 manifest `entry.ui` 加载进 `plugin-<id>` webview）；主窗新增「打开插件窗口 →
  `callPlugin` → `callTakeResult` 取件」演示段。wire-gate 新增门禁：
  `app-plugin-sdk` 必须有非测试的消费方（示例 app）。
- **进程执行器收口（0.4-A3，轮 21 → 发布审计深化）**：wire-gate 钉死
  `CommandSpawner` 必须 piped stdin/stdout + `thread::spawn` 读线程 + `BufReader`
  排空 + EOF 回收句柄（剥注释后断言，允许诚实边界注释引用历史符号）。
  发布审计将轮 21 的"心跳按插件分账"修复**深化为删除**：原修复针对的
  `ProcRunner` 模拟器全仓零生产调用方（孤儿），连同其全局心跳单例、RPC 回显
  模拟与专属配置类型整体移除——真实链路（`CommandSpawner` + `RuntimeTable`）
  从未有过跨插件共享的心跳状态；心跳监控在真实链路上**尚未实现**（诚实边界）。
- **调用投递闭环（0.4-A1，轮 20）**：`host_plugin_call` 此前只登记 pending、从不投递
  （「有命令 ≠ 有链路」的典型）。现在引入 `tauron-host::call_delivery` 的
  `CallDelivery` 抽象，按插件形态选投递实现：
  - **Js 型**：复用事件总线 request 通道（零新增命令、零新增传输）——宿主经
    `EventBus::deliver_inbound` 写入保留 topic `plugin:<id>:__call`，`app-plugin-sdk`
    取件泵识别 `:__call` 帧、自动执行已注册命令并经 `host_call_result` 回填；
  - **Process 型**：`tauron-proc` 的 `CommandSpawner` 改接 `Stdio::piped()`，每进程
    一个 stdout 读线程持续排空，`ProcSpawner` trait 新增 `write_frame` /
    `register_frame_sink`（缺省实现诚实返回不支持）；`ProcessCallDelivery` 写
    JSON-RPC 行帧（`callId` 内嵌关联），回帧经 `ProcessFrameSinkImpl` 结算。
    **诚实边界**：无真 sidecar，端到端无运行期证据；Rust 层闭环用 fake 启动面验证。
  - **未装配 delivery** → `UnwiredDelivery` 如实返回 `Unsupported`，绝不伪造 `PendingCall`。
  - 新增命令 `host_call_plugin` / `host_call_result` / `host_call_take`（self 档，
    追加在 `ErrorCode`/authz/能力表末尾）；新错误码 `E_CALL_ALREADY_SETTLED`
    （重复回填显式拒绝，不覆盖）。
  - Rust 闭环测试：`process_call_delivers_frame_to_stdin_and_settles_via_sink`。
- `host_*` 命令族共 60 条（底座 39 + 插件运行时 21；`plugin-install` feature 另加
  `host_registry_install` / `host_registry_install_preview` 2 条——该 feature **已进默认特性**
  （`crates/tauron-adapter/Cargo.toml:21`，1.0-W6），默认装配会注册，共 62 条；
  仅在接入方显式 `default-features = false` 时不注册）
- 生命周期状态机：10 态 / 18 事件 / 7 守卫，表驱动 `TRANSITIONS`（表内顺序即优先级），
  `MAX_CHAIN_DEPTH = 2`
- 三档授权 `AuthTier{Self_, ScopedRead, Privileged}`；`self` 档的 pluginId 只从
  webview label（`plugin-<id>`）解析，**忽略调用方入参**（防冒充）
- 崩溃恢复 + 安全模式（启动失败计数、幂等快照、恢复向导）
- 设置中心（schema 注册表 + 四层合并 + get/set/watch）
- i18n 桥、通知中心、白标品牌化、主题/皮肤、schema 规范化管线
- 进程插件 host、WASM supervisor
- Lit Web Components UI 层与 `ui-primitives`

**跨语言契约**

- `@tauron/contract-tests` 的 wire-gate：直接正则解析 Rust 源码文本做断言
  （**当前修订实跑 126 条**：`vitest run src/wire-gate.test.ts`；本包合计 **147** 条 /
  2 个测试文件。该数字随门禁增删与用例合并而变，**以实跑为准**，勿引用历史条目里的旧值）
- 全部跨 IPC 边界的 Rust 结构体强制 `#[serde(rename_all = "camelCase", deny_unknown_fields)]`
- 双套错误码且零交集：框架层 `SC-xxxx`（14 个）/ 应用层 `E_*`（21 个）

**工程与发布基础设施**（本次补齐）

- `LICENSE`（MIT）
- `.github/workflows/ci.yml`：TS（build→typecheck→lint→test）、Rust 默认特性、
  Rust `tauri` feature、示例工程 `substrate-only`、wire-gate
- `.github/workflows/release.yml`：tag 版本一致性校验 → windows / macOS(arm64+x64) / linux
  矩阵构建 → 草稿 Release
- `Cargo.lock` 纳入版本控制（原先被 `.gitignore` 忽略，构建不可复现）
- `ci.yml` 的示例工程 `cargo check` 补 `--locked`；`release.yml` 的版本校验补
  `examples/minimal-app/package.json` 与 `src-tauri/Cargo.toml`（示例才是被打包的
  那个应用，它掉队就会产出「tag 是 v0.2.0、安装包写 0.1.0」），并在 `tauri build`
  前加一步 `cargo metadata --locked` 硬校验示例工程的锁文件（`tauri build` 没有
  `--locked` 选项，`-- --locked` 透传属未文档化行为，不能当可复现性保证）
- 示例工程补齐跨平台打包图标：新增矢量源 `examples/minimal-app/app-icon.svg`
  （1024×1024），经 `tauri icon` 生成 `32x32.png` / `64x64.png` / `128x128.png` /
  `128x128@2x.png` / `icon.png` / `icon.icns` / `icon.ico` 及 MSIX 用的
  `Square*Logo` / `StoreLogo`，并在 `tauri.conf.json` 显式声明 `bundle.icon`
  ——此前 `bundle` 段整体缺失，Linux 的 `deb`/`AppImage` 与 macOS 的 `.app`
  都没有图标来源。Windows NSIS 已重测出包；Linux / macOS **仍未实测**
  （见「已知债务」第 7 条）
- 修 `release.yml` 的 Windows 腿：`校验示例工程锁文件` 步骤写的是 `\` 续行
  （bash 语法），而 windows runner 上 `run:` 默认走 pwsh（pwsh 的续行是反引号），
  于是三行被拆成三条独立语句、第一条变成「`cargo metadata` 带一个 `\` 参数」，
  cargo 非零退出 → 步骤失败 → 整条 Windows 腿失败 → `release` job（`needs: build`）
  根本没执行。**v0.1.0 首次 Release 实测**：windows-x64 于 1m6s 失败，
  `tauri build` 与 `收集产物` 均被 skip（同一提交的 linux/macOS 三腿全部成功）。
  该步骤固定 `shell: bash` 并加注释说明为什么不能删；同时把 Windows 腿的 bundle
  显式钉成 `--bundles nsis`（理由见「已知债务」第 11 条）
- 修 `release.yml` 的 Release 作业：`files` 原先写 `artifacts/**/*`，而
  `download-artifact` 会把 macOS 的 `Tauron Minimal App.app` 整棵目录树摊进
  `artifacts/`，宽 glob 会把 `.app` 内部的 `Info.plist` / `icon.icns` /
  `Contents/MacOS` 里的可执行文件都当成独立 release asset 上传（两个 macOS
  架构的同名内部文件还会互相覆盖）。改为按扩展名挑安装包
  （exe / msi / dmg / deb / rpm / AppImage）。另加一步用带 token 的 `gh` 打印
  该 tag 的 Release 状态（含草稿）——未认证的 `GET /releases` 看不到草稿，
  外部查不出「上次失败有没有留下草稿」，而 action 在已有草稿时会**复用**它
  并走 PATCH 资产那条路
- **v0.1.0 首次 Release 的最终结果**：`版本号一致性` + 四平台构建 +
  `创建 GitHub Release` 六个作业**全绿**（总 287s），产出 linux
  deb/rpm/AppImage、macOS arm64+x64 的 dmg、Windows NSIS 安装包，并落成一个
  **草稿** Release（`draft: true`，需人工过一眼再点发布）
- `rustfmt.toml` / `clippy.toml` / `eslint.config.js` / `.prettierrc.json`
- 本文件

**发布流程加固**（发布前补齐）

- `release.yml` 的 `release` 作业新增**「查看并清理该 tag 已有的草稿 Release」**：
  先打印状态（未认证的 `GET /releases` 看不到草稿，外部查不出来），若该 tag 已存在
  **草稿**则删掉，让 `softprops/action-gh-release` 每次走「新建」这条已被实测验证的
  路径。原因是该 action 会**复用**同名草稿，于是资产走「已存在 → 覆盖」那条路——
  v0.1.0 第二次 Release 正是在这里报的 `Not Found .../releases/assets#update-a-release-asset`。
  只删草稿，**绝不删已正式发布的 Release**；`--cleanup-tag=false` 保证不动 tag
- `release.yml` 的 release 作业新增**策展版 release 正文**（`body`，会**前置**到
  `generate_release_notes` 自动生成的提交列表之前）：这是什么 / 下载哪个 / 安装前
  必读（未签名未公证、Windows 无 MSI）/ 装完怎么自检 / 文档索引。此前只有自动生成的
  提交列表，对访问 Releases 页面的人没有可用信息
- `softprops/action-gh-release` 由 `@v2` 升到 **`@v3`**：v3.0.0 把运行时从 Node 20
  迁到 Node 24（v2.6.2 是最后一个 Node 20 兼容版，已停止维护），且 v3.0.2 含
  「复用草稿时正确发布」「替换已存在资产」「加固流式资产上传」「澄清创建 404」
  等修复，正好命中本仓库踩过的坑

**文档**（本次补齐）

- 新增 [`docs/installation.md`](./docs/installation.md)：此前的**完整缺口**——仓库
  有架构文档、线格式协议、插件开发指南，却**没有一份「怎么装、怎么跑」的文档**。
  覆盖三种「安装」辨析（跑起来看看 / 改代码构建 / 当库接入）、各平台安装步骤与
  未签名未公证的实际处理（SmartScreen / Gatekeeper / `xattr`）、**装完怎么自检**
  的四条演示链路表、从源码构建的完整流程与产物路径、三档装配入口、7 条已知限制
  诚实清单、7 条 FAQ
- README 新增「这是什么」整节：两层架构表、「它不是什么」4 条、成熟度指向三处
  权威源（架构概览的接线状态 / 竞品分析的兑现度标记 / CHANGELOG 的已知债务）、
  「现在适合拿它做什么」适不适合表，并挂上安装文档入口
- **文档数字按实测重测并修正**：ESLint `0 error / 81 warning`（原写 82，已过期）；
  示例工程 README 与 `@tauron/host` README 的 `host_*` 命令数 `45 条` → **60 条**
  （底座 39 + 插件运行时 21，逐条数过 `tauron_substrate_handler!` /
  `tauron_plugin_handler!` 的宏定义）
- `docs/architecture/README.md` 文档地图补 `installation.md` 一行（标注为「落地
  入口——第一次接触先读这份」）；示例工程 README 补「安装包获取与运行」一节

**一键集成路径（第三方易用性）**

此前「一键集成」是**半成品**：`@tauron/app-cli` 的脚手架生成的工程**完全没有 tauron
装配**，`init` 写入的是今天解析不了的 registry 坐标与语义错误的 capability。本轮把它
修成**真能出可编译工程**的路径（判据是实机 `cargo check`，不是「代码里看起来有了」）：

- **`tauron-app new` / `create`**（新命令，`create-tauron-app <dir>` 走同一个实现）：
  生成 `src-tauri/{Cargo.toml,src/main.rs,build.rs,capabilities/default.json,tauri.conf.json}`
  与前端骨架。`src-tauri` 与 `examples/minimal-app` **同形态**——`state_init_with_adapter_config`
  + `tauron_generate_handler![]`（默认 62 条）+ 窗口销毁回收 `cleanup_closed_window`，
  capability 覆盖 `main` 与 `plugin-*` 窗；`--features substrate-only` 走 39 条底座
- **依赖坐标诚实化**：tauron 的 20 个包 / 15 个 crate 都未发布，脚手架不再写
  `workspace:*` / `tauron-adapter = "0.1"` / `extends: '@tauron/base-tsconfig'` 这类
  **今天解析不了**的坐标，改为按检出根生成 `path` / `file:` 依赖（检出根默认从 CLI
  自身位置上溯探测，可用 `--tauron-path` 覆盖；含 Windows 跨盘符回落）
- **`tauron-app init` 修复三处缺陷**：① 依赖由 registry 坐标改为 `path` / `file:`；
  ② capability 语义错误（伪权限 `core:<cmd>`——Tauri v2 无 tauron 命名空间，写入会导致
  校验失败）改为 `core:default`；③ 注入形态由 `.plugin(tauron_adapter::tauri::init())`
  （走 `plugin:tauron|*` 路由但 crate 无 `permissions/` 定义）改为 root 注册
  （`tauron_generate_handler![]`）。**`invoke_handler` 覆盖语义**：目标项目已有
  `.invoke_handler(..)` 时**不改源码**，只把需手工合并的那一行打印出来
- 补 `@tauri-apps/cli` devDep（`tauri dev` 的执行体必须来自 devDependencies）与
  `indexmap` 依赖行（与 minimal-app 同源）
- **前端链路补齐（一键到 `npm run tauri dev`）**：生成 `vite.config.ts`
  （`server.port` 与 `devUrl` 一致 + `strictPort`、`build.outDir` 与 `frontendDist`
  一致、`server.watch.ignored` 排除 `src-tauri/`、`base: './'`）、`tauri.conf.json`
  补 `beforeDevCommand` / `beforeBuildCommand`（拉起 vite）、HTML 入口由 `src/index.html`
  移到**工程根**（vite 的 root 约定，放 `src/` 下 rollup 报 "Could not resolve entry"）。
  `package.json` 脚本改为 Tauri 官方模板规范形状（`dev: vite` / `build: vite build` /
  `tauri: tauri`），并补 `vite` 及各框架 plugin 的 devDeps
  （svelte 编译进产物、不进运行时依赖，故只在 devDependencies）
- **占位图标生成**：新增 `icon-assets.ts`，纯代码构造二进制（PNG 用 deflate stored 块、
  ICO 32bpp BGRA + AND 掩码、ICNS `ic07` 装 128×128 PNG），产出 6 个文件
  `32x32.png` / `128x128.png` / `128x128@2x.png` / `icon.png` / `icon.ico` / `icon.icns`，
  写入 `src-tauri/icons/`。之所以是必需项而非美观项：**Windows 上 `tauri-build` 生成
  资源文件要求 `src-tauri/icons/icon.ico`，因此连 `cargo check` 都需要它**（原先只写
  「`tauri build` 打包依赖它」，低估了这道前置）。产出是**纯色占位图**，CLI 落盘后显式
  提示发布前替换成品牌图标
- **`tauron-app plugin new` 坐标修复（原「已知债务」第 13 条，本轮修完）**：① 生成的
  `tsconfig.json` 不再 `extends: '@tauron/base-tsconfig'`（该包在本仓不存在，`tsc` 必报错），
  改为自包含编译选项；② `package.json` 的 `workspace:*` 依赖改为按检出根生成 `file:`
  坐标（CLI 探测不到检出根时**如实报错退出**，不再退回只有同一 workspace 才解析的写法）
- **`tauron-app init` 的 CLI 接线缺陷（文档与实际不符，已修）**：① 文档一直写的
  `init --dir <你的项目>` **此前根本没接线**——`cli.ts` 只读 `--config`，`--dir` 被静默
  忽略，结果是在**当前目录**上动刀（最危险的一类不一致）；现已把 `--dir` 与
  `--tauron-path` 透传给 `initProject`。② `--tauron-path` 为本轮新增，并在指向非检出根时
  当场失败（判据 `crates/tauron-adapter/Cargo.toml` 不存在），不写指向空目录的依赖。
  ③ 失败分支此前只 `printError` 而**不设退出码**，脚本 / CI 会把「没接上」当成功；现设
  `process.exitCode = 1`。④ 本 CLI README 不再为 `init` 宣称 `--force`（它不支持）
- README / installation.md §3.2 / incremental-adoption.md / 本 CLI README 同步补一键路径
  说明与**诚实边界表**（bundler 配置与图标已由 ❌「需自行补齐」改为 ✅ / ⚠️「占位待替换」）
- **实测证据（2026-09-27）**：`tauron-app new` 落盘 18 个文件（12 文本 + 6 二进制图标）后，
  **不补任何东西**即默认形态（62 条）与 `--features substrate-only`（39 条）两档
  `cargo check` 均通过（`Finished dev profile`）
- **口径声明**：上面这条**只覆盖 Rust 侧**。`npm install` / `npm run tauri dev` **未在本机
  验证**——它们要从 registry 取 `vite` / `@tauri-apps/cli`，沙箱无网络。另有两条前置：
  ① `file:` 依赖是**软链**，`@tauron/host` 解析自身 `workspace:*` 依赖走的是检出根自己的
  `node_modules`，所以检出必须已 `pnpm install`；② `packages/*/dist` 必须已构建。
  这四条边界同步写入 README / installation.md §3.2 / 本 CLI README，避免把「编译过」
  说成「跑起来了」

### Changed

- **内部 crate 互引用补齐 `version = "0.1.0"`**（原「已知债务」第 5 条的坐标部分）。
  此前 11 条内部依赖只有 `path`，`cargo package` 会因
  `all dependencies must have a version requirement specified when packaging` 直接失败；
  现全部补上（根 `Cargo.toml` 4 条 + `tauron-adapter` 4 条 + `tauron-acl` / `tauron-market` /
  `tauron-settings` 各 1 条）。验证：`cargo metadata --format-version 1 --locked` 退出码 0，
  15 个 `tauron-*` 全为 `0.1.0`；并做受控对照（临时移除 `version` 可稳定复现原报错）。
  根 `Cargo.toml` 与 `tauron-adapter/Cargo.toml` 里已过时的注释同步更正——发布仍须
  **按依赖顺序逐个 `cargo publish`**（剥离 `path` 后要去 crates.io 解析内部依赖）
- **TS 用例数口径刷新为实测值 `1729`**（`pnpm -r test`，20 个包 / 101 个测试文件 / 0 failed，
  退出码 0）。此前 `README.md`（徽章 + 测试表）、`competitive-analysis.md` §四、
  `multi-plugin-substrate-roadmap.md` 的口径注写的是 **1701**——已落后于实际（本轮新增
  17 条，另有 11 条是上一轮未同步的漂移）。基线表（标着「轮 12 快照、不随后续改动刷新」）
  保持原值不动
- **15 个 crate 的 `[package]` 元数据统一为 workspace 继承**。此前 9 个 crate 硬编码
  `version = "0.1.0"` / `edition = "2021"`，另有 14 个缺 `repository` —— 升版时这些
  crate 会静默掉队，且 crates.io 元数据不完整。现在全部继承
  `version / edition / rust-version / license / repository / homepage`。
- `Cargo.toml` 的 `repository` 由占位符 `https://example.invalid/tauron` 改为真实地址。
- 20 个包的 `lint` 脚本由 `pnpm run typecheck` 改为 `eslint .`（原值名不副实：
  它不 lint 任何东西）。`@tauron/app-plugin-sdk` 原先根本没有 `lint` 脚本，已补。
- 根 `package.json` 新增 `lint` / `format` / `format:check` / `fmt:rust` /
  `fmt:rust:check` / `lint:rust` 脚本。

### Fixed

- **能力表对 feature-gated 命令误报已注册（0.4-A2）**：`host_registry_install` /
  `host_registry_install_preview` 在 Rust 侧挂 `#[cfg(feature = "plugin-install")]`，
  而 `crates/tauron-adapter` 的 `default = []`——默认构建的宿主根本注册不了这两条。
  （**注**：该 feature 已于 1.0-W6 进默认特性，见上文「1.0 W 系列落地」；
  本条描述的是修复当口的事实，纪律「静态能力表不得硬编码可选命令」与
  feature 的默认值无关，故仍成立。）
  此前它们被无条件列进 TS 的 `FRAMEWORK_COMMANDS`，于是 `capabilities()` /
  `supports()` 对它们返回 `true`，调用方直到真正 `invoke` 才 `command not found`。
  现在这两条移入新的 `OPTIONAL_FRAMEWORK_COMMANDS`，**不进**静态能力表；
  只能由运行期真相开门——`ShellClient.refreshCapabilities()` 调 `host_capabilities`
  并把返回的实际命令集回填传输层（`TauriBackend.adoptCapabilities`）。
  新增 2 条 wire-gate 门禁锁死「TS 可选集 == Rust 可选集」与「可选命令不得出现在静态表」，
  并已做负向突变验证。
- **wire-gate 能力协商门禁在默认构建下假红（0.4-A8-1）**：门禁的 `parse()` 辅助
  按字面串 `indexOf('pub const X: &[&str] = &[')` 匹配，而 `PLUGIN_INSTALL_COMMANDS`
  的声明被 rustfmt 折成 `=\n    &[`，匹配随即失配（`-1`），
  `host_capabilities 命令集与 handler 宏严格同源` 恒红。改为容忍换行的正则，
  并加上「解析到 0 条必须失败」的断言——否则正则失配会表现为"空数组 = 一致"的假绿。
  修复后 `pnpm -r test` 由 1645 含 1 红变为 **1647 全绿**。
- **`tauron-notify` 的活锁风险**：`push` / `trim_to` 的驱逐循环在 `order` 与
  `entries` 失同步时会空转（宿主挂死）。加入收敛保护（一轮未减少即跳出），
  并补 2 个针对性测试。
- **`tauron-settings::merge::write_path` 的可达 panic**：点分路径经过标量中间节点时
  `expect` 会触发。改为按需把中间节点提升为对象，并补 2 个测试。
- **`tauron-theme::ThemeRegistry::default()` 返回空表**：缺省注册表不含内置主题，
  导致 `active_id` 指向不存在的主题。改为默认装载内置 light/dark，并补测试。
- **自动更新的 `relaunch()` 名不副实**：实现走的是 `host_window_quit`（只退出、
  应用不会自己回来），而不是 `host_window_relaunch`（会先把恢复引擎的阶段判定
  对账到插件侧再请求重启）。已改正，并补测试断言「走 relaunch 而非 quit」。
- **`bootstrap.ts` 头注释与代码错位**：注释写「7 阶段」，实际是 8 个阶段，漏掉
  了「上报本轮启动结果」——漏了它每次启动都会被计为崩溃。已补齐注释。
- **`tauron-ui-primitives/src/skeleton.ts` 的 `// @ts-nocheck`**：该指令让整个文件
  不做类型检查。移除后 `tsc --noEmit` **通过** —— 它白挡了检查，没有掩盖任何问题。
- **裸表达式语句**（`signals.test.ts` ×3、`animation-hook.ts`、`component-animation.ts`）：
  改为 `void expr` 并补上「为什么是刻意的」注释（向 effect 注册依赖 / 强制重排）。
- **`plugin-manager.test.ts` 的死变量**：`let callCount = 0` 从未被使用，已删除。
- **`@tauron/cli` 的 `create` / `plugin new` 谎报成功**：两条命令只调生成器就返回
  `Created app/plugin 'x' with N files`，而 `scaffold.ts` / `plugin.ts` **从不碰文件
  系统**——一个字节都没写。现在落盘由 `scaffold-writer.ts` 负责，消息按真实结果分
  三种（真写了 / `--dry-run` 只报告 / 目录已存在且未加 `--force` 时**如实失败**）。
  与轮 12 修掉的 `plugin dev/test/pack/publish` 属同一类缺陷。
- **`bin/tauron.js` 把参数过滤成了「凡以 `-` 开头就丢」**，导致两类静默失效：
  ① `tauron --version` / `--help` 永远回 `No command specified`（`runCli` 里明明有
  这两个分支）；② `--type=wasm` 被丢掉，`tauron plugin new x --type wasm` **静默产出
  js 插件**。改为白名单：只过滤本 wrapper 自己消费的 `--verbose/-v/--dry-run/--force`。
- **`plugin new --type` 只实现了 `--type=x` 一种写法**，与帮助文本写的
  `[--type js|process|wasm]`（空格形式）不符，且非法值被 `as any` 静默降级。
  现在两种写法都生效，非法值**如实失败**并列出可选值。
- **`LazyPluginLoader` 的并发加载 bug**：`load()` 靠 `entry.status === 'loading'`
  判重，但 `_doLoad` 是**排进队列之后**才执行的——在「已排队、尚未开始」的窗口里
  状态仍是 `pending`，两个并发调用会各排一次 `_doLoad`，导致 `entry()` 被调用两次、
  插件被初始化两遍（副作用重复、attempts 被多吃一格）。改为用 `_inflight` Map 复用
  同一 promise，并补 4 个并发回归测试（原测试**完全没有并发覆盖**）。
- **`plugin new` 不产 `tauron.plugin.json`**：只写了 `package.json`，而生命周期命令
  （`dev/test/pack/sign/publish`）读的是前者——于是新插件跑任何生命周期命令都得到
  「no tauron.plugin.json found」。现在两个清单都生成（职责见开发指南）。
- **脚手架产出物不可编译**（5 类缺陷，均经 `tsc --noEmit` 实证）：引用了
  `@tauron/core` **不存在**的 `TauronClient` / `defineConfig`；React 模板的 JSX 写在
  `.ts` 里且引用不生成的 `./App`；`package.json` 无 react 依赖也**不生成
  `index.html` / `vite.config.ts`**（而 scripts 宣传 `dev: vite`）；tsconfig 缺
  `lib: ['DOM']` 与 `jsx`。vanilla / react 两模板现在都能 `tsc --noEmit` 通过。
- **`derivePluginId` 把 `.` 也替换掉**：`com.acme.formatter` 会退化成
  `com.example.com-acme-formatter`（丢段分隔语义）。改为保留 `.` 并合并连续分隔符。
- **wasm 脚手架引用不存在的 `tauron_wasm::plugin` 宏**：骨架 `cargo check` 必失败。
  改为可编译的 cdylib 骨架，并附 `#[cfg(test)]`（实测 `cargo check` + `cargo test` 通过）。
- **`tauron.plugin.json` 的 `package.json` 模板把 `type` 写成插件形态**：应为 npm 的
  `'module'`，否则 ESM 入口被按 CJS 解析。
- **`@tauri-apps/api` 与 `tauri` crate 的 minor 版本不匹配 → `tauri build` 直接失败**。
  npm 侧锁的是 `2.5.0`，而 `src-tauri/Cargo.toml` 的 `tauri = "2"` 解析到 `2.11.6`；
  Tauri CLI 会在打包前做 major/minor 一致性检查，于是
  `Found version mismatched Tauri packages: tauri (v2.11.6) : @tauri-apps/api (v2.5.0)`
  并中止。**CI 的 `release.yml` 同样会红**（`tauri-action@v0` 走同一条检查）。
  已把 4 处声明统一升到 `^2.11.1` / `2.11.1`（根 `package.json`、`@tauron/core`、
  `@tauron/host` 的 peer + devDep、示例工程）。
- **`E_ABI_MISMATCH` 是一条三段式断链（孤儿错误码）**：`tauron_proc::validate_abi`
  在生产代码里**零调用点**（只有测试），`ProcError::AbiMismatch` 因此不可达，
  `ErrorCode::E_ABI_MISMATCH` 便**永不产生**——而 TS 侧 `shell-client.ts` 的文档却在
  承诺它。修复：在 `cmd_runtime_spawn` 的 spawn 前校验里接上 `validate_abi`（比对
  宿主 ABI 契约 `current_abi_contract()` 与调用方声明的 `profile.abi`），把
  `AbiMismatch` 从「并入 `E_INSTALL_FAILED`」改为**独立成码**，并导出两端同值的
  `SIDECAR_ABI_CONTRACT`（TS）/ `SIDECAR_ABI_RUST_VERSION` + `SIDECAR_ABI_INTERFACE_HASH`
  （Rust），由 wire-gate 门禁锁死同值。
- **熔断器只写不读（`circuit_open` 无任何生产读取点）**：`IncrementFailure` 会把
  `failure_count` 累积到 `CIRCUIT_THRESHOLD` 并置 `circuit_open`，但**没有任何迁移
  规则读它**——熔断打开后照样按预算自动重试。修复：为 `ENABLED` / `RUNNING` 的
  `ErrorRetryable` 补 `Guard::CircuitOpen` 规则（熔断打开 → 直接 `ERRORED_USER_CONFIRM`，
  不再自动重试）。同时删除 3 个**无任何规则引用**的死代码（`Guard::CircuitClosed` /
  `TrialBudgetAvailable` / `Action::SetCircuitOpen`），并补门禁
  `every_guard_and_action_is_referenced_by_a_rule` 防止再出现「有实现、没入口」。

#### 全链路审计（三轮）修复的断链与孤儿

- **生命周期零耗环门禁漏检自环**（`tauron-host/src/lifecycle.rs`）：注释声称「自环在
  调用处单独处理」，实际无此代码——`chain.is_none()` 的自环规则会绕过门禁。现在
  门禁显式覆盖自环分支，并补测试 `zero_cost_cycle_gate_actually_catches_bad_self_loops`。
- **流句柄表无上限且终帧不回收句柄**（`tauron-host/src/stream.rs`）：`open()` 无容量闸，
  `push()` 终帧后只置 `closed` 而不摘除句柄 → 句柄表单调增长。新增
  `MAX_STREAMS = 1024` 与终帧回收，新增错误码 `E_STREAM_FULL`（追加到枚举末尾）。
- **`check_heartbeat` 跨插件污染**（`tauron-proc/src/spawn.rs`）：签名缺 `plugin_id`，
  无差别把所有 Running 进程标 Crashed。改为只标记目标进程。
- **shell 事件总线无订阅上限**（`tauron-shell/src/eventbus.rs`）：新增
  `MAX_SUBSCRIBERS_PER_TOPIC = 256`，超限丢弃并计入 `dropped_count`。
- **`host_stream_write` 在 TS SDK 无入口**：`host.ts` 补 `openStreamHandle()` /
  `writeStream()`，导出 `StreamHandle` / `StreamWriteInput`。
- **`Registry::install` 无生产入口**（孤儿 API）：新增
  `tauron_adapter::install_plugin_from_json`（含失败留痕），示例工程接线。
- **`ClientConfig` 无生产消费方**：新增 `AdapterConfig::from_client_config`。
- **`call_status`（TTL 校验）无生产接线**：接进 `Registry::end_call`——过期调用返回
  `E_CALL_TIMEOUT` 而非「成功」。
- **`Registry::install` 的检查-写入竞态（TOCTOU）**：并发安装同一 id 会静默覆盖。
  改为单写锁内原子完成的 `try_put_entry`，补两个并发回归测试。
- **设置只改内存态、不落盘**（`tauron-adapter/src/lib.rs`）：`cmd_settings_set` 写成功
  但重启即丢。新增 `settings_path` + 原子写 `persist_settings_doc` / 装配期
  `load_settings_doc`；`adopt_legacy` / `migrate` 同样落盘。补 3 个跨进程回归测试。
- **`@tauron/cli` 生成的插件模板引用不存在的 `ctx.pluginId`**：生成代码过不了 `tsc`。
- **`registerPlugin` 的 `onDisable` 声明了却从未触发**：补
  `TAURON_DISABLE_EVENT` 接线 + `bridge.notifyDisabled()`。
- **`WindowState.save()` / `clear()` / `apply()` 空 `catch {}`**：localStorage 不可用时
  静默失败。改为记录 `lastError` 并返回布尔结果。
- **`tauron-brand::apply_env_overrides` 用 `let _ =` 吞掉写失败的键**：新增
  `apply_env_overrides_reporting`，如实返回被丢弃的点路径。
- **`ShellController` 用户动作失败只 `console.warn`**：新增 `onError` 回调
  （缺省回落 `console.warn`），标题栏/更新/插件开关的失败都能进 UI。
- **插件卸载无入口**（「有接口、无入口」）：新增 `oc-plugin-uninstall` 事件 +
  `<oc-plugin-manager>` 的卸载按钮 + `ShellController` 接线。
- **`PluginManagerStore` 非测试零消费者**：`<oc-plugin-manager>` 的开关/卸载按钮
  现经 `SHELL_EVENTS` 契约接线到 `host_registry_admin`。
- **`tauron-adapter` 重复装配时静默丢弃 `plugin_flags` 注入**：冲突改为留痕告警
  （同一底座装配两个插件运行时会写向旧注册表）。

#### 第三轮（终检）：两套真相、谎报状态、无界增长

- **删除 `lifecycle::BootTier` / `boot_tier()`**（**两套真相 + 一套死代码**）：本模块
  的档位判定（`Repair` if 已在安全模式，否则 `Safemode` if 连续失败 ≥ 2）与真正在跑的
  `tauron_recovery::BootCounter::decide_phase()`（阈值常量在那边）**语义并不等价**，
  且 `boot_tier()` 在生产代码里零调用点——只有单测在调它。留着它，读者会以为它权威，
  改了恢复引擎的阈值它也不会跟着变。现在启动档位的唯一权威是 `tauron-recovery`。
- **`Registry::install` 的失败文案谎报状态**：校验失败路径用 `let _ = try_put_entry(...)`
  记 `INSTALL_FAILED`，写入失败（同 id 已存在 / 注册表已满）被静默吞掉，而返回的错误
  文案仍宣称「已记入 INSTALL_FAILED，可卸载」——调用方会据此去列表里找一条并不存在的
  记录。改为按真实写入结果分支文案。
- **`install_plugin_from_json` 同一问题**：`record_install_failure` 的结果被丢弃，
  文案同样谎报。已改为如实分支。
- **`ContributesRegistry` 无容量上限**：`host_contributes_register` 是 `self` 档命令，
  只按 `(plugin_id, id)` 去重——换个 `id` 就能再插一条，`Vec` 无界增长。新增
  `MAX_CONTRIBUTES = 4096`，到顶返回 `E_REGISTRY_FULL`（如实拒绝，不静默丢弃）。
- **`MemoryWindowSink` 留痕无上限**：未注入平台 sink 的宿主（非 Tauri / 降级装配）
  用的就是它，每次窗口操作永久追加一条。改为环形（`MAX_WINDOW_OPS = 512`，丢最旧）。
- **`tauron-settings` 的变更订阅队列无上限**：`broadcast` 无条件 `push`，订阅者不
  `drain` 时队列随每次写入无限增长。新增 `MAX_PENDING_EVENTS = 1024`（环形）。
  同时给 `watch()` 补上**诚实边界**文档：它目前**没有生产调用点**，`plugin_id` 参数
  被忽略（广播不按命名空间过滤）——要用得先在宿主侧接一条事件出口。

#### 第四轮（发布收口终检）：self 档命令的归属校验

- **`host_call_end` / `host_cancel` 跨插件越权**（`tauron-adapter`）：两条命令在
  `authz::COMMANDS` 里登记为 `self` 档（契约是「只能作用于调用者自己」），但 wire
  包装器既不解析 webview label、核心函数也不比对归属——任意插件一旦拿到别人的
  `callId`，就能终结对方的 pending call 及其名下**全部流**（越权 + 拒绝服务）。
  现在两条命令的主体从 label 派生（`Caller::from_label`，与 `host_call_result` /
  `host_call_take` 同源），核心函数先取 pending 条目、再比对归属主体
  （`pending.pluginId` = 发起方，主窗发起的调用归属键为 `"main"`），不符即
  `E_AUTH_DENIED` 且**零副作用**。补回归测试
  `call_end_and_cancel_reject_cross_plugin`。
- **显式登记 `host_events_unsubscribe` 的身份口径**（`tauron-host/src/authz.rs`）：
  它是唯一不按 label 绑定的 self 档命令——凭据是 `subscribe` 返回的**不可猜 token**
  （能力 token 模型，token 只回给订阅者本人，分组 token 另有 `subscriber` 一致性
  校验）。在授权表里写明这条边界，免得后续读者把它误当越权入口或误改成 label 绑定。

#### 第五 / 六轮（发布收口终检）：跨主体链路的诚实边界与示例能力面口径

- **跨主体调用「无流式通路」显式登记**（`docs/api/plugin-development-guide.md` +
  `docs/architecture/app-layer-wire.md`）：`host_call_plugin` 线格式**没有** `channel`
  形参、`call_begin_cross` 不绑帧载体，TS 侧 `callPlugin` 也一元——跨主体调用今天
  **没有**流式通路；流式 `host_stream_*` 只服务 `host_plugin_call`（自带后端）这条
  路径，且身份取自 `plugin-<id>` label，故**主窗不能开流**。这是**两侧对称**的
  「未接线」——只补一侧会立刻变成断链，故按仓库规矩如实登记而不是留白。
- **示例能力文件口径修正**（`examples/minimal-app/src-tauri/capabilities/default.json`）：
  描述里仍写「54 条 `host_*` 命令」（过时两代），已改为「底座 39 + 插件运行时 21；
  `plugin-install` 已进默认特性 → 默认 62 条」，与其余文档同口径。
- **交叉复核（无缺陷，仅登记结论）**：`host_stream_write` / `host_stream_close` 的
  归属校验在 `stream.rs::push`（`handle.subscriber != subscriber → E_AUTH_DENIED`）；
  `host_events_drain` / `host_stream_*` 的身份一律由 label 派生（`Principal` /
  `resolve_self_identity`）；`host_resource_stats` 的逐插件计数与
  `pending_for` / `stream_active_for` / `subscription_count` / `group_counts`
  **同源**（不存在第二份计数状态）。

### 文档

- **删除已失效的历史文档**（4 份，均无引用或状态为假）：

  | 删除 | 原因 |
  |---|---|
  | `docs/architecture/client-foundation-upgrade-plan.md` | 2026-01 审计基线（TS 1122 / Rust 984），数字落后两代 |
  | `docs/architecture/tauron-substrate-refactor-plan.md` | R1–R8 已完成，文档头却标「设计稿（待执行）」——状态是假的 |
  | `docs/architecture/competitive-analysis.md` | 与详版重复，且含已失效声明（「架构文档缺失」「GitHub Actions 未确认」） |
  | `docs/api/openapi.yaml` | **旧品牌 `open-client` 的 HTTP API 规范**，与 tauron 信封协议无关，零引用 |

  耐久产出未丢：R1–R8 的架构决策与六条硬边界沉淀进 `overview.md` 新增的
  「架构演进」一节；canonical 结论在 `canonical-owners.md`。
- 竞品分析由带中文 + 日期的文件名改为稳定路径
  `docs/competitive-analysis/competitive-analysis.md`。
- **修正 `overview.md` 的过期接线状态**：原文写 `tauron-settings` / `tauron-proc`
  「尚未接入适配层运行时」，但两者都已在 `tauron-adapter/Cargo.toml` 里且被真实调用
  （`SettingsStore` / `CommandSpawner` / `CrashTracker`）。改为按依赖表分「已接线 /
  未接线」两栏列证据。
- **修正 `incremental-adoption.md` 的过期行**：原文写设置「接线进行中，`Cargo.toml`
  尚未声明 `tauron-settings`」——已声明，且 `host_settings_migrate` 已注册成命令。
  该行从「尚未接线」表移入「已接线」。
- `README.md`：文档表与项目结构树与实际文件对齐（补上此前漏列的
  `shell-events` / `ui-primitives` / `plugin-context-contract` 三个包）；「快速开始」
  与「CLI 工具」两节不再给出装不上的 `pnpm add @tauron/...` / `npx tauron`。
- **重写 `docs/api/plugin-development-guide.md`**（171 → 512 行）。原文档有两处
  **内容错误**：① 权限表列的 `store:read` / `http:fetch` / `clipboard:read` 等
  **全部不在真实词表** `schema/permissions.index.json` 内（会被宿主
  `PluginManifest::validate` 拒绝）；② `package.json` 示例写 `"type": "js"` 并塞入
  `permissions`（应为 `type: module`，权限在宿主清单里）。另补齐此前缺失的章节：
  `tauron.plugin.json` 清单字段表、宿主命令面（16 条档位表 + 底座命令说明）、
  两套错误码表（`SC-xxxx` 14 个 / `E_*` 19 个）、生命周期状态机（10 态 / 18 事件 /
  3 个预算）、sidecar ABI 契约。
- **修正 `docs/architecture/app-layer-wire.md`** 的 `host_runtime_spawn` 失败码清单：
  补 `E_ABI_MISMATCH`（原文只列了 `E_PLUGIN_TYPE_NO_RUNTIME` 与 `E_LEASE_EXPIRED`），
  并说明它独立于 `E_INSTALL_FAILED` 的理由。
- **全仓文档数字与实测对齐**（三轮审计的副产物，逐条按命令重测）：

  | 位置 | 原文 | 实测 |
  |---|---|---|
  | `README.md` 徽章 + 测试表 | TS 1577 / Rust 1258 | TS **1626** / Rust **1284** |
  | `README.md` 契约测试行 | wire-gate 125 | wire-gate **110** |
  | `CHANGELOG.md` 跨语言契约节 | wire-gate 125 | **110** |
  | `CHANGELOG.md` / `errors.ts` / 开发指南 / `app-layer-wire.md` | `E_*` 18 个 | **19** 个（新增 `E_STREAM_FULL`） |
  | `plugin-development-guide.md` 状态机 | 55 条规则 | **57** 条 |
  | `incremental-adoption.md` | 插件域差集 15 条、shell 17 条、计数输出 50 | **16** / **18** / **54** |
  | `competitive-analysis.md` §四 | Rust 1221 / TS 1577 / 97 文件 | **1251**（执行）/ **1626** / **98** |

  另把「Rust 测试」的**两个口径**在 `competitive-analysis.md` 脚注里讲清楚：
  README 给的是源码 `#[test]` **声明数**（1284，含 feature 门控），
  `cargo test --workspace --lib --tests` 的**执行数**是 1251——两者不要混读。
  `multi-plugin-substrate-roadmap.md` 的基线表按它自己声明的「轮 12 快照、不随后续
  改动刷新」保持原值，只在下方口径注里给出当前实测值。

### 已知债务

以下为**仍未解决**的已知问题。已结清的条目**就地标注并保留原因**（便于回溯），
不再计入未解决债务。发布时不要把未解决项描述为已完成。

**1. Rust 格式化与 lint —— 已结清（1.0，2026-09-27）**

`cargo fmt --all` 已一次性落地并验证编译 / 测试；`cargo clippy --workspace
--all-targets --locked -- -D warnings` **零告警**（真修 5 处 + 2 处带中文原因的
`#[allow]`，无 crate 级放行）。`ci.yml` 的 `fmt` / `clippy` 两个 job 已删除
`continue-on-error` 转**硬门禁**——再出现差异或告警就是**新引入**的问题。
（原值：`cargo fmt --all -- --check` 691 hunk / 59 文件、clippy 从未运行。）

**2. Prettier —— 已结清（1.0，2026-09-27）**

全仓已一次性 `pnpm format` 规范化（`.prettierignore` 有意排除 Markdown——docs/ 下
是人工排版的中文长文，重排无收益且破坏可读性）。`pnpm format:check` 现零差异，
并已作为**硬门禁**接入 CI 的 TypeScript 作业。

**3. ESLint 的 81 条 warning —— 已结清（1.0，2026-09-27）**

`pnpm lint`（`eslint . --max-warnings 0`）现为 **0 error / 0 warning**，已是硬门禁。
（原 81 条 warning 绝大多数是测试文件里的未用导入。）

**4. Rust 测试未在有网络的环境执行过**

本仓库的 `cargo test --workspace` 结果以 CI 为准。README 徽章上的 Rust 数字
是源码 `#[test]` 声明数（静态计数），**不是**执行结果 —— feature 门控
（`tauron-adapter` / `tauron-shell` 的 `tauri` feature）另计，两者不同口径。

**5. `crates.io` 发布：拓扑序列已脚本化，但未在真实 registry 上验证**

内部互引用的 `version` 已全部补齐（15 个 crate 全部 `0.1.0`）。此前 `cargo package`
会因 `all dependencies must have a version requirement specified when packaging`
直接失败，现已不再触发——`cargo metadata --format-version 1 --locked` 退出码 0，
且做受控对照（临时移除某个 `version`）可稳定复现原报错。

`cargo publish` 会在剥离 `path` 之后去 crates.io 解析内部依赖，因此**必须先把
被依赖的 crate 发出去、再发依赖方**。该拓扑序列现由 `scripts/publish-crates.mjs`
排定并脚本化（默认只校验，真发布需显式 `--publish` + token）；CI 侧由
`cargo package --workspace --no-verify --allow-dirty` 守住产物 manifest。
**仍未在真实 registry 上验证过**——发布公告里不要写成「已可发布」。

**6. npm 包全部 `private: true` —— 已结清（1.0，2026-09-27）**

19 个 `packages/*/package.json` 已去 `private` 并补发布元数据（仅
`@tauron/contract-tests` 是内部测试包，保留 `private`）；新增
`scripts/publish-npm.mjs` 逐包 `pnpm pack` 并**解包**校验（含 `dist/`、入口齐全、
无 `workspace:` 残留）。**仍未在真实 registry 上发布过**——序列与产物已就绪，
不等于「已发布」。

**7. 示例工程打包图标：配置已补齐，但仅 Windows 实测过**

原先 `icons/` 只有 16×16 `icon.ico`，且 `tauri.conf.json` 完全没有 `bundle` 段
——Linux 的 `deb` / `AppImage` 要 png、macOS 的 `.app` 要 icns，两头都没有来源。

现已补齐：矢量源 `examples/minimal-app/app-icon.svg`（1024×1024，经
`tauri icon` 生成全平台图标集），`icons/` 含 `32x32.png` / `64x64.png` /
`128x128.png` / `128x128@2x.png` / `icon.png` / `icon.icns` / `icon.ico`，
外加 MSIX 用的 `Square*Logo` / `StoreLogo`；`tauri.conf.json` 显式声明了
`bundle.icon`（此前 `bundle` 段整体缺失）。

**已实测**：Windows 上 `tauri build --bundles nsis` 出包成功。**仍未实测**：
`deb` / `AppImage` / `.app` / `.dmg` 没有在任何 Linux / macOS 机器上跑过。
图标与配置现在是「就绪」，不等于「验证过」——发布公告里不要写成跨平台已验证。

**8. 命令面缺口 —— 已结清（1.0，2026-09-27）**

menu / tray / fs / http / updater 五域共 **15** 条 + 主题域 **3** 条已补齐，
全部主窗专属；底座命令面 **39 → 57**，全量 **60 → 78**，默认特性（含
`plugin-install` 2 条）**80 条**。**诚实边界**：`host_http_request` 因离线依赖闭包内
无 TLS 栈，缺省 `UnavailableHttpSink` 如实降级（可注入 `HttpSink` 真跑），
不是「开箱即用能发 HTTP」。

**9. 孤儿 crate —— 已结清（1.0，2026-09-27）**

`tauron-brand` / `tauron-theme` / `tauron-distribute` / `tauron-wasm` 已进
`crates/tauron-adapter/Cargo.toml` 依赖表，分别承接 `host_brand_info` /
`host_theme_*` / `host_updater_*` / WASM 形态状态层校验。`tauron-shell` 保持孤儿是
**有意为之**（框架层门面，供外部宿主直接依赖），不再算待激活。

> **口径更正（1.0，2026-09-27）**：本条此前把 `tauron-acl` / `tauron-market`
> 也列为孤儿——**已过期**。两者现已在 `crates/tauron-adapter/Cargo.toml` 依赖表内
> （`acl` 用于授权判定、`market` 用于安装验签，均在 `plugin-install` feature 下被调）。

**10. 测试期 peer 依赖告警**

`@testing-library/react-hooks@8` 声明 `react ^16.9 || ^17`，实际装的是 react 18；
`@testing-library/react@16` 缺 `react-dom` peer。属既有状态，不影响测试通过
（`pnpm peers check` 可复现）。

**11. Windows 只出 NSIS，不出 MSI**

`release.yml` 的 Windows 腿显式传 `--bundles nsis`，因此**不产出 `.msi`**。
原因是 `tauri.conf.json` 的 `targets: "all"` 在 Windows 上等于 `nsis + msi`，
而 MSI 需要 tauri-bundler 在**构建期**去 GitHub Releases 下载 WiX v3.14 工具链
（本机实测日志：`Info Verifying wix package` → `Downloading
https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip`
→ 失败即 `failed to bundle project`）。那是一次额外的网络依赖，一旦失败整条
Windows 腿红掉、连 NSIS 也拿不到。NSIS 是 Windows 侧主安装包格式，先保证它
确定可出。

**未实测**：CI 上 `--bundles nsis,msi`（即 WiX 下载在 runner 上是否稳定）——
本机因沙箱代理无法验证。要加回 MSI，需先在 CI 上确认这一步能过。

**12. sidecar ABI 校验的「实际值」无可信来源**

`host_runtime_spawn` 现在会比对宿主 ABI 契约与调用方声明的 `profile.abi`（不符 →
`E_ABI_MISMATCH`），但 `profile.abi` 是**调用方自报**的——这道校验挡的是「配置错配 /
前端用了旧模板」，**不是**「恶意调用方伪造 ABI」。真正的可信校验需要 sidecar 在
RPC 握手时自报指纹（**尚未实现**：0.4-A1 已把 `tauron-proc` 的 RPC 帧循环接线
（piped + 读线程），但握手期指纹自报仍是路线图项）。`validate_spawn_config` 的
签名 / 哈希检查同样只做**格式**校验（非空 / 长度 64），不做真实验签——发布审计
已把零产生点的 `ProcError::SignatureInvalid` / `HashMismatch` 变体删除，未来实现
真实验签时按需新增。这两条**不构成安全边界**，发布公告里不要写成
「已实现进程插件验签 / ABI 校验」。

> **已结清（1.0，2026-09-27）**：原第 13 条「`tauron-app plugin new` 的产出仍有不可解析
> 坐标」**本轮已修完**——`tsconfig.json` 不再 `extends: '@tauron/base-tsconfig'`（改为自包含
> 编译选项），`package.json` 的 `workspace:*` 依赖改为按检出根生成 `file:` 坐标（CLI 探测不到
> 检出根时如实报错退出）。详见「Added · 一键集成路径」，此处不再列为未解决债务。

---

## 版本流程

1. 三处版本号一起升：
   - `Cargo.toml` 的 `[workspace.package] version`（15 个 crate 全部继承它）
   - 根 `package.json` 的 `version`
   - 20 个 `packages/*/package.json` 的 `version`
2. 在本文件顶部新增一节，记下发布日期与本版变更
3. `git tag vX.Y.Z && git push origin vX.Y.Z`
4. `release.yml` 会校验工作区、npm 包与示例应用版本一致，再矩阵构建并自动创建公开 Release

[1.0.2]: https://github.com/coeasy/tauron/releases/tag/v1.0.2
[1.0.0]: https://github.com/coeasy/tauron/releases/tag/v1.0.0
[0.1.0]: https://github.com/coeasy/tauron/releases/tag/v0.1.0
