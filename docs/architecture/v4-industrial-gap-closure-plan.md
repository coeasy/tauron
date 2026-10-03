# Tauron V4 工业级缺口收口方案

> 对照对象：`docs/Tauron-Universal-Industrial-Application-Substrate-Final-Architecture-V4.md`
> §134（DoD 33 项）、§135（A64–A110 + Batch 1–6）、§136（40 条 CI/Release 门禁增量）、
> §137–§138（支持等级与「是否工业级」判定）、§140（12 问治理）、§141（最终验收矩阵）、
> §40（W1–W13 与 1.1/1.2/1.3/1.4/2.0 出口判据）。
> 生成日期：2026-10-02（轮 9 之后的专项对照，不改代码，只出方案）。

---

## 0. 口径与取证方法

沿用仓库既有判据，**不新造口径**：

```text
已落 = 代码里真能用 + 有真实生产消费点 + 有门禁把守
部分 = 三层里缺一条或几条（最常见：类型/实现在，消费点或门禁不在）
未落 = 只有设计文字，或只有自测
```

取证方式：五个只读子代理分域（生产就绪与 profile / 并发与事务 / 安装存储与安全 /
线协议错误码 schema / 发布证明与 CI 门禁）逐项定位 `文件:行`，主线代理另对
**结论级关键断言**做二次核验（`resolve_principal`、`origin_gate` 空清单、
`CreditWindow`/`FairQueue`/`OrderingTracker::observe` 消费点、`retryable()` 真值、
`admin_audit_available` 出口、`host_production_doctor` 三层接线、
`tauron-host` 依赖表、`contracts/target-matrix.json` profile 维度）。

**本文不重复粘贴门禁通过数字**。本轮未重跑的测试计数一律以
`docs/Tauron-...-V4.md` 的「轮 9 门禁实照」表和 `CHANGELOG.md` 轮 9 条目为准；
凡本文写「门禁 exit 0」，指的是那张表里同一轮采集的记录。

一条已核实的口径差异需要登记：`packages/tauron-host/src/host.ts:603` 确实存在
`productionDoctor()`，`examples/minimal-app/src/main.ts:392` 是它的仓内消费点，
因此 A109 属**三层已通**（Rust `production.rs` + 命令面 `host_production_doctor` +
TS 客户端 + 示例）；子代理初判「TS 无消费点」系 grep 截断所致，已纠正。

---

## 1. 结论摘要

### 1.1 整体位置

```text
V4 设计面：完整（A01–A110 + 40 门禁 + 4 层支持等级 + 12 问治理）
代码面  ：Tauri-first substrate 扎实，Universal/Industrial 增量约完成 55–60%
宣称面  ：必须继续按 §138.2 执行——「当前 main != V4 已实施」这句话今天仍然成立
```

按 Batch 统计 A64–A110 共 47 项：**已落 22 项 / 部分 21 项 / 未落 4 项**（轮 11 后按 §3 各表
逐行数得，口径与表内的限定语一致——「已落但有天花板」仍计入已落）。轮 10 记的是
15/21/11，差值全部来自 §3 表在轮 10/11 被逐项刷新而本段没跟着改，这本身就是本文
要防的那类「文档内部断链」。
已落项集中在「并发终态、durable 存储、单一写者租约、发布证明、迁移回滚（A101）」；
未落项集中在「Profile 分层、零 Surface 就绪、跨宿主生产接线、可复现供应链」。

### 1.2 五个最危险的缺口（按「后果 × 修复成本」排序）

| # | 缺口 | 位置 | 后果 | 修复代价 | 轮 10 状态 |
|---|---|---|---|---|---|
| F1 | **任意非 `plugin-` label 即主窗特权** | `crates/tauron-host/src/authz.rs:517-525`（`resolve_principal` 的 `None => Principal::MainWindow`） | 次级窗口/伪造 label 直接拿到 `require_main_window` 保护的 admin 面（该符号在 `adapter/lib.rs` 命中 51 处，含定义与测试）；40+ 主窗命令不在 `ADMIN_COMMANDS` 8 条表内，靠注释约束 | 小（改判定 + 配置默认主窗 label + 反向用例） | **Production 已闭合**：`production_caller_allowed` 要求 label ∈ `main_window_labels`；`resolve_principal` 的开发态语义按设计保留 |
| F2 | **origin 允许清单为空 = 门禁关闭** | `crates/tauron-adapter/src/tauri.rs:2795-2799`（`if allow.is_empty() { return Ok(()) }`） | Production 形态可在**零 origin 白名单**下通过自检：§138.2 点名的这一条今天仍然字面成立 | 小（Production 下空清单视为 not-ready；开发态保留兼容） | **已闭合**：新增独立 readiness 事实 `origin_gate_armed` + doctor `origin-gate` + `ORIGIN_GATE_ARMED_REQUIRED` 启动门 + 门内 `ORIGIN_GATE_NOT_ARMED` |
| F3 | **admin 审计只有开关位** | 旧实现是 `AdapterConfig.admin_audit_available: bool`（宿主自报）→ 只喂 `production.rs` 的 readiness 位；全仓无 audit sink 写入 | doctor 能说「审计可用」，但没有任何一条审计事实被产出——即 §140 的「有文档无真实能力」 | 中（需要事实源：结构化事件 + durable 落盘 + 读口） | **已闭合（轮 11）**：事实源 `crates/tauron-host/src/admin_audit.rs`（链式哈希记录 + `AdminAuditSink` 落盘 `admin-audit.json` + `verify_file` 离线复核 + 512 条环形上限）；写口是 admin 分发的单点 `record_admin_audit`（`adapter/lib.rs`，Allowed/Denied 都留痕）；开关位删除，doctor 的 `admin-audit` 检查项（readiness 字段 `audit_for_admin_operations_available`）改由 `AdminAuditFacts::healthy()`（落盘 ∧ 链完整 ∧ 零写失败）推导，读数快照随 `ProductionDoctorReport.admin_audit` 上线 |
| F4 | **`CreditWindow` / `FairQueue` 是死类型** | `crates/tauron-host/src/admission.rs`（仅本文件 + `lib.rs` re-export 命中） | A79/A80 的「按 owner 公平调度」停在类型层；饿死风险真实存在，只有单测 `admission.rs:228` | 中（接线或按孤儿台账删除并登记） | **轮 11 全部收口**：`CreditWindow` 已接线（轮 10，`StreamRegistry` 唯一额度算术）；`FairQueue` 已删除并登记；`ActivationRecord::verify_bytes` **已接线**（`PluginAssetTrust::verify_asset` `adapter/lib.rs:778→:811` ← 逐资产服务 `tauri.rs:1777`）；`MigrationSnapshot` **已删除**（与 `MigrationReceipt.before` 同一事实的两份表达，A101 改用 `DurableEnvelope` 的宿主设置协议）；`OrderingTracker::observe` 定性为**发布侧 oracle**——生产 publish 走 `issue`，`observe` 由 `v4_host_conformance.rs:94` 钉语义，接收端判定在 TS `EventOrderingWatcher`（A102）。后三条登记在 V4「未接线公开 API 台账」轮 11 小节 |
| F5 | **Local Host Service 未进生产链路** | `LocalHostBroker::new` 只出现在 tests / `v4_local_host_chaos.rs` / `cargo run --example`；适配器与命令面零消费 | §137 的「第二官方 Host」与 §141「Local Host Service 是第二参考 Host」均未达成；Electron/Qt/.NET 路线没有真实依赖点 | 大（A86+A106 一整批） | ⬜ 未动（Batch 3'） |

F1/F2/F3/F4 四项属**同一类病**：判定或类型存在、缺一个 fail-closed 的默认值或一个真实消费者。
这正是 §140 12 问要拦的东西，也是本方案 Batch 0 的全部内容。

### 1.3 1.1 / 1.2 / 1.3 / 1.4 / 2.0 出口判据现状

| 出口 | 判定 | 卡住的判据 |
|---|---|---|
| 1.1 Platform Foundation | ⚠️ 未达成 | W3（一个 Tauri plugin 即得最小 Substrate，当前是 root handler 注册）、W5（无手工生命周期编排）、W2 三 profile 体积门禁 |
| 1.2 Runtime Platform | ⬜ 未达成 | W9 无统一 RuntimeDriver（三条路各自独立）；W13 `tauron-wasm` 无 wasm 引擎；1.2 出口要求 WebView/Process/WASM **过同一份 Runtime Conformance**，今天不存在跨形态同一用例集 |
| 1.3 Extension Platform | ⬜ 未达成 | Manifest V3 字段全无、依赖解析器/锁文件全无、插件级 update/rollback 全无、`host_market_*` 仍 simulated |
| 1.4 Ecosystem | ⬜ 未落（按方案定位 optional） | registry 协议与 catalogue 为零 |
| 2.0 | ⬜ 距离最远 | 15 条判据里 **N-1 兼容、Public Conformance Kit、Cross-platform Packaged E2E、No-Orphan Process Gate、Security Review、≥3 个真实第三方 Consumer** 六条没有证据 |

---

## 2. §138.2「当前仍可观察到的九条」复查

这九条是方案作者自己写下的「不许宣称工业级」的理由，逐条复查最有价值——它直接回答
「还有哪些没实现」。

| §138.2 原文 | 今天 | 证据 |
|---|---|---|
| Tauri 2 是正式支持边界 | **仍成立** | 唯一的 Adapter 是 `tauron-adapter`；Local Host 只有 example（F5） |
| origin empty allowlist = gate disabled | **仍成立** | `tauri.rs:2795-2799`（F2） |
| recovery durability 可关闭 | **已闭合**（改为 fail-closed） | `adapter/lib.rs:5755` 无持久化即 `Quarantined`；`recovery.rs:305/345` 走 `DurableEnvelope::seal` |
| RecoveryAction 幂等生产链未接 | **已闭合**（A92） | 生产命令 `cmd_recover_trial_enable` `lib.rs:6865`，幂等键取持久化 incident 序列 `:6854-6863`，`should_execute=false` 走零副作用分支 `:6874-6881` |
| whole-package memory read | **部分闭合** | 验签流式 64 KiB（`package_signature.rs:182-249`）、解包逐 entry `io::copy`（`adapter/lib.rs:3917-3942`）；但 `verify_tpkg(&[u8])` 兼容入口仍在（`:158`），且**没有任何下载消费者**、RSS 门禁只测 wire 往返探针（`performance-budgets.json process.maxPeakRssKb` → `perf_probe.rs`），安装路径内存从未被测 |
| ErrorCode 顺序耦合 TS gate | **半闭合** | canonical 注册表已按**名集合**比对（`wire-gate.test.ts:252-263` + `contracts/error/error-codes.json`），但 `packages/tauron-host/src/gates.test.ts:63` 与 `packages/tauron-app-contract-kit/src/contracts.test.ts:44` 仍逐序 `toEqual`，`crates/tauron-host/src/error.rs:74,166` 注释仍在教「追加末尾」 |
| E_HOST_PANIC = retryable | **已闭合**（A70） | `error.rs:127 => RetryClass::Never`，`retryable()` 由 class 派生（`:141-143`），wire-gate 锁定 panic=never |
| manifest target 只有 OS 粒度 | **部分** | `TargetSpec` 已是 os×arch×abi×min-os×cpu（`target.rs:41`），`contracts/target-matrix.json` 有 `profiles` 维度；但 `resolve_best`（`target.rs:85`）零生产消费者、`ArtifactVariantResolver` 只存在于注释（`adapter/lib.rs:918`），安装路径不过滤变体 |
| cargo-deny 仍 advisory | **已闭合** | `ci.yml:400-407` `EmbarkStudios/cargo-deny-action` `command: check`，blocking、无 `continue-on-error`；`deny.toml` 含 advisories/licenses/bans/sources |
| main protection 未开启 | **仍成立** | `.github/` 只有 workflows，无 ruleset/CODEOWNERS；`verify-source-ci.mjs` 能断言 required jobs，但**分支保护本身是仓库设置**，需要用户/管理员在 GitHub 侧开启 |

**净结论**：九条里 4 条已闭合、3 条半闭合、2 条字面仍成立（F1/F2 类）+ 1 条属仓库设置（待用户操作）。
文档若只更新「已闭合」而不更新「仍成立」，就变成 §140 说的「有文档无真实能力」。

---

## 3. A64–A110 逐项状态（按 §135.1 批次）

> 列含义：**判定** = 已落/部分/未落；**消费点** = 生产链上的真实调用方；
> **门禁** = §136 对应门禁今天的实际形态（CI 作业 / 仅单测 / 缺失）。

### Batch 1 — 先封生产风险：A64 A65 A66 A69 A70 A91 A92 A93 A109

| A码 | 判定 | 消费点与门禁证据 | 缺口（要做的事） |
|---|---|---|---|
| A64 DeploymentMode + ProductionReadinessCheck | **部分** | 枚举与 `validate/doctor` `tauron-host/src/production.rs:10/70/130`；装配期 panic `adapter/src/tauri.rs:3180`；TS `host.ts:603`；门禁 `tests/v4_host_conformance.rs:69` | Production 下 origin 空清单仍放行（F2）；`caller_identity_policy_enabled` 可被同一空清单满足 |
| A65 Profile V2：Tier 与 Capability Bundle 解耦 | **未落** | 全仓无 `ProductTier`/`CapabilityBundle`；TS 的 tier 是 `AuthTier`（`capabilities.ts:10`），语义不同 | 从零建模：Tier↔Bundle↔命令子集三张表 + 一致性门禁 |
| A66 zero-surface/headless ReadinessSet | **未落** | `headless` 只命中注释（`production.rs:261`、`adapter/lib.rs:259`）；CI 的 `Minimal substrate profile`(`ci.yml:153`) 与最小依赖门(`ci.yml:184`) 是**构建门禁**，不是零 IPC 面就绪 | 定义 `ReadinessSet`（无 window/tray/menu 时的必备项）+ 一条真跑「零 IPC 面启动→健康」的 conformance |
| A69 generated Error Registry | **部分** | 见 §2 半闭合行 | 取消三处顺序耦合；补 codegen（Rust 24 个 `E_*` / TS 数组 / JSON 三份手工同步）；框架侧 14 个 `SC-####`（`tauron-shell/src/error.rs`）与宿主码的关系成文 |
| A70 RetryClass | **部分** | 判定已改（`error.rs:110-134`）、两侧断言齐（`wire-gate.test.ts:273-299`） | §90 的 RetryPolicy/OperationClass/RetryBudget **无类型无实现**；`RetryClass::AutoIdempotent` 当前无码映射 → `retryable()` 恒 false（诚实但字段零信息）；TS 侧无自动重放路径 |
| A91 FaultBoundary / subsystem quarantine | **部分** | `fault.rs:71` panic 后 `NotReady` 确定性拒新工作；生产面仅 settings 一条边界（`adapter/lib.rs:6146` `run_settings_boundary`；四个写租约消费点 `:6177` reconcile / `:6317` set / `:6350` adopt_legacy / `:6388` migrate），重建自 durable、无持久化即 Quarantined（`:5755`，测试 `:11635`）；**轮 11 拆分**：闸门只保留「就绪判定 + 事后登记」（`ensure_ready:63` + `record_panic:85`），边界锁**不再跨磁盘 I/O**，单写者边界交回 `settings_write_lock`（A87），并有行为证明 `concurrent_settings_writes_never_lose_a_key_or_share_a_generation`（`adapter/lib.rs:16671`） | 其余子系统（事件/审批/注册表/http/fs/process）未包边界——一个 panic 仍是全宿主风险；`FaultBoundary::run` 拆分后只剩 fault.rs 单测消费者，按「未接线公开 API 台账」登记为 embedder 面（轮 11 小节） |
| A92 RecoveryExecutor 生产接线 | **已落** | `lib.rs:6865` + dedup 分支 `:6874-6881`；测试 `:10020-10022` | — |
| A93 DurableEnvelope/checksum/generation/quarantine | **已落** | `durable.rs:37-76`；读写口 `adapter/lib.rs:5467-5492/5512`、`recovery.rs:305/345/394`；损坏隔离到 `.corrupt`；测试 `:11849` | — |
| A109 production security self-test / doctor | **已落** | Rust + 命令面 + TS + 示例三层通（§0 末段）；档位 `authz.rs:340` Privileged；测试 `lib.rs:12615`；**轮 11**：`admin-audit` 检查项由真实 sink 的 `AdminAuditFacts::healthy()` 推导，并把读数快照（条数/裁剪/写失败/链完整）一并上线（`production_doctor_report` `adapter/lib.rs:5648-5661`） | 空清单 fail-open（F2）与开关位（F3）均已在轮 10/11 闭合；剩余边界=分支保护未开（0-7，需用户操作） |

### Batch 2 — 固化 Universal Contract：A67 A68 A71 A72 A73 A74 A75

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A67 Wire framing + mandatory json-v1 | **部分** | `wire.rs:110` 解析前查上限；`remote_host_reference.rs:140-149` 先验后分配；消费点全是**校验/回环**（`tauron-ffi/src/lib.rs:267`、`remote_host.rs:535`），唯一副作用式消费是 8 MiB pending args 预算（`registry.rs:40`） | 真实命令面走 Tauri 裸 `invoke`（`tauri-backend.ts:270`），**不穿 envelope、不协商 codec**（`adoptCapabilities:249` 只协商命令集）；无固定二进制头/frame kind/flags/sequence |
| A68 schema extension / unknown-field policy | **部分** | Wire 侧 extensions 收口（`wire.rs:58/62/66`，测试 `:157-163`）；Manifest `manifest.rs:673` | `WireExtensions` 全仓仅 wire.rs 自用；Manifest 无 `manifestVersion`/extensions 通道；Capability「required 未知→incompatible」零实现 |
| A71 TargetSpec + ArtifactVariantResolver | **部分** | `target.rs:41` 模型完整、`current_target_spec:138` 被 `adapter/lib.rs:1101` 消费 | `resolve_best:85` 只有自测；`ArtifactVariantResolver` 仅注释（`lib.rs:918`）；安装路径不过滤变体 |
| A72 C ABI / FFI ownership contract | **已落** | `tauron-ffi` + `conformance/c/ffi_ownership_asan.c`，CI 真跑 ASan + `detect_leaks=1:halt_on_error=1`（`ci.yml:201-206`） | 只有 ASan；无 TSan/Miri |
| A73 ExecutionDomain / thread-affinity | **已落** | `execution.rs:89` 门禁测试 | 仅单测级证据，无真跨线程调度断言 |
| A74 ProviderLifecycle + CapabilityEpoch | **部分** | `provider.rs` 自测齐 | **适配器零消费**：Provider 热替换的 generation 保护未接主链（对照 §141「provider hot swap generation 不混线」） |
| A75 ServiceGraph start/shutdown planner | **部分** | `service_graph.rs:32/83` 真 DAG + 环检测；装配期计算并 panic 防坏图（`adapter/lib.rs:2895-2901`） | 两个序只存字段（`:3017-3018`），**唯一消费者是测试**（`:15607-15612`）；实际装配仍手写固定顺序（`:2910-2960`），退出走 `cleanup_closed_window`（`tauri.rs:288-298`）按窗口手工清 → 无拓扑 shutdown、无 app-exit 逆序回收 |

### Batch 3 — 封并发与资源竞态：A76 A77 A78 A79 A80 A81 A82 A90 A104

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A76 atomic CallState terminal | **已落** | `call_state.rs:49 try_finish` 被 `registry.rs:233` 真实持有；过期判定+终态仲裁+摘除收进同一 pending 锁（`:853-917`），settle/cancel/timeout/teardown 四方全走它；256 轮 4 线程唯一赢家测试（`call_state.rs:71`） | 无 loom |
| A77 CallGraph hop/cycle/re-entrancy | **已落** | `registry.rs:782 begin` → `E_CALL_CYCLE`（Never 重试）；命令面透传 `parent_call_id`（`tauri.rs:684-690` ← `host.ts:322`） | legacy 无 parent 的调用直接成根、不核深度 |
| A78 event causation depth/budget | **已落**（有洞） | `eventbus.rs:854` publish 路径校验；命令面回传上下文（`tauri.rs:493-504`）；测试 `:1618` | 无「A→B→A 调用环 + 事件回流」复合场景 |
| A79 credit-based stream backpressure | **部分**（轮 10 收紧） | stream 一路是真 credit，且**轮 10 起额度只有一个原语**：`StreamHandle.credit: CreditWindow`（`stream.rs` 的 `grant`/`consume`）+ `host_stream_grant` 命令面 | RPC result / state update / telemetry 三类仍无端到端 credit。~~`CreditWindow` 死类型~~ 已接线（`admission.rs` 模块头 + `wire-gate` 双向钉住） |
| A80 hierarchical AdmissionController / fair | **部分** | 双层限额真实存在（global+per_principal，`registry.rs` 的 `AdmissionController` 字段与 `call_begin` admit / 全终态 release） | **按 owner 公平调度仍未落**：`FairQueue` 原为零消费者死类型，轮 10 已**删除并在 V4 台账登记**（半接的轮转队列比不接更危险）。缺额转入后续 Batch「A80 公平调度接线 + 端到端饿死压测」 |
| A81 PolicyEpoch/GrantVersion/DecisionToken | **部分** | `policy.rs:11-107`；`eventbus.rs:553-569` bump_grant、提交前 `validate_scoped`（锁内，锁序注释）；**轮 11 把撤销从「只影响新调用」改成有实效**：`revoke:597` 对「他人声明且非公共」档同时①退订该 `(subscriber, topic)` 的全部既有订阅②经 `drop_queued`（同文件）作废三类通道的待取帧③作废走幂等重放（授权行已不存在也再作一次），公共/自属档刻意不动；命令面 `cmd_events_revoke_as`（`adapter/lib.rs:5603`）→ TS `host.ts` `eventsRevoke`；并发对抗测试 `revoke_racing_publish_leaves_no_revoked_content_behind`（`eventbus.rs:1397`）+ SDK/host 两侧断言 | fs/http/process/secret 完全不重检 token（adapter 内 `DecisionToken` 零命中）；与撤销**并发**且已在撤销前解析到 token 的 publish 仍可能落一帧——靠管理面重放 revoke 收口，要封窗口本身得让发布在 `queues` 临界区内重检审批表（`eventbus.rs` revoke doc 已写明边界） |
| A82 SettingsRevision + post-commit watch | **已落**（范围内） | `tauron-settings/src/store.rs:314-315/471-482`；`commit_settings_change:6056`（`adapter/lib.rs`，`:6070` 把 revision 写进镜像帧）、`cmd_settings_set:6300` stage→persist→commit/restore；SDK `createPlugin.ts:166`；示例 `plugin/first.ts:50`；两侧结构门禁 `settings_commit_has_single_mirror_site`（`adapter/lib.rs`） + `wire-gate:2816-2826/3427-3441/3461` | `getAtLeastRevision()` 未落（`host_settings_revision:6277` 的文档注释自陈无 wire 口）；`cmd_settings_adopt_legacy:6348`/`cmd_settings_migrate:6385` 不产帧也不推 revision → bulk 提交对观察者不可见 |
| A90 stale-handle generation protection | **部分** | runtime handle 一条真走代际：activate（`runtime.rs:223`）→ 捕获 `runtime_generation`（`registry.rs:808`）→ 回帧拒旧代（`process_delivery.rs:44/175`）+ 版本化租约查找（`registry.rs:1287` ← `tauri.rs:827`，测试 `:2867`） | 插件激活（`activation.rs:30`）只带字段不校验；Provider 侧未接（A74 同源） |
| A104 no-lock-across-await gate | **已落**（覆盖面窄） | `ci.yml:88` + `check-no-lock-across-await.mjs:12` | 只扫 `tauron-host`/`tauron-adapter` 两个 crate 的 src，且是文本级而非 AST |

### Batch 4 — 封安装/升级/存储一致性：A83 A84 A85 A87 A88 A89 A94 A101

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A83 InstallReviewToken | **已落**（仅 install 域） | `adapter/lib.rs:759` 类型、`:828` 铸、`:3826-3845` nonce 一次性 `remove` + `review_matches_verified:1009` 绑 package/manifest/permission 三摘要；TOCTOU 靠 `VerifiedPluginPackage.archive:782` 句柄；测试 `:9039/:9085`；TS `host.ts:615` + `wire-gate:1448/1489` | uninstall/purge 走 `cmd_registry_admin`（`tauri.rs:3502`）**无 token**；update 域无消费者；TTL 用墙钟（`:773`）不受 A100 约束；语义仍借 `E_INSTALL_FAILED` |
| A84 immutable installed content + activation integrity | **已落**（范围内） | HMAC 保护的全目录摘要清单（`activation.rs` 写口 `lib.rs:547←:3961`，symlink 直接拒 `:498`）；激活校验 HMAC+generation+当前文件集重算（`:602/644/650/657` ← `tauri.rs:1849`）；篡改/注入测试 `:9196/:9262`；**轮 11**：逐资产服务 `read_installed_plugin_asset`（`tauri.rs:1738`）在 `canonicalize`+`starts_with` 之后、把字节交给响应之前过 `PluginAssetTrust::verify_asset`（`adapter/lib.rs:777`→`:811` `record.verify_bytes`）——无密封记录（安装后注入的文件）与摘要不符同样拒服务，并区分 404（没有这条内容）/403（有这条路径、内容与封的不一致）；`ActivationRecord::verify_bytes`/`ActivationError::IntegrityMismatch` 由此拿到生产消费者 | 摘要复核只覆盖 `plugin-install` 特性的服务路径；probation/commit、以及「摘要不符 ⇒ 事件通道实时撤服务」未建模（撤的是当次请求） |
| A85 PortablePathValidator | **未落**（核心维度缺） | `portable_path.rs:25-54` 只查 NUL/绝对/`..`/盘符；唯一消费者 `scoped_fs.rs:38`；测试 `:75` | 方案 §115 要求的保留名、Unicode 归一、尾点/空格、大小写冲突、长路径前缀**全无**；zip 侧同样缺（`tauron-market/src/lib.rs:150-181`） |
| A87 StorageNamespace + SingleWriterLease | **已落** | 真跨进程 `File::try_lock`（`storage.rs:132`）、OS 自动回收、陈旧 owner 覆盖（`:213-217`）、epoch 递增；装配期 panic（`adapter/lib.rs:2915-2932`）；conformance 真起子进程证 Busy/acquire（`v4_host_conformance.rs:153`） | namespace 硬编码（`:2916-2919`），未与 `tauron-brand/src/lib.rs:77 data_dir` 归属打通 |
| A88 generational plugin/provider/runtime activation | **部分** | runtime 一条已接（见 A90） | `ProviderLifecycle`/`CapabilityEpoch` 适配器零消费；插件激活不校验代际 |
| A89 PackLease / CacheLease | **已落**（库层） | `generation.rs:441/459/481/497` GC 租约测试；跨宿主回收已建模 | 上游缺 Pack Manager（1.3 项），租约只服务引用与状态——登记为「已落但有天花板」 |
| A94 streaming package verify/extract | **部分** | 验签流式 64 KiB（`package_signature.rs:182-249`）、解包逐 entry `io::copy`（`adapter/lib.rs:3917-3942`）、验签/解包共用同一句柄（`:4033/4067`） | 无下载消费者；**RSS 门禁错位**——`performance-budgets.json` 的 32768 KiB 断言的是 wire 往返探针，安装路径内存从未被测；`verify_tpkg(&[u8])` 整包入口仍在（`:158`） |
| A101 migration reversible/snapshot contract | **已落**（宿主设置域） | 事务回滚 + **磁盘回滚镜像**：`cmd_settings_migrate`（`adapter/lib.rs:6385`）先 `snapshot_all()` 再迁；合同 `requires_snapshot` 时先把迁移前用户层落进 `host-settings.rollback.json`（`stage_settings_rollback_image:5895`，自带 `DurableEnvelope`，schema `tauron.host-settings-rollback/1`），**之后**才写正式文档；装配读不回正式文档时按已校验的镜像恢复并**一次性作废**（`:3152-3157`），救不回来才走 `report_settings_load_failure:5780`（生产档 panic 前缀逐字未改）；落盘失败 ⇒ rewind 内存 + 作废镜像（`:6403-6409`）。行为测试 `concurrent_settings_writes_never_lose_a_key_or_share_a_generation`（`:16671`）、重启级 E2E `a101_rollback_image_restores_pre_migration_state_after_a_restart`（`:12778`，三轮：迁移→健康重启不消费→坏文档才恢复）；死结构 `durable.rs MigrationSnapshot` 已删除并登记 | 镜像只在迁移合同要求时产出；recovery/registry 等其它持久命名空间没有同类回滚合同；probation/commit（跨进程两阶段）仍未建模 |

### Batch 5 — 扩跨宿主与安全隔离：A86 A95 A96 A97 A100 A106 A107 A108

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A86 LocalHostBroker + peer auth + single-instance lease | **部分**（非生产接线） | 一次性 challenge/绑定 subject（`local_host.rs:214-217`）、`reclaim_stale:143`；**真 OS 凭据在 reference**：Linux `SO_PEERCRED`（`local_host_reference.rs:132-160`）、macOS `getpeereid:174`、Windows 管道 DACL+模拟 SID `:519/:614/:633` | `LocalHostBroker::new` 只在 tests/chaos/conformance/example；适配器与命令面零消费；CI 只跑 `cargo run --example`；Windows 管道腿不在矩阵 |
| A95 ScopedFsProvider no-follow / handle-based | **部分** | Unix 真句柄式：`scoped_fs.rs:130-215` openat + `O_NOFOLLOW` 逐级、`statat SYMLINK_NOFOLLOW:281`，测试 `:364/:378/:415` | **Windows 无 handle-based**：`*_hard` 全返回 `HardEnforcementUnavailable`（`:320-348`），`platform_enforcement=Partial`（`:83`）；`StdFsSink` 退化为按 `display_path` 重检（`adapter/lib.rs:1849-1877`），插件资产读口（`:710-728`）同样 |
| A96 HttpProvider redirect/DNS/private-network | **部分** | URL 解析取代前缀匹配（`network_policy.rs:53-91`，label 边界 `:75-83`）、逐跳 `authorize_redirect` 且跨源不转凭据（`:228-244`）、私网/字面 IP 拒（`:194-226/:302-346`）、默认 `deny_all`；生产命令 `cmd_http_request:8061-8081` + `authorize_http_url:8045` | `authorize_redirect/authorize_resolution` **除本模块自测外无调用者**：`HttpSink::network_enforcement` 默认 `UrlOnly`（`:2027`），缺省 sink 是 `UnavailableHttpSink`（`:2045`）→ 逐跳/DNS 复检在生产不可达 |
| A97 ProcessSandboxProvider + enforcement descriptor | **已落**（诚实分级=不隔离） | `UnixProcessGroupSandboxProvider` / `WindowsJobObjectSandboxProvider`（`tauron-proc/src/spawner.rs`）均自标 `Partial` 并写明 fs/net/syscall 未隔离；生产 hard-sandbox 语义 = **拒启 + 拒 spawn 双门**（`AdapterConfig::validate_process_runtime_for_start` → `PROCESS_SANDBOX_HARD_REQUIRED`；`cmd_runtime_spawn` 在非 Hard descriptor 下 `E_STATE_INVALID_TRANSITION`；doctor `process-sandbox` 项由 `production_doctor_report` 从 descriptor 推导）；provider 行为各有独立测试 | 仓内无 Hard provider（唯一 `Hard` 是测试替身）→ 代价是「生产形态下进程插件不可用」，这是对外必须写清的能力边界。**未验证清单（轮 11 补登记；`CommandSpawner` 文档注释指向本行）**：① 真 sidecar「收帧 → 回帧 → `settle_call`」端到端——仓内无可执行 sidecar、测试不起真进程，只有契约/格式级证据；② `impl Drop for CommandSpawner` 的连带 tree-aware 回收同样只有代码与结构门禁；③ `kill` 正路（真杀活进程）与 Windows post-spawn attach race 未测。→ 需要集成测试环境（真 sidecar 二进制）才能封口 |
| A100 TimeTrustState + TrustedTimeProvider SPI | **部分** | `require_unexpired` 失败关闭（`time_trust.rs:53`）→ `package_signature.rs:196/359/378` → `adapter/lib.rs:4046/4053`；门禁 `:98/:108`、`production.rs:154` | `with_trusted_time_provider` 只出现在测试装配（`:3333/8940/12638/15581`）→ 生产无 provider；`suspicious_if_skew_exceeds` 零消费者；A83 的 token TTL 仍用墙钟 |
| A106 非 Tauri Local Host 参考应用 | **部分** | `examples/local_host_reference.rs` + `src/local_host_reference.rs:281`；CI 真跑 UDS E2E（`ci.yml:160/210`） | 是 `--example`，**不是可分发应用**；无第三方按文档跑通的证据 |
| A107 FFI/C#/Swift/Kotlin golden conformance fixtures | **未落**（只有静态比对） | `tauron-ffi/src/lib.rs:454-473` 用 `abi-v1.json` 断言 abiVersion 与三个 `TauronStatus` 码值一致，再对 `conformance/{c,csharp,swift,kotlin}` 源码做 `requiredSymbols` 字符串包含断言 | **从未编译/执行任何 C#/Swift/Kotlin**；只有 C 经 ASan 真跑 → §141「C/Native FFI ownership 明确」在 Rust↔C 成立，其余语言是「fixture 文本对得上」而非「 ABI 实测对得上」 |
| A108 local/remote chaos + peer-auth suite | **已落**（框架浅） | `v4_local_host_chaos.rs:13/31/59/81`（challenge flood/重放/takeover 围栏）、`v4_remote_host_chaos.rs:57/109/146/180`（nonce/速率/配额/at-most-once）；CI 四 OS + ubuntu（`ci.yml:156/213/216`） | 无真进程崩溃/网络注入框架（当前是模拟注入） |

### Batch 6 — 发布证明：A98 A99 A102 A103 A105 A110

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A98 pinned reproducible toolchain/actions/provenance | **部分** | `rust-toolchain.toml:2` 钉 1.98.0；全部 workflow 用 40 位 SHA；`generate-release-evidence.mjs:43-56` 校验 pin | **无 provenance、无 SBOM、无签名 tag**（`release.yml:344` 仅 softprops 软发布）；「Reproducible Build Report」无双构建逐字节比对；pin 校验只在 release 期 |
| A99 OS×arch×profile×runtime matrix | **已落** | `contracts/target-matrix.json`（含 `profiles`: substrate/ffi/extension/tauri-desktop）+ `check-target-matrix.mjs:58-63` + CI 四目标真跑（`ci.yml:110-163`）+ `release.yml:308` | profiles 靠 `coverage` 字段声明，非逐 profile 执行 |
| A102 Event ordering contract | **已落**（接收端补齐） | 发布在 `ordering→queues` 原子窗口内 `issue`（`eventbus.rs:849/962`、`ordering.rs:46-76`）；帧元数据铸造 `eventbus.rs:417-426`；命令面透传（`adapter/lib.rs`、`tauri.rs:499`）；跨语言字段级 `wire-gate` + `v4_host_conformance.rs:93-118`；**轮 11**：接收端判定落到 TS —— `EventOrderingWatcher`（`packages/tauron-host/src/events.ts:107`）判 duplicate / gap / revision-regression，gap 报告一次即重同步、revision 不回退记录值，两个生产消费点各接一条泵（`rpc.ts` 宿主取件泵、`tauron-app-plugin-sdk/src/context.ts` 插件内置泵），异常分支**只上报不吞投递**（wire-gate 用花括号配平钉在分支本体上，防止有人顺手 `continue`）；Rust `OrderingTracker::observe` 定性为**发布侧 oracle**（生产 publish 走 `issue`），语义由 conformance 钉住 | 顺序元数据缺失时 watcher 直接 `return null`（老宿主/旁路投递不产生 seq ⇒ 无守卫，静默）；顺序契约不是可协商能力（无 capability 名、无版本位），per-topic FIFO 与 causal 分支未成文 |
| A103 liveness/readiness/degradation split | **已落** | `health.rs:7/15/22/29` + `alive_but_not_ready:60`/`dead:69`；`cmd_runtime_health`（`adapter/lib.rs:5043-5065`）→ `tauri.rs:818` → TS `shell-client.ts:576/597`；门禁 `health.rs:92` + `wire-gate:1334` | — |
| A105 PublicSurfaceLedger | **已落** | `ci.yml:79` + `generate-public-surface-ledger.mjs:105-113` + `contracts/public-surface-ledger.json` | — |
| A110 release evidence bundle generator | **已落**（但强制力为零） | `verify-source-ci.mjs:54-96` 校验 required jobs/steps；`ci.yml:85` 静态 `--check`；`release.yml:127→309` 产出 `tauron-release-evidence.{json,md}`、`conformance-report.json`、`security-report.json`（`:202-217`）并上传资产（`:431`） | 证据断言 CI 绿，但**分支保护未开** → required check 无法真正强制 merge；证据里无 provenance 字段 |

---

## 4. A01–A63 基线：本轮仍未落的部分

方案 §135 写「V4 保留 A01–A63」，但这批里有多条从未登记进 §40 的 P0 表。
按「实体在仓内零命中」判定的未落项（可复现命令：
`rg -l <实体名> crates packages scripts examples`，命中数为 0）：

| A码 | 项 | 现状 |
|---|---|---|
| A01 | 删 shell legacy state engine，改 canonical facade | **未落**：`crates/tauron-shell/src/{registry,eventbus,dispatch}.rs` 头部自陈 `legacy / 非 canonical`（R2-a），`HostState`/`PluginRegistry` 仍在 |
| A06 | Tauri plugin permissions 自动生成 | **未落**：`tauron-adapter/` 无 `permissions/` 目录、无 codegen；权限仍是清单字符串参与摘要（`lib.rs:822-824`） |
| A07 | plugin namespace 形态成为默认接入 | **未落**：默认仍是 root handler 注册（`tauron_generate_handler!`） |
| A12 | SecretProvider | **未落**：全仓无该实体；秘密与普通值同层存储（W6 行已登记） |
| A13 | Message completion push，移除正常调用轮询 | **未落**：`host.ts:478` 仍 `host_events_drain` 拉模型（W11 部分） |
| A15 | RuntimeDriver | **未落**：无统一抽象（W9） |
| A19 | ProcIoDriver shared async reactor | **未落**：实体零命中 |
| A23/A24 | Runtime Pack 格式 + WASM 引擎/WIT | **未落**：`tauron-wasm` 依赖表无任何 wasm 引擎；安装路径只接受 JS 插件（`E_PLUGIN_TYPE_NO_RUNTIME`） |
| A27 | MemoryHost 参考实现 | **未落（命名）**：仓内是 `packages/tauron-host/src/memory-transport.ts`（测试用 transport），不是方案说的 MemoryHost 参考宿主 |
| A37/A35/A36 | HostInstanceId / Principal V2+CallerContext V2 / Surface Model | **未落**：三个实体名零命中（CallerContext 只在 ledger 脚本作为字面串出现） |
| A38 | Protocol Handshake / version negotiation | **未落**：`E_RUNTIME_HANDSHAKE` 无对应实现（规格码名表已登记） |
| A45/A46/A47/A48 | 依赖解析器+锁文件 / RuntimePlacement bounded resolver / OperationCoordinator / InstallTransactionJournal | **未落**：四者实体零命中；placement 只在测试上下文以「包替换」语义出现 |
| A49 | anti-rollback / revocation / repository metadata | **未落**：`anti_rollback` 零命中；Publisher Trust 只到「配了可信公钥才能装」 |
| A51/A52/A53 | TaskScheduler / hard-soft-observed quota / ingress pre-parse limits | **未落**（A53 例外：wire 层有解析前上限 `wire.rs:110`，但不是全局限流模型） |
| A54/A55 | Secrets redaction / telemetry cardinality policy | **未落**：`redact`、`cardinality` 零命中 |
| A60 | Stable API maturity / deprecation gate | **部分**：ledger 脚本与 wire-gate 有 `deprecate` 字面，但无 deprecation 生命周期门禁 |
| A61 | property/fuzz/failure/concurrency 测试矩阵 | **部分**：proptest 在、失败注入在（chaos/fault）；**fuzz / Miri / loom / TSan 全无**（`cargo-fuzz` 无目标目录） |
| A62 | soak / leak trend report | **未落**：无 soak 作业，趋势报告缺失 |
| A63 | release SLO/compat/conformance/security/perf reports | **部分**：conformance/security/perf 报告在 `release.yml:202-217`；**SLO 与 N-1 兼容报告缺** |
| A17 | Windows Job Object / POSIX process group | **已落**：`spawner.rs:163-206/215+` |
| A29/A30 | perf/RSS/installer-size 回归门 / cargo-deny hard gate | **已落**：`check-performance-size.mjs` + `check-release-artifact-sizes.mjs`；deny blocking |
| A31 | main branch required checks/ruleset | **未落**：仓库设置，需人工开启 |
| A32 | legacy iframe demo 移出主 quickstart | **未落**：`examples/minimal-app/src/main.ts` 仍有 `PluginBridge` iframe 演示 |

> A02/A03/A04/A05/A08/A09/A11/A14/A16/A18(部分)/A20(部分)/A21/A22/A25–A28/A33/A34/A40–A44/A50/A56–A59
> 的判定与 §40 P0 表一致或已在 Batch 表中体现（如 A08=fail-closed 已落、A09=InstallationIdentity 已落、
> A11=已落、A21=预算层已落/公平层未落、A50=TLS 证据已落但无消费者、A59=仅文本 fixture）。

---

## 5. §134 DoD 33 项逐条状态

| DoD 项 | 状态 | 决定性证据/缺口 |
|---|---|---|
| Production mode secure-by-default | ✅（轮 11 补齐 F3） | F1/F2 在 Production 闭合：空 origin 清单 ⇒ `ORIGIN_GATE_NOT_ARMED` 拒 + `ORIGIN_GATE_ARMED_REQUIRED` 拒绝启动；未声明 label 不再等同主窗。F3 于轮 11 闭合：特权命令的 Allowed/Denied 都产出**结构化审计事实**（链式哈希 + durable 落盘），doctor 的 `admin-audit` 项由 sink 真实健康度推导，宿主写不了那个布尔位 |
| no desktop-only assumption in core contract | ✅ | `crates/tauron-host/Cargo.toml` 无 tauri/webview 依赖（只有 windows-sys 目标依赖） |
| wire frame pre-allocation limit | ✅ | `wire.rs:110` 先验上限再分配 |
| schema evolution policy | ⚠️ | Wire 侧有；Manifest 无 `manifestVersion`；Capability required-unknown 零实现 |
| target os/arch/abi compatibility | ⚠️ | 模型完整、门禁只断 os×arch、`resolve_best` 无消费者（A71） |
| FFI ownership defined if exposed | ✅ | A72 + ASan 真跑 |
| execution/thread affinity declared | ✅ | `execution.rs:89` |
| ServiceGraph acyclic | ⚠️ | 环检测有（装配期 panic），**拓扑序不执行**（A75） |
| Call terminal state exactly once | ✅ | A76 |
| synchronous call graph bounded / cycle-safe | ✅ | A77（legacy 无 parent 调用不核深度为残留） |
| event causation bounded | ✅ | A78 |
| end-to-end backpressure policy | ⚠️ | 只有 stream 一路（A79）；轮 10 起该路由 `CreditWindow` 单一裁决，但 RPC result / state update / telemetry 三类仍无端到端 credit |
| per-owner fair resource admission | ❌ | 分层 `AdmissionController`（global + per-owner 预算）已真实消费；**按 owner 的轮转调度**没有任何实现——原 `FairQueue` 死类型已于轮 10 删除并在 V4 台账登记，饿死风险仍真实存在，缺额记在本方案 Batch 3' |
| permission revoke TOCTOU semantics | ⚠️ | new-calls-only；既有订阅不退订、provider 不重检（A81） |
| Settings post-commit watch semantics | ✅ | A82 |
| InstallReviewToken binds preview to commit | ⚠️ | install 域 ✅；uninstall/update 域 ❌（A83） |
| installed content integrity retained after install | ⚠️ | 激活校验 ✅，逐资产服务不复核（A84） |
| local IPC peer authenticated | ⚠️ | reference 里真凭据 ✅，生产接线 ❌（F5） |
| shared persistent store has single-writer semantics | ✅ | A87 |
| upgrade generation separation | ⚠️ | runtime ✅ / provider+plugin 激活 ❌（A88/A90） |
| pack/cache GC lease-safe | ✅ | A89（Pack Manager 缺属上游功能，不扣 DoD） |
| panic fault boundary deterministic recovery policy | ⚠️ | 仅 settings 一个子系统包边界（A91） |
| durable records detect corruption/stale generation | ✅ | A93 |
| package verification/extraction uses bounded memory | ⚠️ | 验签/解包流式 ✅，无下载消费者且 RSS 门不覆盖安装（A94） |
| filesystem scope resistant to symlink/reparse TOCTOU | ⚠️ | Unix handle-based ✅，Windows `Partial` ❌（A95）；`PortablePathValidator` 维度缺（A85） |
| network redirects/resolution respect scope | ⚠️ | 策略实现完整但逐跳/DNS 复检无调用者（A96） |
| process isolation capability honestly classified | ✅ | 自标 `Partial` 且生产拒 spawn（A97） |
| build toolchain/provenance pinned | ⚠️ | toolchain/actions pin ✅，provenance/SBOM/签名 ❌（A98） |
| Official target matrix actually tested | ✅ | 四目标原生 runner 真跑（A99） |
| migration rollback/data compatibility proven | ✅（有已写明并钉住的边界） | 事务回滚 + **磁盘回滚镜像** + 重启级 E2E（A101，轮 11）：镜像先于新文档落盘、装配只在坏文档时消费且消费即作废。**边界**：回滚带回的是迁移前的**编码层**——v1 文档存裸键，`cmd_settings_get` 先转义再查，因此带点键要重新迁移才可读（数据没丢，`a101_rollback_image_restores_pre_migration_state_after_a_restart` 把「回滚带回值 → 重新迁移后带回可读性」钉成断言） |
| liveness/readiness/degraded separated | ✅ | A103 |
| no lock held across await/provider callback | ⚠️ | 门禁在，覆盖 2 crate、文本级（A104） |

计数（按上表逐行数得，轮 11 后）：**✅ 16 / ⚠️ 15 / ❌ 1 = 32 行**。标题里的「33 项」沿用
§134 原文口径；本表的行数是 32，差的那一项在本文档里既没有行也没有登记——**记为文档侧
待办**，不是「已达成」。轮 10 此处写的是 13/19/1，与本表长期不同步，同 §1.1 那类断链。
按 §134 末句「少一项不得标 stable/full」——
今天没有任何一条能力线可标 full，对外措辞必须继续按此执行。

---

## 6. §136 40 条门禁 → 实际形态

| 形态 | 条数 | 条目 |
|---|---|---|
| **已在 CI（blocking）** | 27 | Production Config、Wire-gate 侧 Error Registry Drift、Target Variant、Thread Affinity、ServiceGraph Cycle、Call Terminal Race、Call Graph Cycle、Event Causation Loop、Slow Consumer/Credit、Fair Admission Stress、Install Review Token TOCTOU、Installed Artifact Tamper、Local IPC Peer Auth（Linux/macOS 腿）、Single Writer Storage、Generation Isolation、Pack GC Lease、Panic Fault Injection、Recovery Idempotency、Durable Corruption、FS Symlink、HTTP Redirect、Sandbox Honesty、OS×arch×profile Matrix、Clock Trust Fault、Migration Rollback、Liveness vs Readiness、No-Lock-Across-Await、Public Surface Orphan |
| **部分（在 CI 但断言面窄）** | 6 | Headless Zero-Surface（只有构建/依赖门）、Schema Evolution（无跨版本 fixture 矩阵）、FFI Sanitizer（只 ASan）、Toolchain Pin Gate（只在 release 期）、Streaming Installer RSS（门测探针不测安装）、Settings Transaction Watch（单测级，无命令面 E2E） |
| **仅测试内嵌** | 4 | Permission Revocation Race（非并发 race）、Event Ordering Golden Fixture（无 golden 文件）、Reproducible Build Report（`--check` 静态输入）、Wire 有界畸形输入（非 fuzz） |
| **完全缺失** | 3 | **Wire Framing Fuzz**、**Reproducible 双构建逐字节比对**、**分支保护 required check 的强制力** |
| **CI 里根本没有的基础设施** | — | `cargo-fuzz`、Miri、loom、TSan、覆盖率、criterion 基线比对、SBOM、artifact provenance、签名/公证 tag |

结构性结论：**所有「CI 绿」目前都可被绕过**——`main` 无分支保护，任何 maintainer
可直接 push；`verify-source-ci.mjs` 能证明「这些作业存在且成功」，
但没有任何机制要求 merge 前它们成功。这一条不修，Batch 6 的其余成果都是软的。

---

## 7. §141 验收矩阵缺口速查

| 组 | 未满足的判据（其余见上文） |
|---|---|
| Universal | 「Local Host Service 是第二参考 Host」（F5）、「Headless 可零 Surface 运行」（A66）、「TargetSpec 覆盖 os/arch/abi」的消费面（A71） |
| End-to-End | 「provider hot swap generation 不混线」（A74/A88）、「reconnect 后资源可恢复或明确释放」（remote 无消费者） |
| No Orphan | 「no subscription orphan」——A81 revoke 后既有订阅继续收帧；本方案 Batch 0 另登记 5 个死类型/死结构 |
| No Infinite Loop | 全部满足（retry/reconnect/placement/graph/call/causation/migration/recovery/updater 十条中，除 **placement bounded** 因 A46 未落而无从谈起） |
| Production Security | 「Production default fail-closed」（F1/F2）、「permission epoch/revoke safe」（A81）、「local IPC peer authenticated」（F5）、「secrets never enter logs/traces」（A54 未落）、「supply chain anti-rollback/revocation」（A49 未落） |
| Performance / Memory | 「fair per-owner admission」（F4）、「package verification bounded memory」的实测面（A94）、「idle zero meaningless worker/polling」（A13 拉模型） |
| Failure / Recovery | 「StartupReconciler deterministic」——仅 settings 有 `reconcile_settings_boundary`，无统一 A56 协调器 |
| Release Proof | 「pinned toolchain/actions」✅、其余「External Consumer / non-Tauri reference Consumer / N-1 compatibility / security hard gate」中 **N-1 与真实第三方 Consumer 无证据**（A30/A31 之外的 2.0 判据） |

---

## 8. 收口路线图

### Batch 0 — 下一轮就做（全是「默认值改一刀 + 死代码收口」，1–3 天）

原则：不动已发布命令面形状，不引入新依赖，只把 fail-open 改成 fail-closed、
把死类型接线或删除。每项都必须自带一条红→绿的门禁。

| # | 动作 | 落点 | 验收门禁 | 兼容性风险 |
|---|---|---|---|---|
| 0-1 | Production 下空 origin 允许清单视为 **not-ready**（开发/测试态保留缺省放行） | `crates/tauron-adapter/src/tauri.rs:2795`（gate 本体）+ `crates/tauron-host/src/production.rs`（新增 readiness 项） | `v4_host_conformance.rs` 加一例：Production + 空清单 ⇒ doctor `productionSafe=false` 且 invoke 被拒 | 低。存量装配若曾依赖「空清单=不过滤」需显式改为开发态或写清单 |
| 0-2 | `resolve_principal` 对**未知 label 返回 `Invalid`** 而非 `MainWindow`；主窗 label 由装配配置声明 | `crates/tauron-host/src/authz.rs:517`、`crates/tauron-adapter/src/lib.rs:3451/3495` | `authz.rs` 反向用例（伪造/次级 label ⇒ 无 admin）+ `adapter/lib.rs` ADMIN_COMMANDS 档位一致性测试扩至全部主窗命令 | **中**：现网若有非 `plugin-` 的次级窗按主窗调用会被拒，需灰度并在 ledger/接口文档写明 |
| 0-3 | admin 操作产出**结构化审计事实**（而非开关位） | 新 `crates/tauron-adapter/src/admin_audit.rs`，复用 `DurableEnvelope`；`production.rs:98` 的判定改为读真实 sink 状态 | 「有 admin 调用 ⇒ 有审计记录」E2E + doctor 的 `audit_for_admin_operations_available` 由 sink 存在性推导 | 低（新增，默认关） |
| 0-4 | 死类型收口：`CreditWindow`、`FairQueue`、`OrderingTracker::observe`、`MigrationSnapshot`、`ActivationRecord::verify_bytes` | `crates/tauron-host/src/admission.rs`、`ordering.rs`、`durable.rs`、`activation.rs` | 每个二选一：接线并加一条生产消费点断言，或按「未接线公开 API 台账」删除并登记；`generate-public-surface-ledger.mjs --check` 保持 no orphan | 低（库内 API，不在命令面） |
| 0-5 | 取消剩余错误码顺序耦合 | `packages/tauron-host/src/gates.test.ts:63`、`packages/tauron-app-contract-kit/src/contracts.test.ts:44`、`crates/tauron-host/src/error.rs:74/166` 注释 | 三处改为按名集合比对后 `pnpm -r test` + wire-gate 绿；新增一条「乱序声明仍通过」的回归 | 低（测试/注释面） |
| 0-6 | 在文档与 ledger 里登记 `retryable()` 恒 false 与 `AutoIdempotent` 无映射这一事实 | `docs/api/*`、`contracts/public-surface-ledger.json` 注释字段 | 文档一致性检查（沿用轮 9 的文档对账脚本口径） | 无 |
| 0-7 | 开启 `main` 分支保护 + required checks（**需用户在 GitHub 侧授权/操作**） | 仓库设置（`gh api repos/{owner}/{repo}/rulesets`） | `verify-source-ci.mjs` 增一步：从 API 读 required status checks 并与 CI job 名对账 | 无（但属外部状态变更，须用户批准） |
| 0-8 | 主窗 / 宿主 UI 专属面按命令补**代码层**判定（轮 12 追加：文档把权限交给一条不存在的 ACL 机制） | `crates/tauron-adapter/src/lib.rs` 的 `cmd_*_as` + `tauri.rs` wrapper | 每条一个「同命令、插件主体拒 / 主窗主体放行」对照例 + 拒绝零副作用例；刻意不判定的命令逐名进 `wire-gate` 清单并要求注释给理由 | **中**：原先能调到这些命令的插件窗会开始收到 `E_AUTH_DENIED`（本版没有插件依赖这 9 条：示例应用与 packages 全部从主窗侧调用） |
| 0-9 | 命令面接口参考由代码生成，文档不再手写「全量」 | `scripts/generate-command-surface.mjs` → `docs/api/command-surface.md` | `pnpm command-surface:check` 进 CI + `wire-gate` 七向对账（命令集 / 条数 / 孤儿 / 两份人写文档口径 / 判定列 / feature 列 / 无判定清单） | 低（生成物，人改即红） |

**Batch 0 出口判据**：§138.2 的九条 observable gaps 中，F1/F2/F3 三条从「仍成立」变为「已闭合」，
`§134` 的 `per-owner fair resource admission` 与 `Production mode secure-by-default` 两项转 ✅。

> **轮 10 对该判据的修正（诚实口径）**：`per-owner fair resource admission` **不可能**
> 靠 Batch 0 转 ✅——Batch 0 只做「死类型收口」，而轮 10 对 A80 公平调度的处置是
> **删除**（零消费者 + 本版不半接）。这一项的真正出口在 Batch 3'，届时判据是
> 「`call_begin` 按 owner 轮转 + 端到端饿死压测绿」，不是「类型存在」。
> 余下两条按轮 10 实际结果核对：F1/F2 已闭合，F3 未动 ⇒ Batch 0 出口判据**尚未整体达成**。

#### Batch 0 执行状态（轮 10，2026-10-02）

| # | 状态 | 落点与消费点 | 门禁（本轮实测） |
|---|---|---|---|
| 0-1 origin 门装弹 | ✅ 已落 | `authz::production_caller_allowed`（策略本体在 `tauron-host`，默认特性即编译）→ `tauri.rs::origin_gate` 的 Production 分支；readiness 事实 `origin_gate_armed` 由 `AdapterConfig::production_readiness()` 从 `!origin_allowlist.is_empty()` **推导**，宿主无法伪造 | 新增 `wire-gate`「Production 的 origin 门必须装弹…」+ host `production.rs::declared_identity_policy_does_not_arm_the_origin_gate` + host `authz` 5 例 + adapter `production_declared_identity_policy_without_allowlist_is_not_ready` |
| 0-2 主窗 label 必须声明 | ✅ **Production 已落** | `AdapterConfig::main_window_labels` → `effective_main_window_labels()`（留空展开 `["main"]`）→ `ShellExtState` → origin 门；未声明 label 报 `MAIN_WINDOW_LABEL_NOT_DECLARED`，形态非法报 `CALLER_IDENTITY_INVALID` | adapter `main_window_labels_flow_from_adapter_config_and_default_to_tauri_convention` + authz `production_refuses_undeclared_main_window_labels` / `production_rejects_malformed_labels_and_off_list_origins` |
| 0-3 admin 审计事实源 | ✅ **已落（轮 11）** | `crates/tauron-host/src/admin_audit.rs`：`AdminAuditRecord`（链式哈希，`prev_hash` → 下一条）+ `AdminAuditSink`（`DurableEnvelope` 落 `admin-audit.json`，512 条环形裁剪，`verify_file` 可离线复核）+ `AUDITED_ADMIN_COMMANDS` 6 条（判定标准=改状态/授权/供应链，只读特权刻意不入表以免挤掉真实授权事实）；写口是 admin 分发单点 `record_admin_audit`（`adapter/lib.rs:3738`），Allowed 与 Denied 都留痕；`AdapterConfig::with_admin_audit(bool)` **删除**，readiness 与 doctor 都由 sink 的 `healthy()` 推导（`:5655`） | host `admin_audit.rs` 6 例（命令名表自洽、链序、篡改断链、环形裁剪后仍验、落盘回读离线复核、撕裂文件拒开）+ adapter 6 例（`an_admin_call_leaves_a_structured_audit_fact`、`a_denied_admin_attempt_is_audited_without_side_effects`、`doctor_derives_the_admin_audit_check_from_the_live_sink`、`production_without_an_audit_directory_is_not_ready`、`admin_audit_survives_a_restart_of_the_host`、`audited_admin_commands_are_real_dispatched_privileged_commands`，均在 `adapter/lib.rs` 的测试模块）+ `wire-gate`「特权管理操作必须在唯一咽喉点产出结构化审计事实」（命令名集合双向对账） |
| 0-4 死类型收口 | ✅ **已落（轮 11）** | `CreditWindow` **已接线**（轮 10：`StreamHandle.credit`，`grant`/`consume` 为唯一额度算术）；`FairQueue` **已删除**（轮 10）；`ActivationRecord::verify_bytes` **已接线**（轮 11：`PluginAssetTrust::verify_asset` `adapter/lib.rs:778→:811` ← `tauri.rs:1738` 逐资产服务）；`MigrationSnapshot` **已删除**（轮 11，与 `MigrationReceipt.before` 同一事实的两份表达，A101 改落 `DurableEnvelope` 镜像）；`OrderingTracker::observe` **定性为发布侧 oracle**（生产走 `issue`，接收端判定在 TS `EventOrderingWatcher`），后三条登记在 V4「未接线公开 API 台账」轮 11 小节 | `wire-gate`「stream credit 必须由 admission::CreditWindow 单一裁决」+ 轮 11 新增的 A84/A101/A102 三条门禁（含「`MigrationSnapshot` 不得复活」反向钉）；`generate-public-surface-ledger.mjs --check` no orphan |
| 0-5 错误码顺序解耦 | ✅ 已落 | `gates.test.ts`、`contracts.test.ts` 改按名集合；`error.rs` / `errors.ts` 删除「必须追加在末尾」教义；`app-layer-wire.md`、`capability-closure-plan.md`、`full-architecture-refactor-plan.md`、`tauron-host/README.md`、V4 §89 同步改口径 | `pnpm -r test` 20 包 1761 例 rc=0；wire-gate 153 例 rc=0 |
| 0-6 `retryable()` 恒 false 登记 | ✅ 已在档 | 接口文档「重试语义：`retryClass`，不是 `retryable`」小节（早已写明 `auto-idempotent` 一个码都没落）；本轮复核确认该声明与 `error.rs:143` 一致 | 文档一致性人工对账 + `retry_class` 两侧断言（wire-gate） |
| 0-7 `main` 分支保护 | ⬜ **需用户操作** | 仓库设置，代码侧无法闭合 | 见 §5 Batch 0 说明 |

**轮 10 门禁实测**（同一轮内抓取，非回忆）：

```
cargo fmt --all -- --check                       clean
cargo clippy --workspace --all-targets -- -D warnings        rc=0
cargo clippy -p tauron-adapter --features tauri --all-targets -- -D warnings  rc=0
cargo test --workspace --locked                  rc=0  37 suites / 1437 passed / 0 failed
  ├─ tauron-host --lib                           389 passed
  ├─ tauron-adapter --lib                        258 passed
  ├─ tauron-adapter --features plugin-install    272 passed
  ├─ tauron-adapter --features tauri             287 passed
  └─ tauron-shell --features tauri                72 passed
cargo check -p tauron-adapter --no-default-features --locked        rc=0
cargo check --workspace --all-targets --all-features --locked       rc=0
cargo check examples/minimal-app substrate-only --all-targets      rc=0
pnpm -r test                                       rc=0  20 packages / 1761 passed
  └─ @tauron/contract-tests wire-gate             153 passed（含本轮新增 2 条）
pnpm -r --no-bail typecheck / pnpm lint / pnpm format:check        rc=0
node scripts/generate-public-surface-ledger.mjs --check   85 public commands, no orphan
node scripts/check-target-matrix.mjs --check              OK (4 targets)
node scripts/generate-release-evidence.mjs --check        OK (version=1.1.0)
node scripts/check-no-lock-across-await.mjs               OK (41 Rust files)
```

#### 轮 11 执行状态（2026-10-03）

轮 11 的口径：**先复查轮 10 的落点是否真的接在生产链路上**，再补完整性/生命周期缺口。
凡是「测试绿但生产不经过它」的，按未落处理。

| 项 | 状态 | 落点与消费点 | 门禁（本轮实测） |
|---|---|---|---|
| F3 admin 审计事实源（0-3） | ✅ 已落 | 见上表 0-3 | 见上表 0-3 |
| 0-4 死类型收口 | ✅ 已落 | 见上表 0-4 | 见上表 0-4 |
| A84 逐资产摘要复核 | ✅ 已落 | `tauri.rs:1738` 服务路径 → `adapter/lib.rs:778 verify_asset` → `:811 record.verify_bytes`；403/404 分义 | adapter `PluginAssetTrust` 篡改/注入/越权三组用例 + `wire-gate` 服务侧复核钉 |
| A101 迁移回滚镜像 | ✅ 已落 | `cmd_settings_migrate:6385` 先取快照→先落镜像→再写正式文档；装配 `:3152` 消费并作废；`:5780` 是救不回来时的如实记录 | `wire-gate`「A101 迁移回滚镜像必须落盘、先于新文档写、并在重启时被消费」+ adapter 三轮重启 E2E |
| A81 撤销效力 | ✅ 已落（范围内） | `eventbus.rs:597 revoke` → 退订既有订阅 + `:633 drop_queued` 作废三类通道待取帧；`cmd_events_revoke_as:5603` → TS `eventsRevoke` | host `revoke_racing_publish_leaves_no_revoked_content_behind` + `wire-gate` A81 效力钉（两侧文档同步） |
| A102 顺序契约接收端 | ✅ 已落 | TS `EventOrderingWatcher`（`events.ts:107`）接进 `rpc.ts` 宿主泵与 `tauron-app-plugin-sdk/src/context.ts` 插件泵；异常只上报不吞投递 | `wire-gate`「A102 顺序契约必须有接收端判定…」用花括号配平钉异常分支本体 + 三处消费点测试 + `v4_host_conformance.rs:93-118` |
| A91 × A104 设置事务边界 | ✅ 已落 | `run_settings_boundary:6146` 两段式（就绪判定 / 事后 `record_panic`）；写租约成为**唯一**串行化点，四个写点 `:6177/:6317/:6350/:6388` 全持它 | `wire-gate`「设置族闸门必须只判就绪…」+ adapter `concurrent_settings_writes_never_lose_a_key_or_share_a_generation`（**红→绿**：去掉租约 ⇒ 同 generation 或丢键，实测红；恢复 ⇒ 绿） |

**轮 10 复查里揪出的三类「看起来已落、实际断链」**（都值得记住）：

1. **文档与实现互相矛盾**：`settings_write_lock` 的注释声称它是单写者事务边界，而
   `FaultBoundary::run` 把边界锁跨在含磁盘 I/O 的闭包上——实测把 `_write` 换成 `None`，
   并发测试仍全绿。修法是拆闸门（两段式），并给租约加**行为**证明，不是给注释加字。
2. **台账数字与自己的表格脱节**：§1.1 写 15/21/11、§5 写 13/19/1，而逐项数得的是
   22/21/4 与 16/15/1——本轮改为「按表逐行数得」并注明口径，同时登记
   §5 表只有 32 行、标题宣称 33 项这一条**未闭合的文档缺口**。
3. **门禁的否定式判定过松**：A102 旧门用「源码里不许出现 `continue`」表达「异常分支不得
   吞投递」，既抓不到真问题也会因无关 `continue` 误红。改为定位 `if (violation) {` 后按
   花括号配平取分支本体再判定。

**轮 11 门禁实测**（同一轮内抓取，非回忆）：

```text
cargo fmt --all -- --check                                            clean
cargo clippy --workspace --all-targets --locked -- -D warnings        rc=0
cargo clippy -p tauron-adapter --features tauri,plugin-install --all-targets --locked -- -D warnings   rc=0
cargo test --workspace --locked                    rc=0  37 suites / 1454 passed / 0 failed
  ├─ tauron-adapter --lib                              267 passed
  ├─ tauron-host --lib                                 397 passed
  └─ 其余 35 个 suite 全 ok（含 v4_host_conformance / chaos / ffi / wasm / proc）
cargo test -p tauron-adapter --features tauri --locked                296 passed / 0 failed
cargo test -p tauron-adapter --features plugin-install --locked       285 passed / 0 failed
cargo test -p tauron-adapter --features tauri,plugin-install --locked 319 passed / 0 failed
cargo test -p tauron-shell --features tauri --locked                   72 passed / 0 failed
cargo check -p tauron-adapter --no-default-features --locked          rc=0
cargo check --workspace --all-targets --all-features --locked         rc=0
pnpm -r build / pnpm -r --no-bail typecheck / pnpm lint / pnpm format:check   全部 rc=0
pnpm -r --no-bail test             rc=0  20 packages / 106 test files / 1782 passed
  └─ @tauron/contract-tests        162 passed（contract 21 + wire-gate 141；轮 11 新增 9 条：F3/A84/A75/A81/A102/A91×A104/A101/version-sync/docs:check）
node scripts/version-sync.mjs --check                 version-sync OK: 26 处版本号全部为 1.1.0
node scripts/check-doc-line-refs.mjs                  OK：5 个文档 / 204 条引用，行号与符号都可复算
node scripts/generate-public-surface-ledger.mjs --check   85 public commands, no orphan metadata
node scripts/check-target-matrix.mjs --check              OK: linux-x64, windows-x64, macos-arm64, macos-x64
node scripts/generate-release-evidence.mjs --check        OK: version=1.1.0, rust=1.98.0, node=22.23.2, pinnedActions=47
node scripts/check-no-lock-across-await.mjs               OK across 42 Rust source files
cargo check --manifest-path examples/minimal-app/src-tauri/Cargo.toml --features substrate-only --all-targets --locked   rc=0
```

**这条日志里被改过两次的数字，本身就是「凭记忆写数」的反例**：首轮采集时 wire-gate 是 139
（新增 docs:check 门与 version-sync 门之前）、`pnpm -r test` 总数 1780，收尾复跑后分别是
141 / 1782。文档里的门禁数必须**跟着最后一次全量跑**写，否则下一次复核会先怀疑代码。

**轮 11 未做（不得读成已做）**：Batch 1'–6' 的其余条目（A65/A66 Profile 与零 Surface 就绪、
A85 路径校验五维、A86+A106 第二 Host 生产化、A95 Windows handle 强制、A96 重定向/DNS 复检、
A98 provenance/SBOM/签名、A83 uninstall/purge/update token 覆盖、A94 安装路径 RSS 门禁）
一律未动；0-7 分支保护仍需用户在 GitHub 侧操作。发布侧那条**工具链断链已在轮 11 接上**：
仓内原先没有版本号生产者（`scripts/version-bump.mjs` 与 `tauron-build-tools` 早已不在仓内，
「改一处漏五处」只能靠 `release.yml` 的校验事后红），现在由
`scripts/version-sync.mjs` 补齐——`pnpm version:check` 判定 26 处落点同源、
`pnpm version:sync <ver>` 一次写齐，且与 `release.yml` 的 version-check 步骤同一口径。
**仍然成立**的发布侧动作只剩两条：打 tag 与重建 `examples/minimal-app` 的 NSIS 安装包
（本地无 `v1.1.0` 标签，出包需要人推进提交/推送，属仓库动作而非代码缺口）。

#### 轮 12 执行状态（2026-10-03）

轮 12 的口径：**把「接口文档说的权限」和「代码实际执行的权限」逐条对齐**——轮 11 修的
是否接线，轮 12 修的是文档是否在替一条不存在的机制背书。两条都是新登记的 Batch 0 项。

| # | 状态 | 落点与消费点 | 门禁（轮 12 实测） |
|---|---|---|---|
| 0-8 主窗专属面的**按命令**判定补齐 | ✅ 已落 | 9 条宣称主窗专属、实则零判定的命令各得一个 `cmd_*_as(caller, …)` 入口，首句 `require_main_window`：`host_window_quit` / `host_clipboard_read` / `host_clipboard_write` / `host_dialog_open` / `host_dialog_save` / `host_dialog_message` / `host_dialog_confirm` / `host_recover_boot` / `host_i18n_stats`（`adapter/lib.rs`，tauri 侧 9 个 wrapper 注入 `TauriSource` 取 label）。刻意不判定的 9 条（`host_brand_info`、`host_i18n_t` / `_t_params`、`host_window_close` / `_maximize` / `_minimize` / `_restore` / `_set_position` / `_set_size`）逐条在 `///` 注释写理由：几何族的目标窗口是**注入的调用方 label**（入参没有目标 label），i18n 两条一次只答一个键，`brand_info` 只读且不含主体状态 | adapter `main_window_surface_guard_tests` 4 例：九条拒插件主体（码 + 命令名）、**拒绝先于任何写副作用**（剪贴板槽位仍是被拒前的值；`kind` 闭集校验排在判定之后，表外值 + 插件主体先报越权）、主窗主体八条放行 + `quit` 落到内建 sink 的诚实降级、畸形 label 永不降级成主窗 |
| 0-9 命令面全量接口参考改为生成物 | ✅ 已落 | `scripts/generate-command-surface.mjs` → `docs/api/command-surface.md`（85 条逐条：业务形参 / 返回 / 档位 / feature 门 / 实际执行的身份判定 / 前端落点）。三个真坑：feature 属性写在 `#[tauri::command]` **上方**（只扫函数体 ⇒ 85 行全报「无门」）；档位是全限定路径 `tauron_host::authz::AuthTier::Privileged`（只匹配裸名 ⇒ 安装 2 条报 `?`）；判定藏在非 `cmd_` helper 里（`admin_gate` / `visible_notifications` / `scoped_within_roots`），故按调用链遍历（`CS_MAX_DEPTH`，深度 3 与 4 生成的表逐字节相同） | `pnpm command-surface:check`（CI TS 作业前置步骤）+ `wire-gate` 七向对账：命令集合双向、三条数组条数、孤儿必须 0、两份人写文档不得自称全量、判定列含真实判定函数名、feature 列与 tauri.rs 属性逐条同源、「代码层无判定」按**名字**成清单且每条须在自己函数注释里给理由 |

**成因记录（第 4 类缺陷的变体）**：`authz.rs` 模块文档、`capabilities.ts` scope note 与
插件指南「底座命令」三处都写「不在档位表的命令由 Tauri capability 的 `windows` 字段限制
（只授予 `main`）」。这句在代码里零对应：示例能力文件
`examples/minimal-app/src-tauri/capabilities/default.json` 的 `windows` 是
`["main", "plugin-*"]`、`permissions` 只有 `core:default`，而 `host_*` 是 root 注册的裸名，
ACL 不按命令名管辖；`origin_gate` 判的也只是 label + origin 的形态与来源。
**按命令的权限唯一落点就是代码层**——错误口径的后果不是文档不好看，而是评审者以为漏判定
有兜底，于是新增命令时没人再查：本次 9 条正是它养出来的。三处口径已同步更正，
生成器的 fallback 也从「主窗专属（capability）」改成如实的「不在档位表，**代码层无判定**」。

**轮 12 门禁实测**（同一轮内抓取；`cargo fmt --all` 之后所有计数与行号引用复算过）：

```
cargo fmt --all -- --check                        初跑红（本轮新代码未格式化）→ fmt 后 clean
cargo clippy --workspace --all-targets --locked -- -D warnings      rc=0
cargo clippy -p tauron-adapter --features tauri,plugin-install --all-targets --locked -- -D warnings  rc=0
cargo test --workspace --locked                   rc=0  37 suites / 1458 passed / 0 failed
  ├─ tauron-adapter --lib                          271 passed（轮 11 为 267）
  └─ tauron-host --lib                             397 passed
  ├─ tauron-adapter --features tauri               300 passed（轮 11 为 296）
  ├─ tauron-adapter --features plugin-install      289 passed（轮 11 为 285）
  ├─ tauron-adapter --features tauri,plugin-install 323 passed（轮 11 为 319）
  └─ tauron-shell --features tauri                  72 passed
cargo check -p tauron-adapter --no-default-features --locked         rc=0
cargo check --workspace --all-targets --all-features --locked        rc=0
cargo check --manifest-path examples/minimal-app/src-tauri/Cargo.toml --features substrate-only --all-targets --locked  rc=0
pnpm build / typecheck / lint / format:check                      rc=0
pnpm -r --no-bail test                          rc=0  20 包 / 1784 passed
  └─ @tauron/contract-tests 164（wire-gate.test.ts 单文件 143）
pnpm command-surface:check   「85 commands（底座 61 / 运行时 22 / 安装 2），孤儿 0，未归类 0，无代码层判定 9」
pnpm docs:check              「6 个文档，198 条引用 OK」（轮 11 为 5 文档 / 204 条）
pnpm version:check           「26 处版本号全部为 1.1.0」
Public Surface Ledger / Target Matrix / Release Evidence / No-Lock-Across-Await(42 files) / publish:npm --check(20 包)  rc=0
```

一次格式化引出的**第 5 类副作用**：`cargo fmt --all` 把 `adapter/lib.rs` 行号整体推偏，
`pnpm docs:check` 立刻报 7 条漂移。修法不是把数字改对，而是按轮 11 定下的引用口径改成
**符号锚定**（`settings_commit_has_single_mirror_site` 与 6 条 admin 审计测试名去掉行号），
文档从此不随格式化漂移——205 条引用降到 198 条正是这个动作的账面结果。

**轮 12 红探针**（本轮实跑，两条方向不同的假绿）：

1. 代码侧：把 `cmd_clipboard_write_as` 首句换成 `let _ = (caller, "host_clipboard_write");`
   ⇒ `cargo test -p tauron-adapter --lib main_window_surface_guard` =
   「FAILED. 2 passed; 2 failed … 267 filtered out」，且
   `generate-command-surface.mjs --check` rc=1「与代码不同步」；还原后 4 passed / 0 failed、rc=0。
2. 文档侧：把生成物 `host_brand_info` 行的判定写成「仅主窗 `require_main_window`」
   ⇒ `wire-gate` = 「Tests 1 failed | 162 passed (163)」，失败信息
   「host_brand_info 必须如实标成「代码层无判定」」。
3. 口径侧：把示例能力文件的描述改回「Tauri ACL 按命令名管辖它们」
   ⇒ 本轮新增的「示例能力文件不得冒充按命令授权，且必须与 adapter 的 opt-in 口径同源」
   用例红（「Tests 1 failed | 163 passed (164)」），还原后 164 全绿。
   该用例同时钉住三处同源：能力文件描述含「不按命令名管辖」且不含「已在默认特性」、
   `tauron-adapter` 的 `default = []`、示例 `Cargo.toml` 的
   `default = ["plugin-install", "runtime-wasm-broker"]` 与 README 的「故实际注册 **85** 条」。

同一批更正还包含示例侧的两处账面谎言：能力文件的 `description` 原写「`plugin-install`
已在默认特性 → 默认 85 条」（与 `default = []` 相反），`examples/minimal-app/README.md`
原写「已进默认特性 → 实际注册 80 条」（两个数都错）；改完由 `cargo build`（rc=0，
2m 17s）重新生成 `src-tauri/gen/schemas/capabilities.json` 一并入库，
生成物与源文件不留两版描述。

探针顺带划清一处分工，防止后来人误读：`wire-gate` 的判定列断言是**文档自洽**（它读生成物，
不会去解析 Rust 调用链）；代码↔文档的同步由 `command-surface:check` 复算承担，而**该 CI
步骤的存在本身**由 `wire-gate` 钉住（断言 `ci.yml` 含 `pnpm command-surface:check`），
删掉复算步骤即红——链路仍是闭合的，但两头的责任不能混着写。

**轮 12 未做（不得读成已做）**：0-7 分支保护仍需用户在 GitHub 侧操作；Batch 1'–6' 其余条目
未动；发布侧只剩两个人工动作——打 `v1.1.0` 标签与重建 `examples/minimal-app` 的 NSIS 安装包
（本轮代码门禁已全绿，出包需要人推进提交/推送）。

### Batch 1'（对齐 §135.1 Batch 1，2–3 天）

- A65 Profile V2：建模 `ProductTier` × `CapabilityBundle`，与命令面子集做**一致性门禁**（新 `contracts/tier-bundles.json` + `scripts/check-tier-bundle.mjs`）；不做「声明了但没人读」的表。
- A66 `ReadinessSet`：定义零 Surface 就绪集合，落一条真跑 conformance（无 window/tray/menu 启动 → doctor → liveness/readiness 分离）。
- A91 扩边界：把 settings 之外的子系统（事件/审批/注册表）逐个包 `FaultBoundary`，每包一个就补一条 quarantine→reconcile 测试。

风险：Tier/Bundle 一旦进 ledger 就是对外承诺面，须与 §137 支持等级措辞同时更新。

### Batch 2'（Universal Contract 固化，1 周）

- A67/A68：命令面穿 envelope + codec 协商握手（**破坏性**：需 `host_protocol_info` 一条新命令承载协商结果，方案已点名「不硬凑」的两条里这是第一条）；Manifest 引入 `manifestVersion` 与 extensions 通道，配 V2→V3 兼容判定。
- A71：把 `resolve_best` 接进安装/更新路径（变体过滤），`ArtifactVariantResolver` 从注释变实现。
- A75：让拓扑序**真的执行**——装配走 `service_startup_order`、app-exit 走逆序 `service_shutdown_order`，并用一条 E2E 证明逆序回收（当前 `cleanup_closed_window` 按窗口手工清的逻辑保留为兜底）。

风险最高的一步：动的是已发布命令面形状。必须在独立轮次做，并配 N-1 兼容门禁（这也是 2.0 判据之一）。

### Batch 3'（并发与资源，3–5 天）

- A80 公平调度接线：**从空白重新落**（轮 10 已删除零消费者的 `FairQueue` 死类型，见 V4 台账轮 10 小节）。要求一次性带齐三件——`call_begin` 路径真的按 owner 轮转、端到端饿死压测（慢 owner 不拖垮快 owner）、`wire-gate` 反向钉住「准入门必须有消费者」。只补类型不接消费点会重演 F4。
- A79 其余三类资源 credit 建模；A81 撤销三档策略成类型，并把 revoke 后果扩到「既有订阅退订 + provider 重检」。
- A78 补「调用环 + 事件回流」复合用例。

### Batch 4'（安装/存储一致性，1 周）

- A83 token 覆盖 uninstall/purge/update；TTL 从墙钟改走 A100 的可信时间。
- A84 逐资产服务复核摘要（`read_installed_plugin_asset`）。
- A85 PortablePathValidator 补齐保留名/归一/尾点/大小写/长路径五维，zip 侧同步。
- A94 安装路径 RSS 门禁（把 `maxPeakRssKb` 断言从探针迁到真实安装流；删或降级 `verify_tpkg(&[u8])`）。
- ~~A101 `MigrationSnapshot` 落盘 + 进程重启级回滚 E2E~~ **轮 11 已落**（改为宿主设置域的磁盘回滚镜像 + 三轮重启 E2E；死结构 `MigrationSnapshot` 删除）。同批的 A84 逐资产摘要复核亦已落。
- 剩余：A83 token 覆盖 uninstall/purge/update、A85 五维路径校验、A94 安装路径 RSS 门禁。

### Batch 5'（跨宿主与安全隔离，2 周，最大投入）

- A86+A106：把 `LocalHostBroker` 从 example 提升为**可分发的第二官方 Host**（真长进程、命令面路由、Windows named-pipe 腿进矩阵），并把 `local_host_reference` 的 OS 凭据实现下沉到 broker。
- A95 Windows handle-based enforcement（或明确把 `Partial` 写进对外能力矩阵，二者必居其一，不能沉默）。
- A96 把 `authorize_redirect/authorize_resolution` 接进真实 sink 路径；A97 明确「Hard 隔离」是实现还是能力边界声明。
- A100 生产 provider 装配。
- A107：C#/Swift/Kotlin 至少各有一条**真编译或真执行**的 fixture（否则从对外文档里删掉「多语言 FFI 已就绪」的暗示）。

### Batch 6'（发布证明，3–5 天）

- 双构建逐字节比对 + SBOM + attested provenance + 签名 tag。
- fuzz：给 wire framing / portable_path / network_policy 各建 `cargo-fuzz` 目标并入 nightly（不是 release blocking，避免抖动）。
- soak/leak trend（A62）、N-1 兼容报告（2.0 判据）、覆盖率与 criterion 基线比对。

### 里程碑与出口

```text
Batch 0 完成 → §138.2 observable gaps 剩 2 条（仍成立的只剩「Tauri 是唯一正式 Host 边界」与仓库设置类）
Batch 0+1'+2' 完成 → 1.1 出口三判据可判定（W3/W5/W2 体积门需一并补）
Batch 3'+4' 完成 → DoD 33 项中 ⚠️ 收敛到 ≤5
Batch 5'+6' 完成 + 分支保护生效 → 才允许对外使用 Industrial-grade 措辞
```

---

## 9. 明确推迟 / 不做（附理由）

| 项 | 处置 | 理由 |
|---|---|---|
| A23/A24 WASM 引擎 + WIT | **推迟到独立一轮** | `tauron-wasm` 需引 wasmtime/wasmi 级重依赖 + 新命令面 + 新门禁；塞进收尾只会做成 `simulated`，即 §33 点名的「接口在、事实不在」。1.2 出口要求三形态过同一 conformance，半吊子反而误导 |
| 插件级 update/rollback、Runtime Pack Manager、Manifest V3 全闭环 | **推迟（1.3）** | 需要包来源、版本协商与回滚事务三件套；当前 `host_market_*` 是 simulated，先把 A83/A84 的安装事务做严 |
| P2 Marketplace registry / catalogue / portal | **不做** | §40 明写「全部 optional」，不进 1.x 判据 |
| 为适配 HarnessDock 抢 DSH Runtime 所有权 | **不做** | §41 原则：真实 Consumer 用来验证通用性，不把产品逻辑塞回 Tauron |
| 新增 `E_REVIEW_STALE` / `E_RUNTIME_HANDSHAKE` 等线名码 | **暂不做** | 现行语义已由「现行码 + 结构化字段」承载且有回归门禁；新增码会同时动 Rust 枚举、`Display`、`HOST_ERROR_CODES` 三处，并使 N-1 客户端分流表整体错位。A69 完成 codegen 后再评估 |
| 删除 `Registry::bind_identity` / `PluginIdentity` 重复实现 | **1.x 内保留** | 删除是破坏性变更；已登记为已知重复，按新提案统一 |
| 把 `memory-transport.ts` 改名/升级为 MemoryHost | **不做** | 它是测试 transport，冒充 A27 的参考宿主比诚实登记更糟 |

---

## 10. 对外口径修订（必须与代码同批改）

1. 继续使用 §138.2 的表述：**「较强的 Tauri-first substrate 基础 + 完整的 Universal/Industrial 演进设计」**，
   不出现 industrial-grade。
2. §137 支持等级今天能如实写的只有：
   ```text
   Tauri Desktop        → Official / Full
   Memory transport     → 测试内参考，非 Official Host（不写 MemoryHost 参考实现）
   Local Host Service   → 实验性（example + chaos，无生产接线）
   Electron/Qt/.NET     → 未支持（不得写「可通过 Local Host Service 达到 Conformant」）
   Browser              → 未评估
   ```
3. `README.md` / `docs/installation.md` 需要一句：**「V4 的 A64–A110 共 47 项：已落 22 / 部分
   21 / 未落 4」**的可核查描述，并指向本文件的表；不允许只写「已完成 P0」。
   ——**轮 11 已落**：README「文档」小节末尾新增口径句并同步 §1.1/§5 的数；
   `docs/installation.md` 的包真机验证状态仍按 A 档 ⚠️ 表述（出包 ≠ 装过）。
4. 本文件所有状态随批次推进更新，**更新时同轮采集门禁输出**（沿用轮 9 的「门禁实照」写法），
   禁止凭记忆改数字。
5. **轮 11 新增**：发布链的**版本号生产者**必须与 `release.yml` 的 version-check 同源
   （`scripts/version-sync.mjs` + `pnpm version:check` / `version:sync`，CI 第一步判定）。
   README 的发布流程改成一条命令写齐落点，不再靠人肉记六个文件。

---

## 11. 复核命令（任何人可自行验证本文判定）

```bash
# F1（轮 10 后）：开发态仍「非 plugin- = 主窗」，Production 必须声明 label
rg -n 'production_caller_allowed|MAIN_WINDOW_LABEL_NOT_DECLARED' crates/tauron-host/src/authz.rs crates/tauron-adapter/src/tauri.rs
# F2（轮 10 后）：空清单在 Production = 拒绝，且 readiness 有独立事实位
rg -n 'origin_gate_armed|ORIGIN_GATE_ARMED' crates/tauron-host/src/production.rs crates/tauron-adapter/src/lib.rs
# F4（轮 10 后）：CreditWindow 有生产消费者；FairQueue 应**零命中**（已删除）
rg -n '\bCreditWindow\b' crates/tauron-host/src          # 期望：admission.rs + stream.rs
rg -n '\bFairQueue\b' crates                              # 期望：无命中
rg -n 'OrderingTracker|\.observe\(' crates packages | rg -v 'tests/|ordering.rs|eventbus.rs'
# A69（轮 10 已闭合）：三处都应是**按名集合**比对，不该再出现逐序 toEqual
rg -n 'toEqual\(rustCodes\)|HOST_ERROR_CODES\]\)\.toEqual\(\[' packages
sed -n '58,74p' packages/tauron-host/src/gates.test.ts
sed -n '34,78p' packages/tauron-app-contract-kit/src/contracts.test.ts
# 本轮新增的两条反向门禁
pnpm --filter @tauron/contract-tests exec vitest run src/wire-gate.test.ts \
  -t "Production 的 origin 门必须装弹"
pnpm --filter @tauron/contract-tests exec vitest run src/wire-gate.test.ts \
  -t "stream credit 必须由 admission::CreditWindow 单一裁决"
# DoD 2：core 无桌面依赖
rg -n 'tauri|webview' crates/tauron-host/Cargo.toml
# A107：只文本比对，从未编译
sed -n '450,476p' crates/tauron-ffi/src/lib.rs
# A75：序算而不用
rg -n 'service_startup_order|service_shutdown_order' crates/tauron-adapter/src/lib.rs
# 门禁形态与分支保护现状
rg -n 'name:' .github/workflows/ci.yml | head -40
ls .github/            # 无 CODEOWNERS / 无 ruleset
```

轮 11 新增项的复核命令（同一口径：命令能自己复算本文的判定）：

```bash
# F3：审计是事实源，不是开关位——布尔开关必须已消失
rg -n 'fn with_admin_audit\(' crates       # 期望：零命中（只剩 with_admin_audit_dir）
rg -n 'audit_for_admin_operations_available' crates/tauron-adapter/src/lib.rs  # 由 healthy() 推导
# A101：镜像先于正式文档落盘；死结构不得复活
rg -n 'HOST_SETTINGS_ROLLBACK_FILE|stage_settings_rollback_image|clear_settings_rollback_image' crates/tauron-adapter/src/lib.rs
rg -n 'pub struct MigrationSnapshot' crates packages   # 期望：零命中（durable.rs 只有删除史的注释）
# A84：逐资产服务真的复核摘要
rg -n 'verify_asset|verify_bytes' crates/tauron-adapter/src/tauri.rs crates/tauron-adapter/src/lib.rs
# A102：接收端判定有两个生产消费点，异常分支不吞投递
rg -n 'EventOrderingWatcher|ordering\.observe\(' packages/tauron-host/src packages/tauron-app-plugin-sdk/src
# A81：撤销的效力（退订 + 作废待取帧）
rg -n 'fn revoke|drop_queued|revoke_racing_publish' crates/tauron-host/src/eventbus.rs
# A91×A104：闸门不再跨 I/O 持锁；写租约是唯一串行化点
rg -n 'settings_fault\.lock\(\)\.run\(' crates   # 期望：零命中
rg -n 'let _write = state\.settings_write_lock\.lock\(\);' crates/tauron-adapter/src/lib.rs  # 期望：4 处
# 发布链：版本号生产者与 release.yml 校验器同源
pnpm version:check
# 文档行号引用核对：本文与 app-layer-wire / 插件开发指南里每一条带行号的引用
# （路径 + 行号、符号 + 行号两种写法）都必须仍指得到那个文件/那个符号
pnpm docs:check

# 轮 12（0-8）：主窗专属面的判定必须在**代码**里，不在 capability 文件里
rg -n 'require_main_window\(caller, "host_(window_quit|clipboard_read|clipboard_write|dialog_open|dialog_save|dialog_message|dialog_confirm|recover_boot|i18n_stats)"\)' crates/tauron-adapter/src/lib.rs   # 期望 9 命中
rg -n '主窗专属（capability' docs/api crates examples     # 期望：无命中（旧口径只在更正叙述里作为历史被引用）
rg -n '"windows"|plugin-\*|core:default' examples/minimal-app/src-tauri/capabilities/default.json   # 事实源：windows 含 plugin-*
cargo test -p tauron-adapter --lib main_window_surface_guard --locked   # 4 passed
# 轮 12（0-9）：接口参考是生成物——手改 docs/api/command-surface.md 必然红
pnpm command-surface:check
git diff --stat docs/api/command-surface.md              # 期望：无未提交改动
pnpm --filter @tauron/contract-tests exec vitest run src/wire-gate.test.ts -t "命令面接口参考必须由代码生成并覆盖全部命令"
```

> **引用口径（轮 11 定）**：新增或修订证据引用时**优先符号锚定**（写 `` `fn revoke` ``、
> 测试名、常量名，而不是行号）。行号是可漂的，一次 `+1330` 行的改动就把本文 5 条
> `symbol:NNN` 引用指到了无关行（其中 `drop_queued`、`review_matches_verified`、
> `getpeereid`、`cmd_http_request`、`authorize_http_url`），A97/F3 两行连
> `adapter/lib.rs` 这种缩写路径都不在真实路径里。`pnpm docs:check` 现在把这些引用当
> **可执行断言**跑：路径要按路径段子序列解析得到、行号要在文件行数内、被指的那几行
> 必须真的含该符号。CI 有同名步骤，`wire-gate` 钉住「脚本 + `docs:check` + CI 步骤 +
> 本口径」四者同源。

**轮 11 补：文档宣称与代码相反，也是断链（第 4 类，比行号漂更严重）**

轮 11 逐行核对时发现三处「文档把已经接线的东西写成未接线」——方向与常见的夸大相反，
但同样杀人：接入方按文档就不会去用一条已经能用的链路。

| 宣称位置 | 文档原话 | 代码实际 |
|---|---|---|
| `docs/api/plugin-development-guide.md` 的 Process 边界、`docs/architecture/app-layer-wire.md` 的 runtime 限制段、`docs/integration/incremental-adoption.md` 的缺口表 | 「JSON-RPC 帧回路**未接线**（stdout 走 `Stdio::null()`）」「无进程组/作业对象，`kill` 只覆盖直接子进程」「`tauron-acl` 未进依赖表」 | ① `CommandSpawner` 走 `Stdio::piped()` + 每进程一个读线程，回帧经 `ProcessFrameSinkImpl` → `settle_call`（spawn 成功后在 `cmd_runtime_spawn` 里注册接收器）；② `UnixProcessGroupSandboxProvider` / `WindowsJobObjectSandboxProvider` + `impl Drop for CommandSpawner` 的 tree-aware 回收；③ `tauron-acl` 是 `plugin-install` 的可选依赖并在安装路径被消费 |
| 同前两处 | 「进程崩溃的**沙箱等级**」口径缺失 | 两 provider 自报 `Partial`；Production 下接上 `ProcSpawner` 即被 `validate_process_runtime_for_start` 拒启，`cmd_runtime_spawn` 另有非 `Hard` 拒 spawn 门（A97） |
| `plugin-development-guide.md` 的拒绝原因表 | 「只会以**三种**原因拒绝」，表里却列了 4 行 | `production_caller_allowed` 实际返回 4 个原因串 |

教训与门禁：**「未接线/未实现」这类否定式宣称和肯定式宣称一样要被核实**——行号引用由
`pnpm docs:check` 复算，而"这件事到底做没做"由本轮起改成一处一查（本次三处已按
`CommandSpawner`、`production_caller_allowed`、`AUDITED_ADMIN_COMMANDS` 的当前实现改写，
诚实边界（无真 sidecar 运行期证据、`Partial` 非 `Hard`、`to_capability` 无生产消费者）
同时写进文档与 A97 行的未验证清单）。
