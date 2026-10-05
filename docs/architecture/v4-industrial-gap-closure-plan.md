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
| F1 | **任意非 `plugin-` label 即主窗特权** | `crates/tauron-host/src/authz.rs:524-531`（`resolve_principal` 的 `None => Principal::MainWindow`） | 次级窗口/伪造 label 直接拿到 `require_main_window` 保护的 admin 面（该符号在 `adapter/lib.rs` 命中 51 处，含定义与测试）；40+ 主窗命令不在 `ADMIN_COMMANDS` 8 条表内，靠注释约束 | 小（改判定 + 配置默认主窗 label + 反向用例） | **Production 已闭合**：`production_caller_allowed` 要求 label ∈ `main_window_labels`；`resolve_principal` 的开发态语义按设计保留 |
| F2 | **origin 允许清单为空 = 门禁关闭** | `crates/tauron-adapter/src/tauri.rs:2990`（`if allow.is_empty() { return Ok(()) }`） | Production 形态可在**零 origin 白名单**下通过自检：§138.2 点名的这一条今天仍然字面成立 | 小（Production 下空清单视为 not-ready；开发态保留兼容） | **已闭合**：新增独立 readiness 事实 `origin_gate_armed` + doctor `origin-gate` + `ORIGIN_GATE_ARMED_REQUIRED` 启动门 + 门内 `ORIGIN_GATE_NOT_ARMED` |
| F3 | **admin 审计只有开关位** | 旧实现是 `AdapterConfig.admin_audit_available: bool`（宿主自报）→ 只喂 `production.rs` 的 readiness 位；全仓无 audit sink 写入 | doctor 能说「审计可用」，但没有任何一条审计事实被产出——即 §140 的「有文档无真实能力」 | 中（需要事实源：结构化事件 + durable 落盘 + 读口） | **已闭合（轮 11）**：事实源 `crates/tauron-host/src/admin_audit.rs`（链式哈希记录 + `AdminAuditSink` 落盘 `admin-audit.json` + `verify_file` 离线复核 + 512 条环形上限）；写口是 admin 分发的单点 `record_admin_audit`（`adapter/lib.rs`，Allowed/Denied 都留痕）；开关位删除，doctor 的 `admin-audit` 检查项（readiness 字段 `audit_for_admin_operations_available`）改由 `AdminAuditFacts::healthy()`（落盘 ∧ 链完整 ∧ 零写失败）推导，读数快照随 `ProductionDoctorReport.admin_audit` 上线 |
| F4 | **`CreditWindow` / `FairQueue` 是死类型** | `crates/tauron-host/src/admission.rs`（仅本文件 + `lib.rs` re-export 命中） | A79/A80 的「按 owner 公平调度」停在类型层；饿死风险真实存在，只有单测 `usage_tracks_and_releases_reservations:230` | 中（接线或按孤儿台账删除并登记） | **轮 11 全部收口**：`CreditWindow` 已接线（轮 10，`StreamRegistry` 唯一额度算术）；`FairQueue` 已删除并登记；`ActivationRecord::verify_bytes` **已接线**（`PluginAssetTrust::verify_asset` `adapter/lib.rs:810→:843` ← 逐资产服务 `tauri.rs:1777`）；`MigrationSnapshot` **已删除**（与 `MigrationReceipt.before` 同一事实的两份表达，A101 改用 `DurableEnvelope` 的宿主设置协议）；`OrderingTracker::observe` 定性为**发布侧 oracle**——生产 publish 走 `issue`，`observe` 由 `v4_host_conformance.rs:94` 钉语义，接收端判定在 TS `EventOrderingWatcher`（A102）。后三条登记在 V4「未接线公开 API 台账」轮 11 小节 |
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
| origin empty allowlist = gate disabled | **仍成立** | `tauri.rs:2990`（F2） |
| recovery durability 可关闭 | **已闭合**（改为 fail-closed） | `reconcile_settings_boundary:7213` 无持久化即 `Quarantined`（行为测试 `settings_fault_without_durable_state_is_quarantined_not_faked_ready:14728`）；`recovery.rs:305/345` 走 `DurableEnvelope::seal` |
| RecoveryAction 幂等生产链未接 | **已闭合**（A92） | 生产命令 `cmd_recover_trial_enable:8484`，幂等键取持久化 incident 序列（`should_execute:8516`），`should_execute=false` 走零副作用分支（`should_execute:8516`）；同 incident 去重测试 `recovery_trial_enable_action_is_deduplicated_in_same_incident:12573` |
| whole-package memory read | **已闭合**（轮 44） | 验签流式 64 KiB（`package_signature.rs:163-230`）、解包逐 entry `std::io::copy`、**激活摘要也改流式**（`ContentIdentity::from_reader`，安装腿此前把每个文件整读进内存，本轮门禁抓出的真实缺口）；`verify_tpkg`/`verify_tpkg_file` 整包入口已删除；内存门禁迁到**真实安装流**（`install_stream_probe` 走 preview→reviewed 提交链；`performance-budgets.json` 的 `installStream`：载荷下限 64 MiB、堆峰值 16 MiB（计数分配器，跨平台）+ VmHWM 32 MiB）。网络下载消费者仍缺，但那是市场下载腿（simulated，A44 线）的事，不影响本条的「无整包读」结论 |
| ErrorCode 顺序耦合 TS gate | **半闭合** | canonical 注册表已按**名集合**比对（`wire-gate.test.ts:252-263` + `contracts/error/error-codes.json`），但 `packages/tauron-host/src/gates.test.ts:63` 与 `packages/tauron-app-contract-kit/src/contracts.test.ts:44` 仍逐序 `toEqual`，`crates/tauron-host/src/error.rs:74,166` 注释仍在教「追加末尾」 |
| E_HOST_PANIC = retryable | **已闭合**（A70） | `error.rs:127 => RetryClass::Never`，`retryable()` 由 class 派生（`:141-143`），wire-gate 锁定 panic=never |
| manifest target 只有 OS 粒度 | **部分** | `TargetSpec` 已是 os×arch×abi×min-os×cpu（`target.rs:41`），`contracts/target-matrix.json` 有 `profiles` 维度；但 `resolve_best`（`target.rs:85`）零生产消费者、`ArtifactVariantResolver` 只存在于注释（`ArtifactVariantResolver:1376`），安装路径不过滤变体 |
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
| A65 Profile V2：Tier 与 Capability Bundle 解耦 | **已落**（轮 45） | `contracts/tier-bundles.json` 三张表 + `scripts/check-tier-bundle.mjs` 六条不变量（85 命令单属划分 / S⊂E⊂P 严格单调 / requires∩surfaceBundles=∅ 的正交性机器证明）；`gate:tier-bundle` 进 `gates:check` 链与 ci.yml | — |
| A66 zero-surface/headless ReadinessSet | **已落**（轮 46） | `tauron-host/src/readiness.rs`：`ReadinessFact` + `ReadinessSet` 六组必备事实、`headless()` 空 Surface 不阻塞、`evaluate()` 未就绪事实具名进 `blockers`、`health()` 桥 A103 `HealthReport`；真跑 conformance 三条（`v4_host_conformance.rs`：零 Surface 就绪 / 声明未挂载 Surface 具名拦下 / kernel 未就绪不豁免）；`wire-gate` 轮 46 段钉形状与测试名 | — |
| A69 generated Error Registry | **部分** | 见 §2 半闭合行 | 取消三处顺序耦合；补 codegen（Rust 24 个 `E_*` / TS 数组 / JSON 三份手工同步）；框架侧 14 个 `SC-####`（`tauron-shell/src/error.rs`）与宿主码的关系成文 |
| A70 RetryClass | **部分** | 判定已改（`error.rs:110-134`）、两侧断言齐（`wire-gate.test.ts:273-299`） | §90 的 RetryPolicy/OperationClass/RetryBudget **无类型无实现**；`RetryClass::AutoIdempotent` 当前无码映射 → `retryable()` 恒 false（诚实但字段零信息）；TS 侧无自动重放路径 |
| A91 FaultBoundary / subsystem quarantine | **部分** | `fault.rs:71` panic 后 `NotReady` 确定性拒新工作；生产面四条边界——settings（`run_settings_boundary:7192`；四个写租约消费点 `reconcile_settings_boundary:7213` / `cmd_settings_set:6876` / `cmd_settings_adopt_legacy:7561` / `cmd_settings_migrate:7560`），重建自 durable（`load_settings_doc:6752`）、无持久化即 Quarantined（`settings_fault_without_durable_state_is_quarantined_not_faked_ready:14728`）；**轮 47 扩到** events/approval/registry 三条（`run_events_boundary:7309` / `run_review_boundary:7325` / `run_registry_boundary:7340`；三段式共用 `run_subsystem_boundary:7288` + 修复前置判定 `begin_subsystem_reconcile:7359`）：7 条事件命令走闸门，修复=**会话态清零且保留 topic 声明**（`EventBus::reset_session_state:518`；测试 `session_reset_clears_subscriptions_but_keeps_topic_declarations:1223` / `events_fault_boundary_rejects_bus_work_and_recover_boot_resets_session_state:14743`）；4 个令牌消费点走闸门，修复=重新 preview 清空旧令牌（`reconcile_review_boundary:7402`；测试 `approval_fault_boundary_blocks_tokens_and_preview_reconcile_clears_them:14795`）；5 条注册表命令走闸门，无会话内重建=**如实 Quarantined 待重启**（`reconcile_registry_boundary:7419`；测试 `registry_fault_boundary_quarantines_until_reassembly_not_faked_ready:14860`）；四条边界的故障读数并入 `host_resource_stats` 的 faults（settings/events/approval/registry）；**轮 11 拆分**：闸门只保留「就绪判定 + 事后登记」（`ensure_ready:63` + `record_panic:85`），边界锁**不再跨磁盘 I/O**，单写者边界交回 `settings_write_lock`（A87），并有行为证明 `concurrent_settings_writes_never_lose_a_key_or_share_a_generation` | 其余子系统（registry 的调用/流/运行时读取面、http/fs/process、总线内部调用点）未包边界——一个 panic 仍是全宿主风险；`FaultBoundary::run` 拆分后只剩 fault.rs 单测消费者，按「未接线公开 API 台账」登记为 embedder 面（轮 11 小节） |
| A92 RecoveryExecutor 生产接线 | **已落** | `cmd_recover_trial_enable:8484` + dedup 分支（`should_execute:8516`）；测试 `recovery_trial_enable_action_is_deduplicated_in_same_incident:12573`、`cmd_recover_report_drives_phase_and_reconciles_registry:12475` | — |
| A93 DurableEnvelope/checksum/generation/quarantine | **已落** | `durable.rs:37-76`；读写口 `load_settings_doc:6752` / `persist_settings_doc:6877`（密封 ↔ `decode_durable`）、`recovery.rs:305/345/394`；损坏隔离到 `.corrupt`；测试 `settings_durable_envelope_detects_tamper_and_quarantines_file:15240` | — |
| A109 production security self-test / doctor | **已落** | Rust + 命令面 + TS + 示例三层通（§0 末段）；档位 `authz.rs:340` Privileged；测试 `production_doctor_is_main_window_only_and_mirrors_readiness:16088`；**轮 11**：`admin-audit` 检查项由真实 sink 的 `AdminAuditFacts::healthy()` 推导，并把读数快照（条数/裁剪/写失败/链完整）一并上线（`production_doctor_report:6665`，测试 `doctor_derives_the_admin_audit_check_from_the_live_sink:10895`） | 空清单 fail-open（F2）与开关位（F3）均已在轮 10/11 闭合；剩余边界=分支保护未开（0-7，需用户操作） |

### Batch 2 — 固化 Universal Contract：A67 A68 A71 A72 A73 A74 A75

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A67 Wire framing + mandatory json-v1 | **部分** | `wire.rs:110` 解析前查上限；`remote_host_reference.rs:140-149` 先验后分配；消费点全是**校验/回环**（`tauron-ffi/src/lib.rs:267`、`remote_host.rs:535`），唯一副作用式消费是 8 MiB pending args 预算（`registry.rs:40`） | 真实命令面走 Tauri 裸 `invoke`（`tauri-backend.ts:270`），**不穿 envelope、不协商 codec**（`adoptCapabilities:249` 只协商命令集）；无固定二进制头/frame kind/flags/sequence |
| A68 schema extension / unknown-field policy | **部分** | Wire 侧 extensions 收口（`wire.rs:58/62/66`，测试 `:157-163`）；Manifest `manifest.rs:673` | `WireExtensions` 全仓仅 wire.rs 自用；Manifest 无 `manifestVersion`/extensions 通道；Capability「required 未知→incompatible」零实现 |
| A71 TargetSpec + ArtifactVariantResolver | **已落** | `target.rs:41` 模型完整、`current_target_spec:197` 被 `adapter/lib.rs:1315` 消费；**轮 49**：`resolve_best:88` 泛型化并新增 `ArtifactVariantResolver`（`target.rs:109`），经 `UpdateArtifactVariant` + `with_variants` 接进 `DistributeUpgradeInstaller` 的 download/install（变体过滤，无兼容硬拒；host 测试 `artifact_variant_resolver_returns_none_when_no_variant_is_runnable:162`，adapter 测试 `variant_list_without_compatible_target_is_rejected_before_any_download:20218`）；孤儿台账条目已删除 | 无（五维模型不变；变体来源仍由装配方提供） |
| A72 C ABI / FFI ownership contract | **已落** | `tauron-ffi` + `conformance/c/ffi_ownership_asan.c`，CI 真跑 ASan + `detect_leaks=1:halt_on_error=1`（`ci.yml:238-243`） | 只有 ASan；无 TSan/Miri |
| A73 ExecutionDomain / thread-affinity | **已落** | `execution.rs:89` 门禁测试 | 仅单测级证据，无真跨线程调度断言 |
| A74 ProviderLifecycle + CapabilityEpoch | **部分** | `provider.rs` 自测齐 | **适配器零消费**：Provider 热替换的 generation 保护未接主链（对照 §141「provider hot swap generation 不混线」） |
| A75 ServiceGraph start/shutdown planner | **部分（轮 11 删装饰、轮 34 复核本行）** | `service_graph.rs:32/83` 真 DAG + 环检测；装配期只认**拓扑有效性**——坏图当场 panic（`canonical_substrate_service_graph` + `startup_order()` 校验，见 `adapter/lib.rs` 的 `if let Err(error) = canonical_substrate_service_graph().startup_order()`）；该调用点上方的「V4 ServiceGraph（轮 11 收口 A75）」注释记录了删除理由 | ~~两个序只存字段~~ **该载体已随轮 11 删除**（当时唯一消费者是测试，属"看起来在按拓扑序装配"的装饰；`wire-gate` 反向钉住它不许回归）。真正剩下的缺口：本仓没有可排序的**服务级回收动作**——底座状态随进程释放，唯一有外部副作用的退出动作（sidecar 子进程）由 `tauron_proc::CommandSpawner::drop` 承担；退出路径 `cleanup_closed_window`（`tauri.rs:288`）按窗口 label 手工清，无 app-exit 逆序回收。A74/A88 给 provider 补上真实回收动作后此项才谈得上落地（见 Batch 4'） |

### Batch 3 — 封并发与资源竞态：A76 A77 A78 A79 A80 A81 A82 A90 A104

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A76 atomic CallState terminal | **已落** | `call_state.rs:49 try_finish` 被 `registry.rs:233` 真实持有；过期判定+终态仲裁+摘除收进同一 pending 锁（`:853-917`），settle/cancel/timeout/teardown 四方全走它；256 轮 4 线程唯一赢家测试（`call_state.rs:71`） | 无 loom |
| A77 CallGraph hop/cycle/re-entrancy | **已落** | `registry.rs:782 begin` → `E_CALL_CYCLE`（Never 重试）；命令面透传 `parent_call_id`（`tauri.rs:684-690` ← `host.ts:322`） | legacy 无 parent 的调用直接成根、不核深度 |
| A78 event causation depth/budget | **已落**（有洞） | `eventbus.rs:854` publish 路径校验；命令面回传上下文（`tauri.rs:500-511`）；测试 `:1618` | 无「A→B→A 调用环 + 事件回流」复合场景 |
| A79 credit-based stream backpressure | **部分**（轮 10 收紧） | stream 一路是真 credit，且**轮 10 起额度只有一个原语**：`StreamHandle.credit: CreditWindow`（`stream.rs` 的 `grant`/`consume`）+ `host_stream_grant` 命令面 | RPC result / state update / telemetry 三类仍无端到端 credit。~~`CreditWindow` 死类型~~ 已接线（`admission.rs` 模块头 + `wire-gate` 双向钉住） |
| A80 hierarchical AdmissionController / fair | **部分** | 双层限额真实存在（global+per_principal，`registry.rs` 的 `AdmissionController` 字段与 `call_begin` admit / 全终态 release） | **按 owner 公平调度仍未落**：`FairQueue` 原为零消费者死类型，轮 10 已**删除并在 V4 台账登记**（半接的轮转队列比不接更危险）。缺额转入后续 Batch「A80 公平调度接线 + 端到端饿死压测」 |
| A81 PolicyEpoch/GrantVersion/DecisionToken | **部分** | `DecisionToken:11`（含 `PolicyAuthority::forget:83` 的订阅者销毁回收，轮 16 R2）；`eventbus.rs:553-569` bump_grant、提交前 `validate_scoped`（锁内，锁序注释）；**轮 11 把撤销从「只影响新调用」改成有实效**：`revoke:612` 对「他人声明且非公共」档同时①退订该 `(subscriber, topic)` 的全部既有订阅②经 `drop_queued`（同文件）作废三类通道的待取帧③作废走幂等重放（授权行已不存在也再作一次），公共/自属档刻意不动；命令面 `cmd_events_revoke_as:6628` → TS `host.ts` `eventsRevoke`；并发对抗测试 `revoke_racing_publish_leaves_no_revoked_content_behind:1479` + SDK/host 两侧断言 | fs/http/process/secret 完全不重检 token（adapter 内 `DecisionToken` 零命中）；与撤销**并发**且已在撤销前解析到 token 的 publish 仍可能落一帧——靠管理面重放 revoke 收口，要封窗口本身得让发布在 `queues` 临界区内重检审批表（`eventbus.rs` revoke doc 已写明边界） |
| A82 SettingsRevision + post-commit watch | **已落**（范围内） | `tauron-settings/src/store.rs:314-315/471-482`；`commit_settings_change`（`adapter/lib.rs`，`:6070` 把 revision 写进镜像帧）、`cmd_settings_set` stage→persist→commit/restore；SDK `createPlugin.ts:166`；示例 `plugin/first.ts:50`；两侧结构门禁 `settings_commit_has_single_mirror_site`（`adapter/lib.rs`） + `wire-gate:2816-2826/3427-3441/3461` | `getAtLeastRevision()` 未落（`host_settings_revision` 的文档注释自陈无 wire 口）；`cmd_settings_adopt_legacy`/`cmd_settings_migrate` 不产帧也不推 revision → bulk 提交对观察者不可见 |
| A90 stale-handle generation protection | **部分** | runtime handle 一条真走代际：activate（`runtime.rs` 的 `RuntimeTable::register` 调用点，符号锚定）→ 捕获 `runtime_generation`（`registry.rs:812`）→ 回帧拒旧代（`process_delivery.rs:44/175`）+ 版本化租约查找（`runtime_lease_versioned:1293` ← `tauri.rs:844`，测试 `runtime_ensure_lease_is_idempotent_and_starts_at_most_once`）；**轮 25 补：代际号源改全局单调计数器**，摘除跟踪后重装同名插件仍拿到更大的号（`abandoned_generation_handle_stays_rejected_after_forget_and_reactivation`） | 插件激活（`activation.rs:30`）只带字段不校验；Provider 侧未接（A74 同源） |
| A104 no-lock-across-await gate | **已落**（覆盖面窄） | `ci.yml:88` + `check-no-lock-across-await.mjs:12` | 只扫 `tauron-host`/`tauron-adapter` 两个 crate 的 src，且是文本级而非 AST |

### Batch 4 — 封安装/升级/存储一致性：A83 A84 A85 A87 A88 A89 A94 A101

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A83 InstallReviewToken | **已落**（install 域 + 管理域 uninstall/purge；轮 43） | install 域：`InstallReviewToken:982` 类型、`mint_install_review:1058` 铸、consumption 处 nonce 一次性 `remove` 再比对 + `review_matches_verified:1081` 绑 package/manifest/permission 三摘要；TOCTOU 靠 `VerifiedPluginPackage`（`:990`，`:994 archive` 句柄）；测试 `reviewed_install_rejects_package_replaced_after_preview:11335` / `reviewed_install_accepts_the_exact_previewed_package_once:11381`。管理域（轮 43）：`AdminReviewToken:1113`（camelCase + `deny_unknown_fields`，绑 plugin_id+op+预览版本）、`ADMIN_REVIEW_TTL_SECS:1124`/`MAX_ADMIN_REVIEWS:1125` 有界、`mint_admin_review:1231`、`validate_admin_review:1267`（凡呈现即严格校验）、`admin_review_required:1221`（feature+Production 才强制）；旧面无令牌入口 `cmd_registry_admin_as:5375` 在该档拒破坏性操作、新面 `cmd_registry_admin_reviewed_as:5373` 预览只铸令牌（零副作用）→ 提交令牌重核事实；wire `RegistryAdminResponse:1135`（`kind` 判别；executed 分支老字段仍在顶层，老读者不破）+ `wire_registry_admin:606`（`HostAdminOp` `preview`/`reviewToken` 双 `#[serde(default)]`，旧载荷仍合法）；TS 判别联合 `RegistryAdminOutcome` + `_destructiveAdminOp:291`（预览→确认→带令牌提交；无 DOM 默认钩子失败关闭）+ shell `_uninstallPlugin` 同链；测试 `admin_review_preview_binds_facts_and_commits_once:13034` / `admin_review_rejects_op_mismatch_and_stays_consumed:13085` / `admin_review_version_drift_forces_a_fresh_preview:13119` / `admin_review_protocol_rejects_non_destructive_mixing:13148` / `production_destructive_admin_op_requires_the_review_path:13276`（+ wire-gate 轮 43 段） | update 域＝install+uninstall 两条腿均已覆盖、无独立通道（`E_PLUGIN_EXISTS` 拒覆盖，正路即两腿组合）；install 域语义仍借 `E_INSTALL_FAILED`（仅过期判定改判 `E_AUTH_DENIED`）；审批令牌 nonce 未进 admin 审计事实（过渡留痕按既有 admin_gate 形状） |
| A84 immutable installed content + activation integrity | **已落**（范围内） | HMAC 保护的全目录摘要清单（`activation.rs` 写口 `collect_plugin_activation_records:549` ← `cmd_registry_install_as:4861`，symlink 直接拒 `symlink_metadata:573`）；激活校验 HMAC+generation+当前文件集重算（`load_sealed_activation:696` → `symlink_metadata:707` ← `with_plugin_asset_protocol:1755`）；篡改/注入测试 `installed_plugin_ui_rejects_content_and_activation_metadata_tampering:11595` / `installed_plugin_ui_rejects_injected_files_after_activation:11661`；**轮 11**：逐资产服务 `read_installed_plugin_asset`（`tauri.rs:1738`）在 `canonicalize`+`starts_with` 之后、把字节交给响应之前过 `PluginAssetTrust::verify_asset`（`verify_asset:820`→`record.verify_bytes`）——无密封记录（安装后注入的文件）与摘要不符同样拒服务，并区分 404（没有这条内容）/403（有这条路径、内容与封的不一致）；`ActivationRecord::verify_bytes`/`ActivationError::IntegrityMismatch` 由此拿到生产消费者 | 摘要复核只覆盖 `plugin-install` 特性的服务路径；probation/commit、以及「摘要不符 ⇒ 事件通道实时撤服务」未建模（撤的是当次请求） |
| A85 PortablePathValidator | **已落（轮 42）** | 五维逐条（`crates/tauron-host/src/portable_path.rs`）：Windows 保留名表 `WINDOWS_RESERVED_STEMS`（主名命中即拒，`NUL.txt` 也算）、NFC 归一（`ComposingNormalizerBorrowed::new_nfc()`，非 NFC → `NotNfc`）、尾点/空格（`TrailingDotOrSpace`）、便携长度上界（`MAX_SEGMENT_BYTES` 255 / 整条 1023 → `TooLong`）；集合级 `validate_portable_entry_set`（ASCII 折叠冲突 → `CaseCollision`）。zip 侧（`crates/tauron-market/src/lib.rs`，`signing` 生产面）委派**同一份**判定：`sanitize_entry_path` 逐条 + `validate_zip_constants` 集合级；测试 host 五条 + market 两条 | 折叠只做 ASCII（Unicode 全表折叠需额外数据表，边界成文）；长度上界是相对路径承诺，Windows MAX_PATH 的宿主侧治理仍属 A95 |
| A87 StorageNamespace + SingleWriterLease | **已落** | 真跨进程 `File::try_lock`（`storage.rs:132`）、OS 自动回收、陈旧 owner 覆盖（`:213-217`）、epoch 递增；装配期 panic（`adapter/lib.rs` 的 `let storage_writer_lease = …` 段：`PersistentWriterLease::acquire` 的 `unwrap_or_else` 里那句 `durable-state single-writer lease rejected startup`）；conformance 真起子进程证 Busy/acquire（`v4_host_conformance.rs:155`） | namespace 硬编码（同一段的 `tauron_host::StorageNamespace { tenant, application, principal }` 三个字面量），未与 `tauron-brand/src/lib.rs:77 data_dir` 归属打通 |
| A88 generational plugin/provider/runtime activation | **部分** | runtime 一条已接（见 A90）；**轮 25 封口 V7 §9 leak gate**：代际台账不再「只进不出」——`RuntimeTable::remove_plugin` 释放租约后连带 `GenerationRegistry::forget` 摘除跟踪（有活跃租约时 `forget` 拒绝，绝不摘掉还在被校验的资源），在管数上界＝在飞插件数而非「曾起过运行期的插件数」，读数 `GenerationStats` 随 `host_resource_stats` 跨 IPC 可查；churn 证明分三层（台账 / 租约表 / 注册表真实安装-起租约-卸载路径） | `ProviderLifecycle`/`CapabilityEpoch` 适配器零消费；插件激活不校验代际 |
| A89 PackLease / CacheLease | **已落**（库层） | `generation.rs` 的 `pack_gc_requires_all_four_guards_to_be_clear` / `live_cross_host_lease_blocks_gc_until_release_or_expiry` / `heartbeat_extends_only_a_live_lease_and_cannot_resurrect_expired_owner` / `gc_retirement_is_atomic_with_the_authority_metadata` 四条 GC 租约测试；跨宿主回收已建模 | 上游缺 Pack Manager（1.3 项），租约只服务引用与状态——登记为「已落但有天花板」 |
| A94 streaming package verify/extract | **已落**（轮 44） | 验签流式 64 KiB（`package_signature.rs:163-230`，`verify_tpkg_reader` / `verify_tpkg_reader_with_time` 单源）、解包逐 entry `std::io::copy`、验签/解包共用同一句柄（`registry_install_preview_inner:4907` → `registry_install_inner:4875`）；轮 44：**删除** `verify_tpkg(&[u8])` / `verify_tpkg_file` 整包入口（零调用者），激活摘要改 `ContentIdentity::from_reader` 流式（固定 64 KiB 缓冲）；`performance-budgets.json` 的 `process.maxPeakRssKb` 换成 `installStream`（载荷下限 67108864 B、堆峰值 16384 KiB、VmHWM 32768 KiB），由 `install_stream_probe`（96 MiB 载荷走真实 preview→reviewed 提交链）产出、`check-performance-size.mjs` 断言；`wire-gate` 轮 44 段反向钉住 | 网络下载消费者仍缺（市场下载腿 simulated，见 §9 推迟表与 A44 线）；重放/限速类对抗不在本条 |
| A101 migration reversible/snapshot contract | **已落**（宿主设置域） | 事务回滚 + **磁盘回滚镜像**：`cmd_settings_migrate:7560` 先 `snapshot_all()` 再迁；合同 `requires_snapshot` 时先把迁移前用户层落进 `host-settings.rollback.json`（`stage_settings_rollback_image:6920`，自带 `DurableEnvelope`，schema `tauron.host-settings-rollback/1`），**之后**才写正式文档；装配读不回正式文档时按已校验的镜像恢复并**一次性作废**（`load_settings_rollback_image:3990`），救不回来才走 `report_settings_load_failure`（生产档 panic 前缀逐字未改）；落盘失败 ⇒ rewind 内存 + 作废镜像（`persist_settings_doc:6877`）；健康启动即回执掉镜像（`clear_settings_rollback_image:3980`）。行为测试 `concurrent_settings_writes_never_lose_a_key_or_share_a_generation`、重启级 E2E `a101_rollback_image_restores_pre_migration_state_after_a_restart:15108`（轮 16 R3 改判后为**四轮**：坏文档→按镜像恢复并作废→再迁移→健康启动**回执掉镜像**→二次损坏时因无镜像而如实降级，绝不消费陈旧镜像）；死结构 `durable.rs MigrationSnapshot` 已删除并登记 | 镜像只在迁移合同要求时产出；recovery/registry 等其它持久命名空间没有同类回滚合同；probation/commit（跨进程两阶段）仍未建模 |

### Batch 5 — 扩跨宿主与安全隔离：A86 A95 A96 A97 A100 A106 A107 A108

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A86 LocalHostBroker + peer auth + single-instance lease | **部分**（非生产接线） | 一次性 challenge/绑定 subject（`local_host.rs:214-217`）、`reclaim_stale:143`；**真 OS 凭据在 reference**：Linux `SO_PEERCRED`（`local_host_reference.rs:132-160`）、macOS `getpeereid:207`、Windows 管道 DACL+模拟 SID `:519/:614/:633` | `LocalHostBroker::new` 只在 tests/chaos/conformance/example；适配器与命令面零消费；CI 只跑 `cargo run --example`；Windows 管道腿不在矩阵，且 Windows `serve_one`（`local_host_reference.rs:741`）的 `ConnectNamedPipe`/`ReadFile` 传空 OVERLAPPED = **同步等待、无读超时**（Unix 侧 `:256` 有 `REFERENCE_EXCHANGE_TIMEOUT` 20s 交换上界）——调用方必须在专用线程上跑并用 `recv_timeout` 收它的结局 |
| A95 ScopedFsProvider no-follow / handle-based | **部分** | Unix 真句柄式：`scoped_fs.rs:130-215` openat + `O_NOFOLLOW` 逐级、`statat SYMLINK_NOFOLLOW:281`，测试 `:364/:378/:415` | **Windows 无 handle-based**：`*_hard` 全返回 `HardEnforcementUnavailable`（`:320-348`），`platform_enforcement=Partial`（`:83`）；`StdFsSink` 退化为按 `display_path` 重检（`impl FsSink for StdFsSink:2233-2253`），插件资产读口（`:710-728`）同样 |
| A96 HttpProvider redirect/DNS/private-network | **已落**（轮 48） | URL 解析取代前缀匹配（`network_policy.rs:53-91`，label 边界 `:75-83`）、逐跳 `authorize_redirect:228` 且跨源不转凭据（`:228-244`）、私网/字面 IP 拒（`:194-226`、`is_public_ip:315`）、默认 `deny_all`；**轮 48 接生产**：`cmd_http_request:9306` 重写为宿主侧单跳循环——sink 契约=只发单跳 + 必须回报 `resolved_addrs`（`HttpResponseSpec:2469`）、非法地址失败关闭；逐跳链 `resolve_redirect_location:260`（RFC 3986，`Url::join`）→ `authorize_redirect`（hop 上界 + 跨源剥离 `authorization`/`cookie`/`proxy-authorization` `:9298`）→ 按 provider 回报地址 `authorize_resolution` 复检；`NetworkEnforcement::RedirectAndDns` 成能力闸（其它档拒发 `:9243-9250`）；注入 builder `AdapterConfig::with_http_sink:424`；五条行为测试（`http_redirect_flow_follows_hops_with_rfc_3986_resolution:19060` / `http_redirect_cross_origin_strips_credentials_and_denied_target_never_sent:19085` / `http_redirect_loop_is_bounded_by_policy_max_redirects:19115` / `http_provider_resolution_is_rechecked_against_private_network_policy:19131` / `http_provider_without_redirect_enforcement_is_rejected_before_any_request:19149`）+ TS 线形可选 `resolvedAddrs`（`shell-client.ts:490`） | 真实 provider 仍缺：缺省 `UnavailableHttpSink` 如实 Unsupported，`RedirectAndDns` 实现者=注入方责任；`max_redirects` 上界与凭据剥离目前只有 `ScriptedHttpSink` 测试证明（无第三方实现） |
| A97 ProcessSandboxProvider + enforcement descriptor | **已落**（诚实分级=不隔离） | `UnixProcessGroupSandboxProvider` / `WindowsJobObjectSandboxProvider`（`tauron-proc/src/spawner.rs`）均自标 `Partial` 并写明 fs/net/syscall 未隔离；生产 hard-sandbox 语义 = **拒启 + 拒 spawn 双门**（`AdapterConfig::validate_process_runtime_for_start` → `PROCESS_SANDBOX_HARD_REQUIRED`；`cmd_runtime_spawn` 在非 Hard descriptor 下 `E_STATE_INVALID_TRANSITION`；doctor `process-sandbox` 项由 `production_doctor_report` 从 descriptor 推导）；provider 行为各有独立测试 | 仓内无 Hard provider（唯一 `Hard` 是测试替身）→ 代价是「生产形态下进程插件不可用」，这是对外必须写清的能力边界。**未验证清单（轮 11 补登记；轮 22 部分收口；`CommandSpawner` 文档注释指向本行）**：~~① 真 sidecar「收帧 → 回帧 → `settle_call`」端到端~~ **已落**——`crates/tauron-test-sidecar/tests/sidecar_e2e.rs`（真二进制、真起进程）连同生产投递/结算面一起跑；~~③ `kill` 正路（真杀活进程）~~ **随之有运行期证据**（禁用与静默不回帧两条用例）。仍未封口：② `impl Drop for CommandSpawner` 的连带 tree-aware 回收没有注入式失败测试（只有 E2E 收尾的 `tracked()` 归零断言 + 源码形门禁），以及 Windows post-spawn attach race 未测 |
| A100 TimeTrustState + TrustedTimeProvider SPI | **部分**（轮 43 收紧） | `require_unexpired` 失败关闭（`time_trust.rs:53`）→ `package_signature.rs:358`（provider 不可信即拒）与 `package_signature.rs:376`（过期即拒）；**轮 43：install 与管理两域令牌 TTL 全部改走 `review_now:1173` / `review_unexpired:1199`**——provider 在则走 `trusted_time`（Trusted 放行、非 Trusted 失败关闭），None+Production 在铸发与消费两处都直接拒；provider 缺席仅开发/测试档回落墙钟（`unix_time_seconds:5176`，现只余 ACL 授权行的发放时间戳，不再参与 TTL 判定）；生产档启动即要求 provider 存在且 Trusted（`TRUSTED_TIME_REQUIRED`，`AdapterConfig::validate_for_start`）；门禁 `suspicious_clock_does_not_bypass_expiry:98`、`trusted_clock_enforces_expiry:108`、`production.rs:154`；测试 `install_review_ttl_follows_the_trusted_provider:13455` / `admin_review_ttl_follows_the_trusted_provider:13416` / `demoted_clock_after_startup_fails_closed_for_admin_review:13365` / `production_without_trusted_time_is_rejected_at_startup:13339` / `production_with_suspicious_clock_is_rejected_at_startup:13351` / `production_preview_fails_closed_without_trusted_time_in_substrate_builds:13335` | 生产 provider 装配仍缺（生产档启动直接拒启 → provider 注入须随生产装配一起落地，见 Batch 5'「A100 生产 provider 装配」）；`suspicious_if_skew_exceeds` 零消费者 |
| A106 非 Tauri Local Host 参考应用 | **部分** | `examples/local_host_reference.rs` + `src/local_host_reference.rs:275`；CI 真跑 UDS E2E（`ci.yml:160/210`） | 是 `--example`，**不是可分发应用**；无第三方按文档跑通的证据 |
| A107 FFI/C#/Swift/Kotlin golden conformance fixtures | **未落**（只有静态比对） | `tauron-ffi/src/lib.rs:454-473` 用 `abi-v1.json` 断言 abiVersion 与三个 `TauronStatus` 码值一致，再对 `conformance/{c,csharp,swift,kotlin}` 源码做 `requiredSymbols` 字符串包含断言 | **从未编译/执行任何 C#/Swift/Kotlin**；只有 C 经 ASan 真跑 → §141「C/Native FFI ownership 明确」在 Rust↔C 成立，其余语言是「fixture 文本对得上」而非「 ABI 实测对得上」 |
| A108 local/remote chaos + peer-auth suite | **已落**（框架浅） | `v4_local_host_chaos.rs:13/31/59/81`（challenge flood/重放/takeover 围栏）、`v4_remote_host_chaos.rs:57/109/146/180`（nonce/速率/配额/at-most-once）；CI 四 OS + ubuntu（`ci.yml:172/230/233`） | 无真进程崩溃/网络注入框架（当前是模拟注入） |

### Batch 6 — 发布证明：A98 A99 A102 A103 A105 A110

| A码 | 判定 | 证据 | 缺口 |
|---|---|---|---|
| A98 pinned reproducible toolchain/actions/provenance | **部分** | `rust-toolchain.toml:2` 钉 1.98.0；全部 workflow 用 40 位 SHA；`generate-release-evidence.mjs:43-56` 校验 pin | **无 provenance、无 SBOM、无签名 tag**（`release.yml:344` 仅 softprops 软发布）；「Reproducible Build Report」无双构建逐字节比对；pin 校验只在 release 期 |
| A99 OS×arch×profile×runtime matrix | **已落** | `contracts/target-matrix.json`（含 `profiles`: substrate/ffi/extension/tauri-desktop）+ `check-target-matrix.mjs:58-63` + CI 四目标真跑（`ci.yml:110-163`）+ `release.yml:308` | profiles 靠 `coverage` 字段声明，非逐 profile 执行 |
| A102 Event ordering contract | **已落**（接收端补齐） | 发布在 `ordering→queues` 原子窗口内 `issue`（`eventbus.rs:849/962`、`ordering.rs:46-76`）；帧元数据铸造 `eventbus.rs:417-426`；命令面透传（`adapter/lib.rs`、`tauri.rs:499`）；跨语言字段级 `wire-gate` + `v4_host_conformance.rs:93-118`；**轮 11**：接收端判定落到 TS —— `EventOrderingWatcher`（`packages/tauron-host/src/events.ts:107`）判 duplicate / gap / revision-regression，gap 报告一次即重同步、revision 不回退记录值，两个生产消费点各接一条泵（`rpc.ts` 宿主取件泵、`tauron-app-plugin-sdk/src/context.ts` 插件内置泵），异常分支**只上报不吞投递**（wire-gate 用花括号配平钉在分支本体上，防止有人顺手 `continue`）；Rust `OrderingTracker::observe` 定性为**发布侧 oracle**（生产 publish 走 `issue`），语义由 conformance 钉住 | 顺序元数据缺失时 watcher 直接 `return null`（老宿主/旁路投递不产生 seq ⇒ 无守卫，静默）；顺序契约不是可协商能力（无 capability 名、无版本位），per-topic FIFO 与 causal 分支未成文 |
| A103 liveness/readiness/degradation split | **已落** | `health.rs:7/15/22/29` + `alive_but_not_ready:60`/`dead:69`；`cmd_runtime_health:6327` → `tauri.rs:818` → TS `shell-client.ts:576/597`；门禁 `health.rs:92` + `wire-gate:1334` | — |
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
| A06 | Tauri plugin permissions 自动生成 | **未落**：`tauron-adapter/` 无 `permissions/` 目录、无 codegen；权限仍是清单字符串参与摘要（`lib.rs:1033-1036`） |
| A07 | plugin namespace 形态成为默认接入 | **未落**：默认仍是 root handler 注册（`tauron_generate_handler!`） |
| A12 | SecretProvider | **未落**：全仓无该实体；秘密与普通值同层存储（W6 行已登记） |
| A13 | Message completion push，移除正常调用轮询 | **未落**：`host.ts:478` 仍 `host_events_drain` 拉模型（W11 部分） |
| A15 | RuntimeDriver | **未落**：无统一抽象（W9） |
| A19 | ProcIoDriver shared async reactor | **未落**：实体零命中 |
| A23/A24 | Runtime Pack 格式 + WASM 引擎/WIT | **部分（轮 22 更正）**：`tauron-wasm` 依赖表里有真引擎（`wasmi = "2.0.0"`），`WasmEngine::execute` 真编译/真实例化/真调导出；**未落的是 Runtime Pack 格式与 WIT 接口层**，以及适配层投递对该 provider 的消费（回执仍 `delivered:false`）；安装路径仍只接受 JS 插件（`E_PLUGIN_TYPE_NO_RUNTIME`） |
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
| target os/arch/abi compatibility | ✅ | 模型完整、门禁断 os×arch；变体过滤已接安装/更新装配腿（A71，轮 49：无兼容硬拒、download/install 同口径） |
| FFI ownership defined if exposed | ✅ | A72 + ASan 真跑 |
| execution/thread affinity declared | ✅ | `execution.rs:89` |
| ServiceGraph acyclic | ⚠️ | 环检测有（装配期 panic）；两序字段轮 11 已删（装饰），服务级回收动作仍缺（A75） |
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
| upgrade generation separation | ⚠️ | runtime ✅（轮 25 起 `GenerationRegistry` 的 abandoned generation 也有界：回收连带 `forget`、号源全局单调、`GenerationStats` 读数跨 IPC）/ provider+plugin 激活 ❌（A88/A90） |
| pack/cache GC lease-safe | ✅ | A89（Pack Manager 缺属上游功能，不扣 DoD） |
| panic fault boundary deterministic recovery policy | ⚠️ | settings/事件/审批/注册表四处包边界（A91 轮 47，各带确定性修复语义与实测）；http/fs/process 与 registry 的调用/流/运行时读取面仍未包 ❌ |
| durable records detect corruption/stale generation | ✅ | A93 |
| package verification/extraction uses bounded memory | ✅ | 验签/解包/激活摘要全链流式；门禁跑**真实安装流**（96 MiB 载荷，堆 + VmHWM 双读数，A94 轮 44） |
| filesystem scope resistant to symlink/reparse TOCTOU | ⚠️ | Unix handle-based ✅，Windows `Partial` ❌（A95）；`PortablePathValidator` 维度缺（A85） |
| network redirects/resolution respect scope | ✅ | 逐跳/DNS 复检在生产路径（轮 48：`cmd_http_request` 宿主侧单跳循环；`resolve_redirect_location` / `authorize_redirect` / `authorize_resolution` 全部真实消费）；真实 provider 缺省仍如实 Unsupported |
| process isolation capability honestly classified | ✅ | 自标 `Partial` 且生产拒 spawn（A97） |
| build toolchain/provenance pinned | ⚠️ | toolchain/actions pin ✅，provenance/SBOM/签名 ❌（A98） |
| Official target matrix actually tested | ✅ | 四目标原生 runner 真跑（A99） |
| migration rollback/data compatibility proven | ✅（有已写明并钉住的边界） | 事务回滚 + **磁盘回滚镜像** + 重启级 E2E（A101，轮 11）：镜像先于新文档落盘、装配在坏文档时消费它，**且消费即作废、健康启动读到好数据时也必须回执作废**（轮 16 R3 改判：镜像的保护窗是「迁移提交 → 首次证明可读」，留着陈旧镜像只会在下次损坏时把用户退回迁移前）。**边界**：回滚带回的是迁移前的**编码层**——v1 文档存裸键，`cmd_settings_get` 先转义再查，因此带点键要重新迁移才可读（数据没丢，`a101_rollback_image_restores_pre_migration_state_after_a_restart` 把「回滚带回值 → 重新迁移后带回可读性」钉成断言） |
| liveness/readiness/degraded separated | ✅ | A103 |
| no lock held across await/provider callback | ⚠️ | 门禁在，覆盖 2 crate、文本级（A104） |

计数（按上表逐行数得，轮 44 后）：**✅ 17 / ⚠️ 14 / ❌ 1 = 32 行**。标题里的「33 项」沿用
§134 原文口径；本表的行数是 32，差的那一项在本文档里既没有行也没有登记——**记为文档侧
待办**，不是「已达成」。轮 10 此处写的是 13/19/1，与本表长期不同步，同 §1.1 那类断链。
按 §134 末句「少一项不得标 stable/full」——
今天没有任何一条能力线可标 full，对外措辞必须继续按此执行。

---

## 6. §136 40 条门禁 → 实际形态

| 形态 | 条数 | 条目 |
|---|---|---|
| **已在 CI（blocking）** | 28 | Production Config、Wire-gate 侧 Error Registry Drift、Target Variant、Thread Affinity、ServiceGraph Cycle、Call Terminal Race、Call Graph Cycle、Event Causation Loop、Slow Consumer/Credit、Fair Admission Stress、Install Review Token TOCTOU、Installed Artifact Tamper、Local IPC Peer Auth（Linux/macOS 腿）、Single Writer Storage、Generation Isolation、Pack GC Lease、Panic Fault Injection、Recovery Idempotency、Durable Corruption、FS Symlink、HTTP Redirect、Sandbox Honesty、OS×arch×profile Matrix、Clock Trust Fault、Migration Rollback、Liveness vs Readiness、No-Lock-Across-Await、Public Surface Orphan、Streaming Installer RSS（轮 44 起测**真实安装流**：96 MiB 载荷探针走 preview→reviewed 提交链，堆峰值 + VmHWM 双读数） |
| **部分（在 CI 但断言面窄）** | 5 | Headless Zero-Surface（只有构建/依赖门）、Schema Evolution（无跨版本 fixture 矩阵）、FFI Sanitizer（只 ASan）、Toolchain Pin Gate（只在 release 期）、Settings Transaction Watch（单测级，无命令面 E2E） |
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
| Universal | 「Local Host Service 是第二参考 Host」（F5）、「Headless 可零 Surface 运行」（A66） |
| End-to-End | 「provider hot swap generation 不混线」（A74/A88）、「reconnect 后资源可恢复或明确释放」（remote 无消费者） |
| No Orphan | 「no subscription orphan」——A81 revoke 后既有订阅继续收帧；本方案 Batch 0 另登记 5 个死类型/死结构 |
| No Infinite Loop | 全部满足（retry/reconnect/placement/graph/call/causation/migration/recovery/updater 十条中，除 **placement bounded** 因 A46 未落而无从谈起） |
| Production Security | 「Production default fail-closed」（F1/F2）、「permission epoch/revoke safe」（A81）、「local IPC peer authenticated」（F5）、「secrets never enter logs/traces」（A54 未落）、「supply chain anti-rollback/revocation」（A49 未落） |
| Performance / Memory | 「fair per-owner admission」（F4）、「idle zero meaningless worker/polling」（A13 拉模型） |
| Failure / Recovery | 「StartupReconciler deterministic」——仅 settings 有 `reconcile_settings_boundary`，无统一 A56 协调器 |
| Release Proof | 「pinned toolchain/actions」✅、其余「External Consumer / non-Tauri reference Consumer / N-1 compatibility / security hard gate」中 **N-1 与真实第三方 Consumer 无证据**（A30/A31 之外的 2.0 判据） |

---

## 8. 收口路线图

### Batch 0 — 下一轮就做（全是「默认值改一刀 + 死代码收口」，1–3 天）

原则：不动已发布命令面形状，不引入新依赖，只把 fail-open 改成 fail-closed、
把死类型接线或删除。每项都必须自带一条红→绿的门禁。

| # | 动作 | 落点 | 验收门禁 | 兼容性风险 |
|---|---|---|---|---|
| 0-1 | Production 下空 origin 允许清单视为 **not-ready**（开发/测试态保留缺省放行） | `crates/tauron-adapter/src/tauri.rs:2990`（gate 本体）+ `crates/tauron-host/src/production.rs`（新增 readiness 项） | `v4_host_conformance.rs` 加一例：Production + 空清单 ⇒ doctor `productionSafe=false` 且 invoke 被拒 | 低。存量装配若曾依赖「空清单=不过滤」需显式改为开发态或写清单 |
| 0-2 | `resolve_principal` 对**未知 label 返回 `Invalid`** 而非 `MainWindow`；主窗 label 由装配配置声明 | `crates/tauron-host/src/authz.rs:525`、`resolve_principal:4528`（`Caller::from_label` 只拒畸形 `plugin-` label，非 `plugin-` 仍解析成主窗） | `authz.rs` 反向用例（伪造/次级 label ⇒ 无 admin）+ `adapter/lib.rs` ADMIN_COMMANDS 档位一致性测试扩至全部主窗命令 | **中**：现网若有非 `plugin-` 的次级窗按主窗调用会被拒，需灰度并在 ledger/接口文档写明 |
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
| 0-3 admin 审计事实源 | ✅ **已落（轮 11）** | `crates/tauron-host/src/admin_audit.rs`：`AdminAuditRecord`（链式哈希，`prev_hash` → 下一条）+ `AdminAuditSink`（`DurableEnvelope` 落 `admin-audit.json`，512 条环形裁剪，`verify_file` 可离线复核）+ `AUDITED_ADMIN_COMMANDS` 8 条（判定标准=改状态/授权/供应链，只读特权刻意不入表以免挤掉真实授权事实；轮 40 增补 `host_market_download`/`host_market_install` 后 6→8）；写口是 admin 分发单点 `record_admin_audit:4634`），Allowed 与 Denied 都留痕；`AdapterConfig::with_admin_audit(bool)` **删除**，readiness 与 doctor 都由 sink 的 `healthy()` 推导（`production_doctor_report:6665`） | host `admin_audit.rs` 6 例（命令名表自洽、链序、篡改断链、环形裁剪后仍验、落盘回读离线复核、撕裂文件拒开）+ adapter 6 例（`an_admin_call_leaves_a_structured_audit_fact`、`a_denied_admin_attempt_is_audited_without_side_effects`、`doctor_derives_the_admin_audit_check_from_the_live_sink`、`production_without_an_audit_directory_is_not_ready`、`admin_audit_survives_a_restart_of_the_host`、`audited_admin_commands_are_real_dispatched_privileged_commands`，均在 `adapter/lib.rs` 的测试模块）+ `wire-gate`「特权管理操作必须在唯一咽喉点产出结构化审计事实」（命令名集合双向对账） |
| 0-4 死类型收口 | ✅ **已落（轮 11）** | `CreditWindow` **已接线**（轮 10：`StreamHandle.credit`，`grant`/`consume` 为唯一额度算术）；`FairQueue` **已删除**（轮 10）；`ActivationRecord::verify_bytes` **已接线**（轮 11：`PluginAssetTrust::verify_asset` `adapter/lib.rs:810→:843` ← `tauri.rs:1738` 逐资产服务）；`MigrationSnapshot` **已删除**（轮 11，与 `MigrationReceipt.before` 同一事实的两份表达，A101 改落 `DurableEnvelope` 镜像）；`OrderingTracker::observe` **定性为发布侧 oracle**（生产走 `issue`，接收端判定在 TS `EventOrderingWatcher`），后三条登记在 V4「未接线公开 API 台账」轮 11 小节 | `wire-gate`「stream credit 必须由 admission::CreditWindow 单一裁决」+ 轮 11 新增的 A84/A101/A102 三条门禁（含「`MigrationSnapshot` 不得复活」反向钉）；`generate-public-surface-ledger.mjs --check` no orphan |
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
| A84 逐资产摘要复核 | ✅ 已落 | `tauri.rs:1738` 服务路径 → `adapter/lib.rs:810 verify_asset` → `:843 record.verify_bytes`；403/404 分义 | adapter `PluginAssetTrust` 篡改/注入/越权三组用例 + `wire-gate` 服务侧复核钉 |
| A101 迁移回滚镜像 | ✅ 已落 | `cmd_settings_migrate` 先取快照→先落镜像→再写正式文档；装配 `:3152` 消费并作废；`:5780` 是救不回来时的如实记录 | `wire-gate`「A101 迁移回滚镜像必须落盘、先于新文档写、并在重启时被消费」+ adapter 三轮重启 E2E |
| A81 撤销效力 | ✅ 已落（范围内） | `eventbus.rs:597 revoke` → 退订既有订阅 + `:633 drop_queued` 作废三类通道待取帧；`cmd_events_revoke_as` → TS `eventsRevoke` | host `revoke_racing_publish_leaves_no_revoked_content_behind` + `wire-gate` A81 效力钉（两侧文档同步） |
| A102 顺序契约接收端 | ✅ 已落 | TS `EventOrderingWatcher`（`events.ts:107`）接进 `rpc.ts` 宿主泵与 `tauron-app-plugin-sdk/src/context.ts` 插件泵；异常只上报不吞投递 | `wire-gate`「A102 顺序契约必须有接收端判定…」用花括号配平钉异常分支本体 + 三处消费点测试 + `v4_host_conformance.rs:93-118` |
| A91 × A104 设置事务边界 | ✅ 已落 | `run_settings_boundary` 两段式（就绪判定 / 事后 `record_panic`）；写租约成为**唯一**串行化点，四个写点 `:6177/:6317/:6350/:6388` 全持它 | `wire-gate`「设置族闸门必须只判就绪…」+ adapter `concurrent_settings_writes_never_lose_a_key_or_share_a_generation`（**红→绿**：去掉租约 ⇒ 同 generation 或丢键，实测红；恢复 ⇒ 绿） |

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

**轮 11 未做（不得读成已做）**：Batch 1'–6' 的其余条目（~~A65 Profile V2~~（轮 45 已落）/~~A66 零 Surface 就绪~~（轮 46 已落）、~~A91 扩边界（事件/审批/注册表）~~（轮 47 已落）、~~A85 路径校验五维~~（轮 42 已落）、A86+A106 第二 Host 生产化、A95 Windows handle 强制、~~A96 重定向/DNS 复检~~（轮 48 已落）、A98 provenance/SBOM/签名、~~A83 uninstall/purge/update token 覆盖~~（轮 43 已落）、~~A94 安装路径 RSS 门禁~~（轮 44 已落））
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

#### 轮 13 执行状态（2026-10-03，对照基线 = V5 方案）

本轮的输入是新入库的前瞻方案
[Tauron-Architecture-Competitive-Analysis-Optimization-Plan-V5.md](../Tauron-Architecture-Competitive-Analysis-Optimization-Plan-V5.md)
（已进 `docs/architecture/README.md` 与 `README.md` 文档索引，标注为**对照基线、
不描述现状**）。V5 §46 给了三条可直接执行的修订建议，本轮逐条落实：

1. **§46.1 立即更新架构状态文档** —— 全部核实为真并落笔：`tauron-brand` 已有真实
   `host_brand_info` 接线（env → `tauron-brand` 校验 → `BrandInfo`，未配置时
   `UnsupportedBody`，不是恒 `{}`）；`tauron-theme` 已进真实主题命令面
   （`cmd_theme_list_as/get_as/set_as`，`SubstrateState.themes` 持
   `tauron_theme::ThemeRegistry`）；`tauron-distribute` 已进 updater provider
   （`DistributeUpdaterSink::check_for_update`）；crate/包计数已按文件系统重测
   （16 crates / 21 个包目录 = 20 公开 + 私有 contract-tests）。同步修正文档六处
   （overview.md 接线表与依赖图、canonical-owners.md「轮 13 复核」全量依赖表、
   competitive-analysis.md 四行改判、app-layer-wire.md 品牌条目、
   incremental-adoption.md §5 三行、capability-closure-plan 与
   full-architecture-refactor-plan 的计数加「写作时点快照」标注）。
   其中含一处**旧文档的谎报**：incremental-adoption 曾写 `host_brand_info`
   「桩：恒 `{}`」——该描述从未为真（它一直返回 `UnsupportedBody`），属
   负向谎报（把已接线写成未接线同样是断链），已改判并留更正记录。
   canonical-owners 轮 10/11/12 的依赖表只记了孤儿激活增量、漏报了 b2659c0
   引入的 brand/theme/distribute 默认依赖，轮 13 复核节已按 `Cargo.toml` 全量重测并写明。
2. **§46.2 gap 台账机读化** —— 新增 `contracts/module-maturity.json`
   （16 crates：`adapterDependency` 取 default / `feature:plugin-install` /
   `feature:runtime-wasm-broker` / null，`overviewWiring`、`ownersTable` 三字段；
   21 packages：层级 + private）。台账不是孤儿文件：`wire-gate` 新增门禁
   「轮 13：module-maturity 台账与文件系统/Cargo/文档三方同源」，把它与
   `crates/`、`packages/` 目录清单、各 `package.json`、`tauron-adapter/Cargo.toml`
   依赖表、overview.md 接线表、canonical-owners 0.3 表、README 计数、
   competitive-analysis 计数**逐项对账**，任何一侧漂移即 CI 红。
   V5 建议的 Stable/Preview 分级未写入台账——那需要 conformance 证据，属 Phase A
   出口条件，台账 purpose 字段已注明原因。
   红灯探针（防假绿）：篡改台账 `tauron-brand overviewWired→"partial"` →
   `AssertionError: overview.md 接线表与台账对 tauron-brand 判定不一致`，还原后绿。
   开发中发现并修复两类门禁缺陷：① 接线表标签单元格带 emoji 变体选择符（U+FE0F）
   导致解析漏行——根因修复为**标签单元格纯文本化**（全表去 emoji）；
   ② crate/命令标识符正则 `[a-z-]+`/`[a-z_]+` 匹配不到含数字的 `tauron-i18n`、
   `host_i18n_*`（首轮只解析到 8/16 行、79/85 条），改为 `[a-z0-9-]+`/`[a-z0-9_]+`
   并加「解析行数下限」防 0 匹配假绿。
3. **§46.3 命令面冻结** —— `wire-gate` 新增断言：command-surface.md 根 `host_*`
   命令行计数 `toBe(85)`，消息「V5 §46.3 冻结：新增根 host_* 命令必须显式过账」。
   本轮**未新增任何**根命令，前后端消费点复核通过（theme/updater/brand 在
   `shell-client.ts` 与 `tauri-backend.ts` 白名单均有真实调用）。

**轮 13 门禁实测（本轮日志，非转抄）**：

```text
pnpm -C packages/tauron-contract-tests test       rc=0  Test Files 2 passed (2) / Tests 165 passed (165)
pnpm -C packages/tauron-contract-tests typecheck  rc=0
pnpm command-surface:check                        rc=0  85 commands（底座 61 / 运行时 22 / 安装 2），孤儿命令 0，未归类 0，无代码层判定 9
pnpm docs:check                                   rc=0  行号引用核对：6 个文档，198 条引用 OK
```

Rust 侧本轮零改动（全部为文档/台账/TS 门禁），轮 12 的 cargo 全绿结论仍有效，
发布前在轮 15 统一复跑。

**轮 13 未做（不得读成已做）**：V5 的 Phase A–G 前瞻项**全部如实登记为路线图，
未假落地**——adapter 按域拆分、RuntimeDriver SPI / HostProtocol envelope、
真 process sidecar E2E fixtures、Manifest V3、LocalHost 第二官方 host、
WASM 真执行引擎、observe 层与 SLO 趋势基线、N-1 协议兼容、machine ledger
从 JSON 生成 Markdown（本轮台账是**对账源**而非生成源）。V5 §45 的实施顺序
（A canonical 收敛 → B 通用契约 → …）是下一轮的开新能力前置门槛。

#### 轮 14 复核（2026-10-03，独立复查轮 13）

对轮 13 的门禁与文档做了 fresh-eyes 复查，结论：

1. **同源门禁逐段复核通过**：⑤ 的 slice 边界实测定位在
   canonical-owners「## 0.3 crate 归置决策」（:35）与「## 依据」（:49）之间，
   轮 13 新增的「轮 13 复核」节（:106 起）确认落在 slice **之外**，不会把
   复核表误当归属表对账；③ 的依赖行解析假设**单行 inline table**
   （`tauron-acl/market/wasm` 实测均为单行，:34/:35/:53）——若未来改成多行，
   `optional` 关键字落在续行会把该 crate 误推成 `default`，门禁会**红**而非
   假绿，属可接受的 fail-loud；⑦ 冻结断言与生成器复算（85 = 61+22+2）双源一致。
2. **发现并修正轮 13 的漏网残留**：`multi-plugin-substrate-roadmap.md` §S4
   决策表仍保留三处与现状矛盾的表述——`tauron-brand` 行「恒 `{}` 是桩」
   （从未为真）、`tauron-theme` 行「crate 侧等待消费者」（已有
   `host_theme_*` 消费点）、`tauron-distribute` 行「不进客户端运行时」
   （updater provider 已在客户端）；wasm/acl/market 行补「现状口径」。
   这正是轮 13 门禁**故意不覆盖**的方向：台账对账钉的是 overview/0.3 两张表，
   路线图散文不在锁定面内——散文的诚实性仍靠逐行核查，此例说明扫描面要
   覆盖「同主题的**所有**文档」而非只改被点名的三份。
3. **`tauron-ffi`「零仓内消费者是设计使然」复核为真**：CI 有专属两步
   （`cargo test -p tauron-ffi` + C11/ASan conformance harness 链接
   `libtauron_ffi`），crate 477 行、11 个 extern 函数、契约文档
   `docs/contracts/ffi-v1.md` 在仓。判定：非孤儿逻辑，消费点是**文档化 ABI +
   仓内 C 一致性测试**；overview 表述不改。
4. **registry 计数口径复核**：competitive §5.1 的「均未发布」旧断言已由同页
   2026-10-02 再复核横幅自我更正；README/installation 的 15 crate / 20 包均为
   **registry 已发布口径**，与文件系统口径（16/21）分开表述，无矛盾残留。

**轮 14 门禁复跑（本轮日志）**：修正 roadmap 后
`pnpm -C packages/tauron-contract-tests test` rc=0（**Tests 165 passed**）；
`pnpm docs:check` rc=0；`prettier --check` 三份改动文档（roadmap、v4 方案、
CHANGELOG）均 OK。Rust 侧轮 13/14 零改动。

#### 轮 15 发布条件终审（2026-10-03，全量复跑）

发布前按 V5 §42 的门槛口径复核现状并如实登记：**代码侧条件满足**——Rust
（fmt/clippy×2/workspace 1458 passed/特性矩阵 300·289·323·72/check 三项）、
TS（build/typecheck/test/lint）、八个发布脚本与 `publish:npm --check` 20/20
全部 rc=0；`examples/minimal-app` NSIS 安装包重建成功
（1.1.0 x64，3.39 MiB）。终审中抓到并修复轮 13 的一处自留问题：
新增的台账 JSON 与 wire-gate 用例**未过 prettier**，`format:check` 首跑红，
格式化后全绿——登记为流程教训：新文件也必须过 format 门禁，不能只查改动老文件。
**不满足、且本轮不假装的**：V5 §42 的第 2/3/6/8/12/13 条（双 host 同过
Conformance、三类 Runtime 同过 Conformance、真 WASM 执行、N-1 兼容、
外部安全评审、真实外部消费者）属 Phase A–G 前瞻，维持轮 13 的如实登记。
发布动作侧：提交/推送需用户授权，`v1.1.0` 标签与 registry 真实发布
（`--publish`）是**打标签即触发公开构建**的高影响动作，同样等人。

#### 轮 16 执行状态（2026-10-03，全仓代码链路深检）

本轮不改口径、只改代码：按「主体流程连通 / 无断链 / 无孤儿逻辑 / 无死循环 /
前后端贯通」五个维度逐链路走查 `tauron-host` → `tauron-adapter` → `tauri.rs` 命令面
→ TS 消费者，并横扫 npm 侧 CLI 的退出码与生成物。**Rust 有实质改动**（五个文件），
这同时暴露出文档行号引用的系统性漂移，一并收口。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| R1 | `take_call` 取件后 `call_bindings` 不清扫 | **真无界增长**。生产终局是 reply→settle→take，`host_call_end` 是可选细帧、仓内 SDK 主路径不调；`gc_expired` 只扫 pending 表，`MAX_STREAMS` 只数句柄，不构成容量闸。取件处补 `close_for_call`（与 `end_call` 同款锁序），回归 `settled_take_call_sweeps_stream_binding_like_end`。 |
| R2 | `PolicyAuthority.grants` 只有插入点、无删除点 | `dispose_subscriber` 清了 approvals/queues/subs/ordering 却漏它，按主体无界滞留。新增 `PolicyAuthority::forget:83`，语义按 **fail-closed** 定：删行后 `grant_version` 回落 0，旧 token 判 `StaleGrant` 而非「无版本即放行」；断言打在 token 校验结果上，不只断言行被删。 |
| R3 | A101 回滚镜像的一次性语义只做了一半 | **改判**。原口径「保到下一次成功消费」实现里只有坏文档路径会删，健康启动读得动时镜像留着 → 相对前进中的正式文档永远陈旧，未来任何无关损坏都会把用户带回久远过去。补第三出口：`with_adapter_config` 健康载入即 `clear_settings_rollback_image`；完整形式是「消费即删 ∧ 回执即删 ∧ 健康载入即删」，`a101_…` 的断言随之反写。 |
| C6 | `install_config` 存集成方原样路径 | 真断链：`window_create` 用 `installed.entry.strip_prefix(install_config_root())`，`entry` 已 canonicalize、存的没 canonicalize，多一个 `.`／相对路径／符号链接／Windows 8.3 短名即 `E_INSTALL_FAILED`。与 `fs_allowed_roots` 同规则装配 canonical 副本；目录不存在时按原样保留。CI 干净临时目录踩不到，回归刻意用 `<root>/./plugins`。 |
| C1 | `tauron plugin publish` 前置分支返回 `success: true` | 缺 manifest／缺 `.tgz` 时报成功，`publish && next-step` 在**什么都没发布**后继续走。改判 `success: false` + `published: false`，文案不动。 |
| C3 | `tauron doctor` 报告说坏、退出码说好 | `runDoctorCommand` 无条件 `true`，而报告打印「✗ N check(s) failed — please fix before continuing」，CI 直接抹掉诊断结论。抽出 `doctorCommandResult` 按 `fail` 计数判 `success`；warn/skip 不改判（是提示不是失败）。 |
| C4 | `doctor` 工作区探测指向不存在目录 | **负向谎报**：按 `packages/core`/`ui`/`cli` 找，真目录是 `tauron-host`/`tauron-ui`/`tauron-app-cli`（`@tauron/*` 只是包名），三个包永远报缺；且 `tauron-app init` 出的应用没有 `packages/`，对它报 warn 把正常应用说成坏。改探真路径 + 非框架仓整项 `skip`。 |
| C2 | 脚手架工程按 README 第二步就装不上 | 前后端贯通意义上的真断链：`@tauron/plugin-sdk` 钉在 `CLI_VERSION`（本轮 1.1.0）而 registry `latest` 是 1.0.2 → `npm install` `ETARGET`。仓库源码版本与已发布 npm 版本是两个事实、必须各有其主：`@tauron/cli` 新增 `tauron.publishedNpmVersion`，`version.ts` 导出 `PUBLISHED_NPM_VERSION` 供脚手架与 README 片段用，两者一致时文案自动收敛。 |

**文档行号引用系统性漂移（轮 16 的自留问题，按根因收口）**：上述 Rust 改动使
`lib.rs` +24 行、`policy.rs` +12、`registry.rs` +6/+12、`eventbus.rs` +6，本方案与
另五份文档里的 `file.rs:NNN` 引用整体下移，而 `check-doc-line-refs.mjs` 的旧判定
（路径解析得到 + 行号在范围内 + 区间不整段空行）**一条都没抓到**——指向一个合法
但无关的行完全通过，这比不写引用更糟。处置分两层：① 把五个高频改动文件的引用
换成**符号锚点**（`` `run_settings_boundary:7192` ``／`` `review_token:4886` `` 这种，
门禁要求符号字面落在被指区间，行号漂移即红），本表 L90–L182 的执行态列已全部改写；
② 给检查器加一条判定——`path.ext:NNN` 的**起点行**若整行只有括号/逗号/引号/空白，
即使行号有效也判不可核对（代码增删后下移的引用几乎总以「跳到一个 `}`」露馅）。
开发过程中写过一版 `check-doc-ref-drift.mjs`（直接比对 HEAD 行内容），实测把**已修正**
的引用报成 38 条漂移，口径本身不成立，已删除而不是留着当噪音门禁。
重写过程中我自己把两个锚点行号写错（`load_settings_rollback_image` 多写 5 行、
`trusted_clock_enforces_expiry` 多写 1 行），外加一处历史遗留的 `ci.yml` 数字引用落在空行上，
三条都被新判定抓出来——这是门禁首次生效的现场，也说明它判的是「跳过去看不看得到
被引用的东西」，不是「数字像不像」。

**已知未覆盖口径（如实登记，不假装闭合）**：六份文档本轮实测仍有 **148 处
`path.ext:NNN` 数字引用**（其中 **135 处集中在本方案文档**，另 11 处在
`incremental-adoption.md`，其余散在接口文档），多数落在本轮未改动的文件，
只受「范围 + 非空 + 非纯标点」三条弱判定，行号漂到另一行合法代码上仍然通过；
段落里续写的裸 `:NNNN` 没有文件名头，解析器根本不读，属完全未覆盖。
彻底解法是把引用面收敛到符号锚点，但那是跨全仓文档的一次性重写，不在本轮范围。

**轮 16 门禁复跑（本轮日志）**：`cargo fmt` / clippy×2 / `cargo test --workspace --locked`
（37 target **1460 passed**，较轮 15 净增 2 = R1 + R2）全 rc=0；特性矩阵 `tauron-host --lib`
**399**、adapter **290**（plugin-install，+1 即 C6）／**300**（tauri，该测试被 cfg 门控故不涨）／
**324**（双特性）／shell **72**；`pnpm run verify` rc=0（contract-tests **166 passed**，
`wire-gate.test.ts` 单文件 145）；`docs:check`「6 个文档，**232 条引用** OK」
（轮 15 为 198，上升来自符号锚点被解析到，非篇幅膨胀）；`command-surface:check`
「85 commands，孤儿 0，未归类 0，无代码层判定 9」；`version:check`「26 处全部 1.1.0」；
`format:check` OK。**安装包本轮需重建**：Rust 侧五个文件有实质改动，轮 15 的
`Tauron Minimal App_1.1.0_x64-setup.exe` 已不是当前源码产物。

#### 轮 17 复核（2026-10-03，独立复查轮 16 并修复全部新发现）

轮 16 的自查口径是「它自己改过的地方」，本轮换成**站在轮 16 之外重看它留下的东西**：
新写的门禁是否判得了它声称判的事、新加的回收路径是否只补了一半、新起的公开面有没有
读者。八个发现里 **六个是轮 16 自己留下的**（三条是同一修复只做了一半）。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| G1 | 轮 16 的包级 `consumerStatus` 门禁用**裸子串**匹配包名 | 门禁绿着说谎：注释里提一句包名就算「消费者」。收紧成三条件同时成立（解析 `from`/`import()`/`require()` 的**说明符**本身等于包名或其子路径 ∧ 该源文件的 `package.json` 真声明了它 ∧ 状态在枚举内），另补「`bin` 指向的文件必须存在」与「未知 `consumerStatus` 直接抛」。结果是一次**改判而不是放宽**：`@tauron/adapter-react` 由 `repo-consumed` 降回 `reference-only`，`competitive-analysis.md` §2.2 那句「✅ 已接线」随之改为分包判定。 |
| G2 | registry 现值只有 `@tauron/cli` 有主 | 同类断链在 `@tauron/app-cli` 重生：它的 `create` 仍把 `@tauron/*` 与 crates.io 钉在未发布的 1.1.0。现按 `package.json.tauron.publishedNpmVersion`（1.0.2）出真值，registry 模式打印「装不上 + 用 `--tauron-path`」。**不把钉降到 1.0.2**：那是把「装不上」换成「装上但编译不过」。新门禁钉住两 CLI 同源、源码内不得再现 `= '1.0.2'` 字面量、两份文档必须写到该现值。 |
| R2a | `grants` 版本号按主体自增 ⇒ 回收后可重放 | 轮 16 补了 `forget` 却没改发号方式：删行回落 0、重新 approve 又回 1，而 `validate_scoped` **不看 nonce** ⇒ dispose 前签的旧 token 重新有效。改全局单调发号器（`grant_sequence`），性质由结构而非调用点保证。 |
| R2b | `dispose_subscriber` 的 `forget` 取在 approvals 临界区**外** | TOCTOU：与并发 `approve` 交错可清掉刚点亮的授权。移进临界区，锁序 approvals→policy 与 `approve` 侧一致。 |
| R2c | 回收只做了一半：revoke 侧仍留行 | `cmd_events_approve_as` 接受任意 subscriber 串，旧实现每串永久留一行（`MAX_APPROVALS` 管不到版本表）。补 `reclaim_grant_when_unapproved` 挂在 revoke **两个出口**，且只在「一条审批都不剩」时删——还剩审批就删会把其它 topic 的有效授权打成 `StaleGrant`，**过度回收与漏回收同为缺陷**（反向断言同测）。 |
| R3 | `clear_settings_rollback_image` 的 `let _ = remove_file` | 删不掉 ≠ 不用管：权限/占用/目标是目录都被咽下，留下的正是该函数要防的陈旧镜像。改 remove→rename `.discarded-<ms>`→两败才留痕；镜像的可消费性来自**固定文件名**，改名即失效。测试用目录冒充「存在但删不掉」（两平台都报非 `NotFound`），并钉幂等。 |
| C6b | 轮 16 的 canonical 修复在更窄路径上留了原 bug | 装配期「目录还不存在 → 按原样保留」时，比较点两侧仍不同形 ⇒ 症状照旧是「装得上、开不了窗口」。取径判定收进唯一出口 `plugin_window_asset_relative`（可单测），在比较点 canonicalize，失败退回原值即判不同形 ⇒ 照 `E_INSTALL_FAILED` 拒绝。两条新测试；其中失配用 `..` 段构造——**`.` 段不算失配**（`Path` 按组件比较会忽略它），这个前提此前被写在注释里当卖点，是错的。 |
| G3 | 轮 16 自己留下了孤儿公开 API `grant_count()` | 只有测试在调——与本仓 `13bfa59` 清过的那类同形。删除，判据改用 `grant_version`（全局发号 ⇒ 版本 0 与行不存在同义），不为一处断言扩对外表面积。 |
| G4 | 轮 16 的两条门禁**判定了它们管不着的东西** | ① A101 作废出口计数扫整文件 ⇒ 本轮补两个单测就把自己弄红（`expected 5 to be 3`），这种红教人的是「别给生产语义加测试」；改为只数 `mod tests` 之前的生产区，锚点找不到即抛。② 「`drop_queued` 之后必须回到 `had_grant`」写成**相邻两行** ⇒ 中间插一句合法回收即失配；改为 120 字符内保持**顺序**。两条都有探针实录（见下）。 |

**引用漂移的工具化收口**：本轮代码插入让六份文档 **57 处**符号引用整体下移，
`check-doc-line-refs.mjs --fix` 只重锚「候选文件 ±150 行窗口内的唯一最近解」，
多解／越窗／起点是标点的一律留红给人判——机器猜数字等于把门禁换成谎报器。
人工修的 2 条值得单独记：本方案里指向 `adapter/lib.rs` 的两处数字引用
（原写 2915–2932 装配期 panic、1849–1877 `StdFsSink` 退化重检）**不是本轮漂的**，
是轮 16 写下时就指错了目标（分别跳到托盘类型与 `SpawnerReaper`），本轮第二次跑才被
轮 16 自己新加的「起点行不得只有标点」判定抓出——那条判定至今最值钱的一次生效。
现行锚点：panic 在 `adapter/lib.rs` 的 A87 租约装配段（按符号锚定——从
`let storage_writer_lease = …` 到那句 `durable-state single-writer lease rejected startup`；
这里曾写行号区间，两次因上游插行而漂，故改为不带数字的锚点），退化重检在
`impl FsSink for StdFsSink:2233-2253`。

**已知未覆盖口径（本轮量化，不假装闭合）**：六份文档实测 **235 条被解析的引用**
中 **149 条是 `path.ext:NNN`**（136 条集中在本方案、11 条在 `incremental-adoption.md`、
其余 2 条散在接口文档），只受「范围 + 非空 + 非纯标点」三条弱判定；符号引用 86 条受
「符号必须字面落在被指区间」这条强判定。**新增量化**：另有 **33 处裸 `:NNNN` 续写**
（全在本方案）解析器根本不读，是完全未覆盖的口径。彻底解法是把 149 条数字引用继续
收敛到符号锚点并让续写带上文件名头，那是跨文档的一次性重写，登记为 Batch 1' 的
文档债，不在本轮范围。

**这些数字的复核边界（本轮顺手查清，写给下一轮）**：轮 13 起的改动全部还没提交，
HEAD 仍停在轮 12 的 `305e2d6`（`git show HEAD:` 取出的本方案只有轮 13–17 之前的正文），
所以上文各轮日志里的引用数**只能对应当时的工作区、无法从 git 复算**。本轮的 235 由
`node scripts/check-doc-line-refs.mjs` 给出，149/86/33 的分类由临时清点脚本给出（脚本
不入库，判定面仍以门禁为准）。轮 18 提交后这批数字才第一次变成可复核基线。

**其余如实登记的残留（本轮判定为「不在本轮修」而非「已修」）**：
① `take_call` 的 Settled 分支先删 pending 行、后做 admission/stream 回收，两步不是
原子的——今天两条副作用都不返回错误，panic 才会留下半回收，代价与修复都不成立；
settle 后无人 take 的行由过期回收扫，不构成长驻。② `tauron doctor`（`@tauron/cli`）
唯一的 `fail` 条件是「`node --version` 探不到」，在正常宿主上确实窄，但
`doctorCommandResult` 的失败分支有测试真跑到（`cli.test.ts`），不是死代码。
③ `examples/*` 不在 `contracts/module-maturity.json` 的台账面内（它记 crate 与 npm 包），
示例的接线状态仍靠文档措辞承担。④ `publish:npm --check` 校验包内容与 bin 存在，
但 bin 目标的**文件存在性**进 CI 是靠本轮的 wire-gate 断言，不是靠发布脚本。
⑤ A81 的竞态尾巴（发布方在撤销前已把 token 解析成订阅者）仍靠「撤销时再作废一次
队列」收敛，不是靠串行化——窗口变窄不等于关闭。

**轮 17 门禁复跑（本轮日志）**：`cargo fmt --all -- --check` clean、
`cargo clippy --workspace --all-targets --locked -- -D warnings` clean、
`cargo test --workspace --locked` **37 target / 1463 passed / 0 failed**（轮 16 为 1460，
+3 = 2 条 host + 1 条不受特性门控的 adapter）；特性矩阵 `tauron-host --lib` **401**（轮 16 399）、
adapter **293**（plugin-install，+3）／**301**（tauri，+1 即那条未门控的镜像测试）／
**327**（双特性）；`pnpm -C packages/tauron-contract-tests test` **167 passed**（轮 16 166，
+1 = G2 的 registry 同源门禁）、`@tauron/app-cli` **334 passed / 11 files**、
`pnpm -r --no-bail typecheck` 全包 Done、`pnpm lint` rc=0；`pnpm docs:check`
「6 个文档，**235 条引用** OK」（本轮内两次实测 233 → 235，差值就是上文人工更正的
那两处锚点从「跳过去看不到被引用的东西」变为可解析引用，非篇幅膨胀）、
`pnpm command-surface:check`「85 commands，孤儿 0，
未归类 0，无代码层判定 9」、`pnpm version:check`「26 处全部 1.1.0」、
`npx prettier --check .` 全绿。**红灯探针实录**：改名一处
`reclaim_grant_when_unapproved` → 「必须同时挂在 revoke 的两个出口: expected 1 to be 2」；
生产区加一行含 `clear_settings_rollback_image(&` 的文本 → 「作废出口的消费点漂移:
expected 5 to be 3」（同时证明计数面确实只扫生产区）；把 app-cli 的
`publishedNpmVersion` 改成 1.0.3 → 「两个 CLI 对『registry 现值』各有说法：1.0.2 vs 1.0.3」。
三处还原后复跑全绿，`grep` 确认探针文本零残留。**Rust 与 TS 两侧本轮都有实质改动，
安装包须在轮 18 重建。**

#### 轮 18 发布终审（2026-10-03，全量门禁一次跑全 + 接口文档补齐 + 安装包重建）

轮 17 把链路层面的六个缺陷收口后，本轮**不再改生产代码**：把发布门禁一次跑全（Rust 链式
单跑避开 target-dir 锁、npm 侧并行）、把**轮 16/17 改了行为却只写在内部方案里的口径补进
接口文档**、把安装包重建成当前源码的产物。

**门禁实测（两份日志：`r18-rust.log` / `r18-ts.log`，逐项 rc 记录）**

| 面 | 命令 | 结果 |
| --- | --- | --- |
| Rust 静态 | `cargo fmt --all -- --check`／`clippy --workspace --all-targets --locked -D warnings`／`check --workspace --all-targets --all-features --locked`／`check -p tauron-adapter --no-default-features --locked` | 4/4 rc=0 |
| Rust 全量 | `cargo test --workspace --locked` | **37 个 test result 段 / 1463 passed / 0 failed** |
| 特性矩阵 | `tauron-host --lib`／adapter `plugin-install`／`tauri`／双特性 | **401**／**293**／**301**／**327** |
| Rust 专项 | `v4_host_conformance`、`v4_local_host_chaos`、`v4_remote_host_chaos`、`tauron-proc`、`tauron-ffi`、`tauron-wasm`、两个 reference example | 7/7 rc=0 |
| npm 门禁 | `version:check`／`docs:check`／`command-surface:check`／public-surface-ledger／target-matrix／release-evidence／no-lock-across-await | 全 rc=0：26 处 1.1.0、**235 条引用**、85 commands（61/22/2）孤儿 0 未归类 0、85 public commands 无孤儿元数据 |
| npm 构建 | `pnpm -r build`／`-r --no-bail typecheck`／`lint`（`--max-warnings 0`）／`format:check` | 全 rc=0 |
| npm 测试 | `pnpm -r --no-bail test` + wire-gate 单文件复跑 | contract-tests **167**、`@tauron/app-cli` **334 / 11 files**、`@tauron/cli` **81**、`@tauron/app-contract-kit` **92**、`wire-gate.test.ts` 单文件 **146** |
| 发布面 | `pnpm publish:npm -- --check` | **可发布包 20 个，通过校验 20 个，失败 0 个**（含 dist、入口完整、版本同源、无 `workspace:` 残留） |
| 安装包 | `pnpm -C examples/minimal-app tauri build`（`bundle.targets = "all"`） | rc=0。release 产物 **重建为当前源码**：NSIS `Tauron Minimal App_1.1.0_x64-setup.exe` 3,559,081 B、MSI `..._en-US.msi` 5,410,816 B（轮 15 的 NSIS 是 3,558,752 B——差 329 B 正是轮 16/17 那五个 Rust 改动的体积痕迹）。同目录还躺着 09-27 的 `..._0.1.0_x64-setup.exe`，那是历史构建物、`target/` 已被 gitignore，发布时按文件名取 1.1.0 那一份。行内数字只对应轮 18；轮 23 已按当前源码重建两个包（NSIS 3,574,475 B、MSI 5,431,296 B），当前口径见轮 23 一节 |

**本轮的真实增量是接口文档，不是代码**：三处行为变更此前只有内部方案文档写着，接入方
读不到。① `tauron doctor` 的退出码契约——**有任何 `fail` 项即 exit 1，`warn` / `skip`
只提示不改判**（轮 16 之前它无条件 0，CI 里等于把「✗ N check(s) failed」那句结论抹掉），
`tauron-app doctor` 同口径（`result.ok` 为假即 1；非框架仓的「工作区包就位」项如实
`skip`）→ 写进 `plugin-development-guide.md` 的验证小节与两个 CLI 的 README。判据不是
口头承诺：`doctorCommandResult` 的 fail/warn/ok 三分支在 `cli.test.ts` 有真断言，
`skip` 与「缺包判 fail」在 `doctor.test.ts` 有回归。② `plugin_install_dir` 的「包路径
必须落在 root 之下」到底怎么判——装配期取 canonical 副本、取径侧在比较点**再** canonicalize
一次，因此 `..` 段／符号链接／Windows 8.3 短名这类真换目录的写法判 `E_INSTALL_FAILED`，
而 `<root>/./plugins`（多一个 `.` 段）**不算失配**（`Path` 按组件比较会忽略它）；首装时
目录还不存在则按原样存储、由取径侧兜住 → 写进指南的插件安装命令一节。③ 引用数字与
「这些数字的复核边界」段落（见上），把「HEAD 仍是轮 12 的 `305e2d6`、各轮日志数字只对应
当时工作区」这条事实登记在案。

**发布条件判定**：主体流程连通、四条核心链路（call／event／settings／install→window）
前后端贯通、无未登记的孤儿逻辑；未收口项只有轮 17 如实登记的五条残留（①–⑤）与
Batch 1' 的文档债（149 条数字引用继续收敛到符号锚点）。**代码侧达到发布条件**；
推送、打 tag、npm/crates.io 发布是三件独立的事，各需一次新的授权。

> **这条判定的覆盖面在轮 19 被证明不完整**：它只核了本地门禁，没核 CI 侧的发布链路，
> 而后者才是「能不能真的产出一个公开 Release」。口径更正见下文轮 19。

#### 轮 19 发布链路复核（2026-10-03，commit `6388953` + tag `v1.1.0` 之后）

轮 18 判定「代码侧达到发布条件」后，推送 main、打 `v1.1.0` 附注 tag 并推送。tag 一推就
暴露出**本地门禁看不见的那一段**：`release.yml` 里创建 GitHub Release 的 `release` 作业
`needs: [build, registry-check, release-evidence]`，而 `registry-check` 跑
`scripts/check-published-versions.mjs`——它枚举全部 20 个非 private npm 包与全部 16 个
crate，要求每个都**已经**在 registry 上存在目标版本。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| P1 | `registry-check` 在 1.1.0 上必红 | 实测 `node scripts/check-published-versions.mjs 1.1.0` → **rc=1，36 项全缺（npm 20 + crates.io 16）**，因为 1.1.0 从未发布。这不是门禁的错：**发布顺序本来就是「先 registry，后 tag」**（`publish-sdk.yml` 是 `workflow_dispatch` 手动作业，tag 触发不了它）。处置：登记为发布前置动作，等授权执行 registry 发布后在失败处重跑。 |
| P2 | 同一门禁对**已发布版本**也红 | 实测 `… 1.0.2` → **rc=1，35/36 通过，唯一失败是 `tauron-ffi@1.0.2` 在 crates.io 是 404**。旁证：`list_releases` 返回**空数组**——`v0.1.0`/`v1.0.0` 的历史 Release run（4m45s–8m28s）**从未产出过一个公开 Release**。 |
| P2-判定 | 门禁该不该为此放宽？ | **不该，且这是本轮最容易做错的一步。** 核对后确认 `tauron-ffi` 无 `publish = false` 豁免、manifest 完整（description/license/include 齐备）、就在 `publish-crates.mjs` 的发布集合里（本轮日志：`crates/` 全部 16 个成员都在集合内，`tauron-ffi` 无 `publish = false`、manifest 完整；其拓扑序位次见下文轮 20 P7）。所以它是 1.0.2 时代**漏发**，不是设计上的非公开面。把门禁改成「按已发布的子集判」等于把一个漏发事故固化成长期承诺，且会让未来每个漏发都自动变绿。改为：门禁原样保留 + README 说清 15≠16 的原因 + 1.1.0 发布时带上它。 |
| P3 | `publish-crates.mjs` 的成功横幅自己说谎 | 同一份输出里先打「crate 共 16 个；产物校验失败 0 个」，下一行是 `✓ --check 通过：**15** 个 crate 均可离线产出…`——计数是硬编码字面量，crate 从 15 涨到 16 时它就永远少报一个。改成插值 `order.length`。红灯实录（修复前的真跑）与绿灯实录（修复后 `pnpm publish:crates -- --check` → `✓ --check 通过：16 个 crate…`，rc=0）都在本轮日志里。 |
| P4 | 门禁失败了却不告诉操作者怎么办 | 原实现只逐条打 `✗ … is unavailable (HTTP 404)` 后 `exit 1`，要理解「为什么红、下一步跑什么」必须去读 workflow。补一段汇总：分类计数 + 缺失清单 + 补齐动作（`Publish SDK packages` 手动作业，需 `NPM_TOKEN` / `CARGO_REGISTRY_TOKEN`，之后重跑失败作业）。**判定与退出码一字未改**，改的只是可诊断性。 |

**本轮门禁复跑（本轮日志）**：`pnpm publish:crates -- --check` 修复前后各一次真跑（修复前
横幅报 15、修复后报 16，两次都是 16 个 crate 打包与 tarball 构建全过、rc=0）；
`node scripts/check-published-versions.mjs 1.0.2` → rc=1 且新汇总正确分类（`缺 crates.io：
tauron-ffi`，npm 0 个）；`… 1.1.0` → rc=1、36 项分类为 npm 20 / crates.io 16。

**登记为发布前置口径（不在本轮执行）**：`v1.1.0` tag 已在 `6388953` 上，registry 发布一旦
完成，在 Actions 里从失败处重跑即可补出 GitHub Release；**删 tag 重打是不必要的破坏性动作**。

#### 轮 20 复查轮 19 的发布链路改动（2026-10-03）

轮 19 的两处改动都只由一次真跑验证过，本轮把它们当别人的代码重读一遍，查出**两个真缺陷**
——其中一个是我自己在轮 19 引入的。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| P5 | `registry-check` 把分支名当版本号 | `release.yml` 的 `registry-check` 用 `node scripts/check-published-versions.mjs "${GITHUB_REF_NAME#v}"`，而它的 `if` 第二条允许 `workflow_dispatch` + `dry_run=false` 的**分支**运行进入；那时 `GITHUB_REF_NAME` 是 `main`，剥 `v` 之后仍是 `main`，等于对 registry 查 `@main`。同文件的 `version-check` 早就有 `GITHUB_REF_TYPE == tag` 的分支回落，这扇门没有——**同一份版本号语义在一条 workflow 里两处不同源**是根因。红灯实录（本轮真跑 `… main`）：rc=1、36 项全缺。处置：改成与 `version-check` 同源的 `if`，非 tag 运行不传参、由脚本回落到 `package.json` 的 version，把这条路径如实变成「发布前预检当前版本」。 |
| P6 | 轮 19 新加的汇总自己说谎 | 归类靠事后正则 `/package (\S+)@\d/`——要求 `@` 后紧跟数字。版本是分支名时消息是 `…@main`，`@` 后没有数字；npm 那条「resolved unexpected version」干脆没有 `@`。两种情况都归不了类，于是汇总打成「**共 36 项**在 main 上不可用：**npm 0 个、crates.io 0 个**」，与它自己上面的 36 直接矛盾。**根因不是正则写得糙，是把身份塞进消息文本、再靠文本往回找**：超时与网络异常抛的是 `fetch failed` / `AbortError` 这类别人的错误，压根不带包名，任何正则都归类不了。处置：身份由 `checks` 每项自带（`registry` / `bucket` / `name`），「确实没发布」用 `NotPublished` 标记类抛出，✗ 行统一格式化为 `✗ <registry> <name>@<version>: <原因>`，汇总拆成「缺 N 个」与「未能核实 M 个」两栏并各自列出包名。**判定与退出码一字未改**。绿灯实录（本轮）：`… 1.0.2` → 「npm 缺 0 个、crates.io 缺 1 个」+ `✗ crates.io tauron-ffi@1.0.2: is unavailable (HTTP 404).`；`… 1.1.0` → 36 项全归类（npm 缺 20、crates.io 缺 16）；无参 → 按 1.1.0 核验，证明分支回落取到的是版本号而不是分支名。同轮实录到分类漂移：`1.0.2` 连跑三次，tauron-ffi 两次记「未能核实」（一次超时、一次 `fetch failed`）、一次记「缺」——旧写法只会说「有 1 项未能核实」而不说是哪项，新写法直接点名。 |
| P7 | 「拓扑序第 15 位」是可复现的，但不是代码常量 | 轮 19 P2 里 `tauron-ffi` 的位次取自当时那份 `--check` 日志，而 `order` 由 `cargo metadata` 的成员顺序 + 内部依赖图算出，不是写死的序号。本轮复跑 `pnpm publish:crates -- --check`（rc=0）：**第 15 位、依赖 `tauron-host`**，与轮 19 的观测一致，因此那条说法保留；同时把「无 `publish = false`、在发布集合内」这类**代码可判定**的表述作为主证据，位次只作旁证。同一份日志再确认修复后的横幅是 `✓ --check 通过：16 个 crate…`，与上一行「crate 共 16 个」一致。 |

**本轮门禁复跑（本轮日志）**：`pnpm publish:crates -- --check` rc=0，日志同时给出
「crate 共 16 个；产物校验失败 0 个」与 `✓ --check 通过：16 个 crate…`（轮 19 P3 的修复
仍在位），发布顺序段里 `tauron-ffi` 为第 15 位、依赖 `tauron-host`；
`node scripts/check-published-versions.mjs`（无参）→ rc=1、「共 36 项在 **1.1.0** 上未通过核验：
npm 缺 20 个、crates.io 缺 16 个」——这就是 P5 的绿灯：分支运行回落到的确实是版本号而不是
分支名；`… main` → rc=1、同样 36 项但每个 ✗ 行都写成 `…@main`，即 P5 的红灯形态；
`… 1.0.2` → rc=1、「npm 缺 0 个、crates.io 缺 1 个」+ `tauron-ffi` 点名（与轮 19 同判，
改归类没有改变任何判定）。五个文本门禁 `pnpm format:check` / `pnpm lint` /
`pnpm docs:check`（6 个文档 235 条引用 OK）/ `pnpm command-surface:check`（85 commands，孤儿 0）/
`pnpm version:check`（26 处 1.1.0）逐一以 `> file 2>&1; echo rc=$?` 实读为 rc=0。
> 轮 19 记录 rc 时用的是 `pnpm … | tail -5; echo rc=$?`——那读到的是 `tail` 的状态而非工具的
> 状态。本轮改为 `> file 2>&1; echo rc=$?` 逐个实读，五项全 0 才是这轮的证据。

#### 轮 21 发布链门禁精确化（2026-10-03）

轮 20 把 `registry-check` 那一段修完之后，本轮顺着同一条 `release-evidence` 作业往下读，
撞上第三类同一个根因的问题：**判定用的是聚合数字，而不是它声称要证的性质**。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| P8 | 产物腿门禁数得出「有几个文件」，数不出「有几条腿」 | `check-release-artifact-sizes.mjs` 用硬编码 `files.length < 4` 代表「四条 release 腿都交了产物」。腿身份在 `release.yml` 下载处是保住的（`pattern: bundle-*` + `merge-multiple: false` → `release-bundles/bundle-<label>/…`），脚本自己把它摊平成了计数。**红灯实录**（`git show HEAD:` 取出改前版本，跑同一 fixture：3 腿 / 4 文件，macOS x64 缺席、Windows 交两份）→ **rc=0**，且指标行就写着 `RELEASE_ARTIFACT_SIZE_METRICS files=4 aggregateBytes=4`：整条腿没了，门禁与指标双双好看。这与轮 19 P3（横幅硬编码 15）、P6（靠正则找回身份）是同一族——**聚合数字不能代替逐项在场**。 |
| P8-处置 | 期望集合改为台账同源 + 逐腿在场 | 期望腿从 `contracts/target-matrix.json` 的 `releaseArtifact` 行推出（本轮实测 4 行：linux-x64 / windows-x64 / macos-arm64 / macos-x64），每条腿必须至少贡献一个白名单扩展名的产物，缺席就点名并列出实际到场的腿；出现台账未声明的腿同样判红。**体积预算与阈值一字未改**，改的是判定的粒度。顺带删掉 `\|\| path.endsWith('.AppImage')` 这条冗余条件（`.AppImage` 已在 `allowed` 集合内，`extname` 天然收它）——孤儿条件也是孤儿逻辑。report 新增 `expectedLegs` / `legs`，让证据自己带上传说与实况。 |
| P8-边界 | 摊平会不会静默放行 | 实测把 fixture 摊平（模拟 `merge-multiple: true`）→ rc=1：每个文件名被当成一条未声明的腿，同时四条腿全部判缺。**破了也是红**，不需要再加一道防。这条隐藏约束写进了脚本注释。 |
| P9 | 轮 20 自己漏跑了一个门禁 | 轮 20 编辑了 `release.yml` 却没跑 `check-target-matrix.mjs`，而它正是用正则在 workflow **文本**里抓 `- label:` / `platform:` 配对的（`{0,180}` 距离窗口），最容易被这类编辑碰坏。本轮补跑 → rc=0，`Target Matrix OK: linux-x64@ubuntu-22.04, windows-x64@windows-latest, macos-arm64@macos-15, macos-x64@macos-15-intel`。登记的口径：**改 `release.yml` 的轮次必须跑 target-matrix 门**，光跑 prettier 不算数。 |
| P10 | 姊妹门是否有同类缺陷 | `verify-registry-consumer.mjs`（版本从 `Cargo.toml` + `package.json` 双读并要求一致，不一致直接抛）与 `check-registry-ownership.mjs`（枚举 `packages/`、`crates/`）都从文件系统取集合，未发现硬编码计数；两者都过滤 `private === true`，本仓唯一的 private 包是 `@tauron/contract-tests`。 |

**本轮门禁复跑（本轮日志）**：改后脚本四个 fixture 逐一实跑——4 腿 → rc=0（`files=4`）；
3 腿 → rc=1 且点名 `bundle-macos-x64`（report `legs` 为
`{"bundle-linux-x64":1,"bundle-macos-arm64":1,"bundle-windows-x64":2}`）；多出
`bundle-windows-arm64` → rc=1；摊平 → rc=1。改前版本对 3 腿 fixture 给 rc=0（上面 P8 的红灯）。
`node scripts/check-target-matrix.mjs --check` → rc=0。文本门禁 `format:check` / `lint` /
`docs:check`（6 个文档 235 条引用 OK）/ `command-surface:check` / `version:check` 五项
`> file 2>&1; echo rc=$?` 实读 rc=0。

#### 轮 22 V7 全量落地：写入侧收口 + 孤儿面台账（2026-10-04）

V7 的三轮复查把镜头从「发布链的判定」挪回**主体流程本身**：写路径会不会被对端钉死、
有没有查不到消费者却对外宣称可用的公共 API、前后端形状对不对得上。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| Q1 | sidecar stdin 写路径是**无界阻塞** | 原实现「两级锁 + 命令线程里 `write_all`」：一个不读 stdin 的 sidecar 能把宿主命令线程永久钉住。改为**有界队列（`WRITE_QUEUE_FRAMES = 32`，最坏 32 MiB）+ 每 pid 专属写线程**，`write_frame` 只做 `try_send`，四种结局（`NotFound`/`InvalidInput`/`WouldBlock`/`BrokenPipe`）全部如实上抛，投递失败绝不被记成成功（`crates/tauron-proc/src/spawner.rs`）。**轮 30 补正**：本行原句按"上抛"读过就通过了——`tauron-proc` 那一层确实保住了四种 `io::ErrorKind`，但**适配层把它们又塌成一个码**（`E_STATE_INVALID_TRANSITION`），所以"如实"止步于跨层边界之前，负载下才露馅。轮 30 把分流补到调用方看得见的地方（`process_delivery.rs` 的 `stdin_write_failure`：容量→`E_CALL_PENDING_FULL`、租约→`E_LEASE_EXPIRED`、帧不合协议→`E_INVALID_MANIFEST`、没有写通路→`delivered: false` 而非错误），并配了负载无关的注入用例。详见本文件末尾的轮 30 小节 |
| Q2 | 终止后用阻塞 `Child::wait()` 收尸 | 改为 `reap_bounded` + `KILL_REAP_TIMEOUT_MS = 2s` 的 `try_wait` 轮询；上界内取不到结局时**保留句柄并如实报告**，不当成已回收（同文件） |
| Q3 | 「孤儿逻辑」此前只是散文描述 | 新建 `contracts/orphan-public-api.json` + `scripts/check-orphan-public-api.mjs`：12 条未接线宣称必须在产品接线面查不到消费者、5 条已接线反例必须查得到、自动发现数不得高于基线。**判据本身翻过一次车**——首版把 Rust 源文件在第一个 `#[cfg(test)]` 处截断，抹掉了它之后的生产代码，把真在安装链里调用的 `draft_grant_set` / `validate_grants` 判成孤儿；改为大括号配对删除测试模块，并把这两个符号连同 `validate_zip_constants` 一起钉成反例，让「检查器自己坏掉」也只能变红 |
| Q4 | 框架层被宣传成「接上就能给已有 Tauri 应用加插件系统」 | 实况是仓内**没有任何宿主注册 `plugin_*` 三条命令**，`PluginDispatcher` 的生产 impl 为 0 ⇒ 恒 `SC-9001`。README 场景表改成双向回答、快速上手段去掉「零 capability 配置」的假承诺、`incremental-adoption` 附录 A 加警告块、overview 接线表补 `tauron-shell` 的未接线行，并把台账 `overviewWiring` 置为 `unwired`——让这行被门禁**要求**而不是被容忍 |
| Q5 | `runtime-wasm-broker` feature 从未被**执行** | 此前只有 `--all-features` 的编译检查覆盖它（adapter `default = []`，示例工程又被 workspace 排除）。CI 补两条真跑测试的步骤（`runtime-wasm-broker`、`tauri + runtime-wasm-broker`），`verify-source-ci.mjs` 把它们列为必需步骤，配对的 `cfg` 测试保证关掉 feature 时诚实失败 |
| Q6 | 重构方案里「两级锁就是修复完成」的叙述与实现对不上 | `full-architecture-refactor-plan.md` 的 W7 / P0-7 三处更正为「两级锁是中间态，已被有界队列 + 专属写线程 + `reap_bounded` 取代」，`spawner.rs` 的测试文档注释同批改口径 |

#### 轮 23 第三轮：恢复 / 升级 / 发布链路的连通性复查（2026-10-04）

本轮不找新特性，只沿「前端调用 → 线格式 → 宿主 → 状态机 → 落盘」把核心链路逐条走通，
并回头复查门禁自身是不是把**排版**当成了**语义**——结果这类缺陷一次找出两处。

> **三遍的口径（避免读者按编号误数）**：要求是「至少三遍，每遍修完才开下一遍」。
> 第 1 遍 = 轮 22 前半（V7 逐条落地：WASM provider、真 sidecar E2E、装配冲突硬错、
> 发布面判定）；第 2 遍 = 轮 22 后半（写入侧回扫：无界阻塞的 `write_all`、阻塞收尸、
> 孤儿公共 API 整类）；第 3 遍 = 本轮（连通性与门禁形状：R1–R6）。三遍各自的修复都在
> 同一轮的日志里复算过，不留「下一轮再补」的欠账。

| 编号 | 发现 | 判定与处置 |
| --- | --- | --- |
| R1 | wire-gate 把排版当语义（第一处） | `autoDownload` 后台下载吸收 rejection 的断言写成 `downloadUpdate()` 后 12 字符内必须出现 `.catch(`，prettier 把 `.catch(` 换行缩进 14 格后**必然假红**（实测 147 项里 1 红）。改为语句内锚定 `(?:(?!;)[\s\S])*`，并用三个变异自证非空转：现状 → 命中、删掉 `.catch` → 红、裸调用 → 红 |
| R2 | wire-gate 把排版当语义（第二处，同类但更危险） | §8-20b 的 ClientConfig 落点表解析正则要求 `(` 与键名相邻，而 rustfmt 会把长条目折成 `(` / `"data_dir",` / `ClientConfigLanding::Consumed(` 三行——**折一行就静默少一个键**。红灯实录：`tauron-host` 3 failed｜404 passed，报错形状是「10 个结构体字段 vs 2 个表内键」，只剩两条未折行的键被认出来。修法：正则去掉 `(`，并把扫描范围**截在表体 `];`**（表后面的实现与测试模块也写 `ClientConfigLanding::Deferred(_)`，整文件扫会把它们当条目数进去）。修后 407 passed (407) |
| R3 | `GenerationRegistry::active` 看着像只增不减的泄漏 | **不是**泄漏，是防句柄复用的代数高点：卸载后清理它会让重装同名插件回到 `INITIAL`，旧句柄重新通过 `validate`。该性质已由 `runtime_generation_advances_and_old_handle_becomes_stale` 钉住；`leases` 侧确认只有 `register`/`remove_plugin` 成对铸销（全文件仅两处 `remove`）。落的是 WHY 注释，不是代码 |
| R4 | Windows reference `serve_one` 是编译进库的**无界等待** | `ConnectNamedPipe(…, null)` / `ReadFile(…, null)` 同步无超时，而 Unix 侧 `serve_one` 有 `REFERENCE_EXCHANGE_TIMEOUT` 20s 交换上界。overlapped 改造属 Batch 5'，本轮把它从「代码注释里的自认」升格为 A86 行的对外判定，并写明调用方契约：专用线程 + `recv_timeout` 收结局，不得无限 `join` |
| R5 | 升级/恢复两段前后端形状需逐字段核 | TS `UpdaterCheckOutcome`（`available/version/url/releasedAt/degraded/reason`）与 `UpdaterStatus`（`available/state/grayscalePercent/crashGateStopped/reason`）对 Rust `deny_unknown_fields` 的 camelCase 结构**逐一对齐**；`downloadUpdate`/`installUpdate` 对 `simulated` 与 `ok: false` 一律 `throw`（不假装完成）；恢复引擎唯一驱动入口 `recoverReport` 有生产消费者（`examples/minimal-app/src/main.ts` 的 `.recoverReport('success')`），`recoverBoot` 保持只读定位 |
| R6 | 轮 22 新增的 `publish = false` 夹具把发布链踩穿 | `publish-crates.mjs` 的发布集合从 `cargo metadata` 的 `crates/` 成员**直接枚举**，不看 `publish` 字段：`tauron-test-sidecar` 因此被排进发布序，`--publish` 会在拓扑序中途被 cargo 拒绝——那时前面的 crate **已经发出去、收不回来**。同族第二处：`check-registry-ownership.mjs` 用 `readdirSync(crates/)` 枚举，等于索要一个永不上架名字的 crates.io 写权限。两处都改为「切开集合 + 点名排除」，`publish-crates.mjs` 另加一条 fail-closed：可发布 crate 的依赖若指向 `publish = false` 成员，预检阶段直接报错（发布产物无法解析该依赖）。**判据形状本轮又被实测纠正一次**：第一版按文档写 `publish === false`，跑起来仍是 17 个——`cargo metadata` 把 `publish = false` 表达成**空数组**（默认成员是 `null`），两种形状现在都认。修后 `--check` 出 `crate 共 16 个` 并点名排除 `tauron-test-sidecar@1.1.0`。静默变小是这一族的老毛病（轮 19 P3、轮 21 P8 同源） |

**轮 23 门禁复跑（本轮日志）**：`cargo fmt --all -- --check` rc=0、
`cargo clippy --workspace --all-targets --locked -- -D warnings` rc=0、
`cargo test -p tauron-host --locked` rc=0（主套件 405 passed，另有 1/1/7/4/1 的集成套件全 0 failed）；
`pnpm -F @tauron/host test` rc=0（22 文件 407 tests passed，含 R2 修好的 §8-20b 三条）、
`pnpm -F @tauron/contract-tests test` rc=0（2 文件 168 tests passed）、
`pnpm gates:check` rc=0（Upgrade-File-Hash 1 文件 / Success-Requires-Effect 8 文件 /
Simulated-Never-Commits 6 文件 / 孤儿台账 12 条未接线 + 5 条反例 + 自动发现 635 ≤ 基线 635）、
`pnpm docs:check` rc=0（7 个文档 234 条引用）、`pnpm command-surface:check` rc=0
（85 命令：底座 61 / 运行时 22 / 安装 2，孤儿命令 0，未归类 0，无代码层判定 9）、
`pnpm version:check` rc=0（26 处 1.1.0）、`format:check` / `lint` rc=0。
发布面另两项同轮实跑：`cargo test --workspace --locked` **rc=0**（41 个 `test result: ok`，
0 failed——日志里唯一命中 `panicked` 的行是测试名 `non_array_unset_list_is_ignored_not_panicked`），
`pnpm verify`（build → typecheck → test 全仓）**rc=0**（20 个包的测试文件全绿；两条 stderr
是负例测试自己的输出：`Token mismatch: bridge handshake failed`、`beforeExit hook failed`），
`node scripts/publish-crates.mjs --check` **rc=0**（修好 R6 后 `crate 共 16 个；产物校验失败 0 个`，
并点名排除 `tauron-test-sidecar@1.1.0`；修前同一命令给的是 17 个且没有排除行）。
前后端注册面本轮另核对一条方向：`wire-gate` 的 `HostClient 调用的每个命令都在 Rust 注册表中`
把 TS 侧引用的每个 `host_*` 都对 `tauron_plugin_handler!` 的展开集合求交（危险方向是
「前端调用一条没注册的命令」，拿到的是 Tauri 的 command-not-found），该断言随
contract-tests 168 项一起绿。示例宿主侧的注册落点是 `tauron_generate_handler![]` 宏别名，
不是手抄清单——按 `generate_handler` 文本比对会得出「只注册 4 条」的假断链，本轮踩过一次。

**轮 23 安装包重建（本轮日志，与上面同一批）**：先 `pnpm -r build` rc=0（前端产物
`dist/assets/main-C948Pxbw.js` 86.74 kB／gzip 24.61 kB），再分两次 `tauri build`——
`--bundles nsis` rc=0（`Finished release profile [release] in 1m 23s`、`Finished 1 bundle`），
`--bundles msi` rc=0。两个产物都是本轮新建：**NSIS `Tauron Minimal App_1.1.0_x64-setup.exe`
3,574,475 B、MSI `Tauron Minimal App_1.1.0_x64_en-US.msi` 5,431,296 B**（时间戳同日 07:49／07:50）。
重建 MSI 不是走过场：本轮动手前 `bundle/msi/` 里躺着的是**轮 18 的 5,410,816 B**，文件名却已经
是 1.1.0——按文件名取件会把两轮前的构建当成当前源码发出去。这类「同名旧产物」是发布链上最难
被发现的一类失真，因此上表轮 18 那行的数字只对应轮 18，当前口径以本段为准。
如实登记两条边界：两个安装包**都未签名**（本机无代码签名证书），且**未在本机做安装-卸载验证**；
`bundle.targets` 本轮没有走 `"all"` 单次全量，而是 `nsis`／`msi` 分跑，以免一个 WiX 失败把另一个
已成功的产物掩掉。

#### 轮 24 V7 §10-9 的「至少冻结」分支：adapter 域所有权台账（2026-10-04）

V7 §10 第 9 条给的是**二选一**：把 adapter 按域拆开，**或**至少冻结 domain ownership，
避免继续向单文件追加核心逻辑。物理拆分是 Batch 5'（动的是已发布命令面的落点，风险要单独一轮），
本轮落的是后者——而且落成了机器判定，不是口头约定。

| 编号 | 落地 | 判据与非空转证明 |
| --- | --- | --- |
| S1 | `contracts/adapter-domain-ownership.json` + `scripts/check-adapter-domain-ownership.mjs`（pnpm `gate:domain-ownership`、`gates:check`、CI 两步） | 台账登记**两类形状**：顶层 `impl`（键=目标类型，数量=块体第一层的 `fn` 数）与顶层自由 `fn`（键=函数名首段，数量=函数个数），各带 `owners` 文件集合与数量高点。规则 R1 未登记即红（impl 头解析失败也红，不静默跳过）、R2 一键一域、R3 归属文件集合**必须相等**、R4 数量只减不增、R5 每个文件的顶层条目总数封顶 |
| S2 | 计数单位的选择（吸取轮 23 的教训） | 门禁数的是**声明级条目**，不是行数：一次 rustfmt 重排就能让行数基线假红或把真实增长掩掉，轮 22/23 已经实测过两次「把排版当语义」。`--self-test` 里专门有一条**折行 `impl` 头**夹具（`impl<T: Clone + Default>` 换行 + 裸类型名 + `{` 独行），钉住「排版不得改变判定」；同批夹具还各打一条 R1/R4 的红/绿形状 |
| S3 | 落地过程中自己踩到并当场封掉的两个空转 | ① 只数 `impl` 的初版几乎无价值：实测本仓主体形状是**顶层自由 `fn`**（`lib.rs` 顶层 153 个 `pub fn` + 75 个 `fn`，其中 `cmd_*` 136 个），不登记它们就等于对「往单文件追加逻辑」免检；台账因此加了 `fn` 类条目。② 自证夹具第一版按**单条目**判数量高点，137 个 `cmd_*` 被看成 137 条各自合格的条目 ⇒ R4 对该形状空转；改为按 `kind:key` **聚合后**判定，与 `check()` 同构 |
| S4 | 反向验证（三条都是同轮实录） | 台账里 `lib.rs` 预算调低 1 ⇒ 红（`顶层条目 263 > 基线 262`）；往 `lib.rs` 追加 `pub fn zzz_domain_probe_new_group() {}` ⇒ 同时报出 R5（264 > 263）与 R1（`fn zzz` 未登记域归属），随后按 md5 还原源码逐字节一致；从台账抽掉全部 `fn` 条目 ⇒ `wire-gate` 新增断言红（`台账缺少顶层 fn 类条目（cmd_* 就是这一类）`）。三条红完都还原复绿 |

**当前基线（本轮实测）**：`Adapter-Domain-Ownership gate OK across 5 source files
（35 个 impl 类型 / 170 个方法、83 组顶层 fn / 357 个函数、18 个域、lib.rs 顶层条目 263 已封顶；
数量全部 ≤ 基线）`。`lib.rs` 17,596 行这件事本身没被"修小"——本轮把方向钉住：**新增核心逻辑
要么开新域/独立文件，要么在 PR 里显式改台账**。台账漂移（登记的键/文件已不存在）同样判红。

**同轮的文档面副作用（如实记）**：往 `ci.yml` 插两步把 A66 行的两条 `ci.yml:NNN` 引用推到了
空白行上，`docs:check` 立刻变红。按本文既一口径改成**步骤名锚定**后 `docs:check` rc=0
（7 个文档 **232 条引用**——比轮 23 的 234 少 2，正是那两条换成了符号锚定的行号引用；
此后往 CI 增删步骤不会再漂这两条）。

**轮 24 门禁复跑（本轮日志，Rust 侧零改动）**：`pnpm gates:check` rc=0（七条门禁 + 自证：
Runtime-Assembly-Singleton 5 文件 / Wasm-Loaded-Generation 4 / Upgrade-File-Hash 1 /
Success-Requires-Effect 8 / Simulated-Never-Commits 6 / 孤儿台账 12+5+635 ≤ 635 /
Adapter-Domain-Ownership + self-test）、`pnpm -F @tauron/contract-tests test` rc=0
（2 文件 **168 tests**，其中 `wire-gate.test.ts` **147**）、`pnpm lint`（`--max-warnings 0`）rc=0、
`pnpm format:check` rc=0、`pnpm docs:check` rc=0、`pnpm command-surface:check` rc=0（85 命令、
孤儿 0、未归类 0）、`pnpm version:check` rc=0（26 处 1.1.0）、public-surface ledger rc=0
（85 public commands 无孤儿元数据）、`pnpm verify` **rc=0**（20 个包的 test 段全部 Done）。
`crates/tauron-adapter/src/lib.rs` 在变异验证后按 md5 逐字节还原（`ffba81c9…` → 同一值），
因此轮 23 的 cargo 全绿结论对本轮仍然成立——本轮没有把任何 Rust 文件改回去。

#### 轮 25 V7 §9 leak gate：abandoned generation 从「注释里有上界」变成「代码有上界 + 线有读数」（2026-10-04）

**缺陷形态**：`GenerationRegistry::active` 是**只进不出**的 `HashMap<String, Generation>`，
每个曾经起过运行期的插件 id 都在这里留一条永久条目。V7 §9 的 leak gate 写的是
「abandoned generation 必须有界 metric，不允许无限增长」，轮 22/23 只证明了 `leases` 成对
（铸造/释放一比一），`active` 的上界**只写在注释里**——没有代码钉、没有读数可查、没有测试跑
一遍 churn。按本仓「已落地 = 真代码 + 真消费者 + 门禁」的口径，这一行当时并未成立。

**修法（四件，缺一不可）**：

| # | 落点 | 内容 |
| --- | --- | --- |
| S1 | `generation.rs` | 号源改为**全局单调计数器**（`high_water`），不再按资源各自 `next()`。这样"摘除跟踪"不会让重装/重启动的同名资源退回旧号——防句柄复用的判据从"高点记得多久"升级为"号永不复用"，比原状更强 |
| S2 | `generation.rs::forget` + `runtime.rs::remove_plugin` | 回收租约时**先 `release` 再 `forget`**；资源仍有活跃租约时 `forget` 拒绝（不硬摘），因为摘掉在管资源的高点会让 `validate` 只剩 token 一道判据。被摘除资源的旧句柄走 `NotActive`，**fail-closed**，不会被放回场 |
| S3 | `host_resource_stats.global.generations` | 有界读数 `GenerationStats{trackedResources, liveLeases, generationsIssued}` 一路走：台账 `stats()` → `RuntimeTable::generation_stats()` → `Registry::runtime_generation_stats()` → 命令面 → TS 镜像 `GenerationStats`。「不允许无限增长」必须**查得到**，否则只是测试里的断言 |
| S4 | Rust 测试三层 + 一条复用证明 | 台账级 10,000 个 id churn（`tracked_resources_stay_bounded_under_churn_of_distinct_ids`）、租约表级 5,000 次起停（`generation_tracking_stays_bounded_under_plugin_churn`）、注册表真实安装→起租约→回收 200 轮（`generation_tracked_resources_do_not_accumulate_across_plugin_churn`）、句柄复用（`abandoned_generation_handle_stays_rejected_after_forget_and_reactivation`），外加 adapter 的 `resource_stats_report_live_usage_and_are_main_window_only` 把读数上线核对成活值（起租约 ⇒ 1，回收 ⇒ 0） |

**门禁（§9 的语义门禁自己也要有链路证明）**：`wire-gate.test.ts` 新增
「V7 §9 leak gate：abandoned generation 必须有界，且读数跨 IPC 可查」，钉四件事——号源是全局
计数器、`forget` 的拒绝分支仍在、回收体内 `release` 出现在 `forget` **之前**（按函数体提取后
比较下标，不比行号）、读数五段链路（台账→表→注册表→命令面→TS）与四条 churn 测试名都在。
P0-2 的 TS↔Rust 逐字段同构清单加上 `GenerationStats`（helper 多读一个源文件
`crates/tauron-host/src/generation.rs`）。

**变异证明（门禁不是空转）**：见本节末「轮 25 门禁复跑」——把 `remove_plugin` 里的
`forget` 摘掉、把 `activate` 改回每资源序号，各自都要有一条红。

**文档副作用**：`runtime.rs` / `registry.rs` / `lib.rs` 的行号整体上移，
`pnpm docs:check` 报 5 条漂移——4 条由 `--fix` 重锚（唯一最近解），
A90 行残留的那条「`runtime.rs` 第 223 行」式引用按本轮既定口径改成**符号锚定**
（`RuntimeTable::register` 调用点），不再留一个会漂的数字。A88/A90 行与 §5 DoD 的
`upgrade generation separation` 同步改写。

**诚实边界（不粉饰）**：
1. 注册表条目在本版本**永不移除**（无 uninstall API），所以有界的对象是**运行期代际台账**，
   不是"插件条目集合"——后者的累积属 A83/A84 的既有限制，本轮不动；
2. `generations_issued` 刻意只增不减：它是一个 `u64`，不随 churn 增长内存，却是"号不复用"的
   唯一证据；号源耗尽需 2^64 次激活，用 `saturating_add` 而非回绕，真到那一步仍由 token 匹配
   拒绝旧句柄；
3. `ProviderLifecycle` / 插件激活侧依旧不校验代际（A88 缺口列原样保留）。

**轮 25 门禁复跑（本轮日志，含三次变异的同轮实录）**：正向全绿——`cargo fmt --all --check` OK、
`cargo clippy --workspace --all-targets --locked -- -D warnings` rc=0、
`cargo test --workspace --locked` rc=0（41 个 result 块合计 **1,528 passed / 0 failed**，
其中 `tauron-host` lib **409**、`tauron-adapter` lib **277**）；`pnpm format:check` **第一次 rc=1**
——本轮新加的 `wire-gate` 断言没过 prettier，`pnpm format` 写后复跑 rc=0（自己踩的红，实录）；
`pnpm lint`（`--max-warnings 0`）rc=0、`pnpm docs:check` rc=0（**231 条**行号/符号引用全部可核对，
比轮 24 的 232 少 1 条，正是 A90 那条改符号锚定的结果）、`pnpm command-surface:check` rc=0、
`pnpm version:check` rc=0、`pnpm gates:check` rc=0（七条门禁 + 自证：孤儿台账自动发现
**634 ≤ 基线 635**、域所有权 `35 个 impl 类型 / 170 个方法、83 组顶层 fn / 357 个函数、18 个域、
lib.rs 顶层条目 263 已封顶` + 6 个变异形状自证）、契约测试 2 文件 **169 passed**
（`wire-gate.test.ts` **148** + `contract.test.ts` **21**）、`pnpm verify` rc=0（64 个包任务段 Done）。

三次变异都在本轮实跑，且**每次都按 md5 逐字节还原后复验**：

| 变异 | 结果 | 还原 |
| --- | --- | --- |
| M1 删掉 `remove_plugin` 里的 `self.generations.forget(plugin_id);` | `tauron-host` lib **407 passed / 2 failed**（`runtime::tests::generation_tracking_stays_bounded_under_plugin_churn`、`registry::tests::generation_tracked_resources_do_not_accumulate_across_plugin_churn`）＋ adapter 的 `resource_stats_report_live_usage_and_are_main_window_only` **1 failed**（回收后读数回不到 0）＋ leak gate **1 failed** | `runtime.rs` md5 `55d8106f…` 前后一致 |
| M2 把 `activate` 改回**每资源序号**（轮 23 及之前的写法） | `tauron-host` lib **403 passed / 6 failed**：4 条本轮新测试 + **2 条既有代际测试**（`runtime_generation_advances_and_old_handle_becomes_stale`、`versioned_runtime_lookup_rejects_old_generation`）＋ leak gate **1 failed** | `generation.rs` md5 `dab590a1…` 前后一致 |
| M3 删掉命令面那行 `"generations": generations` | adapter `resource_stats…` **1 failed**（`["global"]["generations"]` 那条断言）＋ leak gate **1 failed**，消息点名 `host_resource_stats 未带上代际读数` | adapter `lib.rs` md5 `c890fd1f…` 前后一致 |

M2 的失败集合是本轮最有价值的产出：**既有测试一起红**说明「摘除跟踪」与「每资源序号」在逻辑上
不可共存——不是本轮挑旧测试的毛病，而是旧写法根本撑不住有界性。因此号源改全局单调计数器不是
可选优化，是 leak gate 的必要条件（对外语义变化已记入 CHANGELOG 的 `### Changed`）。
还原后复验：`tauron-host` lib **409 / 0 failed**、`tauron-adapter` lib **277 / 0 failed**、
`wire-gate.test.ts` **148 passed**、`pnpm format:check` rc=0。

#### 轮 26 V7 §7 Process 行 / §9 leak gate：kill 失败从「只留日志」变成「有人重试 + 查得动的终态证据」（2026-10-04）

**缺陷形态（一条真正的断链）**：`tauron-proc::ProcSpawner::kill` 在终止失败时把子进程句柄
**放回自己的跟踪表**，错误文案写着「句柄已保留，可重试」；而它上面那一层
`RuntimeTable::terminate` 当时只做一件事——`failures += 1` 记下原因，然后 `remove_plugin`
把租约摘掉。**下层留着可重试的句柄，上层没有任何重试方**：那句"可重试"因此是空头承诺，
进程可能仍在跑，宿主却已经把它忘了。V7 §9 leak gate 的原话是
「kill failure 必须有 retry/terminal evidence，不能只留日志」，§7 的 Process 行给的处方是
「kill retry/backoff + startup orphan sweep + evidence」。轮 22–25 只做到"不静默吞"，
`failures` 计数器就是全部答案——按本仓「已落地 = 真代码 + 真消费者 + 门禁」的口径，
这一行当时并不成立。

**修法（四件，缺一不可）**：

| # | 落点 | 内容 |
| --- | --- | --- |
| S1 | `runtime.rs::terminate` → `enqueue_pending` | 失败即入队（插件 id + pid + 原因 + 已试次数）。两条封口同时成立：**同一 pid 只留一条**（否则队列随失败次数增长）、**队满不再排队**而是直接固化证据并计 `overflow`（增长被拒 ≠ 静默丢弃） |
| S2 | `runtime.rs::retry_pending_reaps` | 驱动一次 = 每个待重试 pid 至多再打一次；成功即出队并计 `recovered`，失败累计到 `MAX_REAP_RETRY_ATTEMPTS` 后固化。三重上界：队列 64 / 每条 3 次 / 证据环 16（环形覆盖最旧） |
| S3 | `registry.rs::runtime_retry_pending_reaps` + `cmd_resource_stats` | 生产驱动腿落在**已有**的主窗命令里，与 `gc_expired` 同址（"诊断读 = 回收点"是既定口径）。顺序由门禁钉住：先重试、后取快照。刻意不新开命令——85 条命令面被线门禁冻住，而且"让插件轮询着驱动宿主杀进程"本来就不该存在 |
| S4 | 读数上线 + 测试六条 | `ReapStats` 重试腿（`pending`/`retries`/`recovered`/`terminal`/`overflow`/`terminalRecords`）随轮 25 的 `generations` 一起挂 `host_resource_stats.global`；TS 镜像 `ReapStats` / `TerminalReapRecord` 进 P0-2 逐字段同构清单。测试：租约表级 4 条（恢复 / 固化 / 风暴上界 / 去重）+ 注册表卸载路径 1 条（真走 `admin_op(Uninstall)`）+ adapter 命令侧 1 条（读命令把重试打出去、把证据读回来） |

**门禁**：`wire-gate.test.ts` 新增「V7 §7/§9：kill 失败必须有 retry 或终态证据，两者都有
上界，且重试不得误杀同号活进程」（轮 27 把防误杀封口并进同一条检查的第 ⑤ 段），钉链条而不是
结论——进程侧那句「可重试」仍在、`terminate` 失败必入队、
三个上界常量都在、`reap_stats` 合成 `pending` 与 `terminalRecords`、注册表驱动出口存在、
`cmd_resource_stats` 体内"先重试后读数"的顺序、命令面带 `"reap": reap`、TS 字段齐全，
外加六条测试名与两条文档口径。P0-2 同构清单加 `TerminalReapRecord`。

**文档同步（同一轮，不留反向宣称）**：`app-layer-wire.md` 的回收段补上重试腿与三重上界；
`plugin-development-guide.md` 里「正在退出的宿主没有重试方」这句**已被本轮推翻**，改写成
"运行期有人重试、到顶留证据；宿主自身退出与跨重启扫描仍未接"。

**诚实边界（不粉饰）**：
1. **重试需要驱动方活着**：队列只在宿主继续运行且有人读诊断时推进。驱动腿的可达出口是
   SDK 已发布方法 `ShellClient.resourceStats()`（主窗专属命令），**仓内没有 UI 面板在轮询它**
   ——没有调用就没有重试，这是事实而不是待办美化。宿主自己正在退出时，最后一道仍是
   `CommandSpawner::drop` 的 tree-aware `kill`，它失败同样可能留下存活进程；
2. **startup orphan sweep 仍缺席**（V7 §7 处方的第三条腿）：跨宿主重启的 pid 扫描需要一份
   持久化 pid 台账 + 平台侧存活性探测，属 Batch 5' 体量，本轮不做，也不宣称做了；
3. 上界是**有界丢弃**而非无限保管：队列满时新失败直接固化为终态证据并计 `overflow`，
   因此"每个失败 pid 都还在重试"只在队列容量内成立——涨破容量本身就是该看板的信号；
4. `backoff` 取的是**次数上界 + 驱动节奏**（主窗轮询），刻意不引入 sleep 或定时器：
   宿主核心全同步，不在这里长出计时器面。

**轮 26 门禁复跑（本轮日志，含三次变异的同轮实录）**：`cargo fmt --all --check` **第一次 rc=1**
（7 处 diff 全在本轮新写的 `runtime.rs`／`registry.rs`／adapter 代码上——本轮先写实现后格式化，
是自找的红，如实登记）→ `cargo fmt --all` → rc=0。`cargo clippy --workspace --all-targets
--locked -- -D warnings` rc=0。`cargo test --workspace --locked` rc=0：**1,534 passed / 0 failed**
（41 个 result 块；`tauron-host` lib **414**、`tauron-adapter` lib **278**，真 sidecar crate
`tauron-test-sidecar` 在同一轮里被编译并计入）。`pnpm docs:check` **第一次 rc=1**：**37 条**
行号引用漂移，全部落在 adapter `lib.rs` 的锚点上（本轮往同一文件里插了驱动腿调用与活值测试，
行号整体后移）→ `node scripts/check-doc-line-refs.mjs --fix` 重锚 → rc=0（7 个文档、231 条引用）。
`pnpm format:check`／`lint`／`command-surface:check`（85 条：底座 61／运行时 22／安装 2，孤儿 0）／
`version:check`（26 处 1.1.0）／`gates:check`（孤儿面 634 ≤ 基线 635；域台账 35 impl/170 方法、
83 组/357 fn、18 域、lib.rs 顶层 263 封顶，均未长大）／PublicSurfaceLedger（85 条命令无孤儿
元数据）全 rc=0。契约测试 **170 passed**（`wire-gate` **149** + `contract` 21）。`pnpm verify`
rc=0（64 段 Done）。

| 变异（逐字节恢复，md5 现场核对） | 结果 |
| --- | --- |
| M1：删掉 `terminate` 失败分支里的排队调用 | `wire-gate` 红（消息即「terminate 失败后不再排队（kill 失败又只剩日志了）」）；`tauron-host` lib 407 passed / **7 failed**（去重、风暴上界、恢复、终态固化、租约表留痕 + 注册表两条卸载路径） |
| M2：删掉 `cmd_resource_stats` 里那行重试驱动 | `wire-gate` 红（「host_resource_stats 不再驱动重试（断链回来）」——它比的是同一函数体内"先重试、后读数"的顺序）；adapter 活值测试红（诊断读到 `attempts = 1` 而非 2，即"读了但没打"） |
| M3：把命令面返回的 `reap` 键改名 | `wire-gate` 红（「host_resource_stats 未带上回收留痕」）；adapter 活值测试读到 `Null`；TS 侧字段同构清单同时失配 |

M3 的第一次尝试把行号偏移了一位、误改了 `generations` 那行，于是轮 25 的 leak gate 也一起红——
**两条门禁都在盯同一个命令返回体，这正是它们有牙齿的证据**；该文件按 md5 逐字节恢复后复绿。
恢复位点实录：`runtime.rs` 恢复后 md5 `9ab6479929b0ca3d30e3f9d46bdeb09e`、adapter `lib.rs`
恢复后 md5 `b2a413fbc40cbc26527dd86da5320cfc`（两者都是变异前的基线值）；恢复后契约测试
170 passed、`tauron-adapter` lib 278 passed。格式化后的最终 md5 另记：`runtime.rs`
`2e3523ae8b33e4eea92b48d3261606ee`、`registry.rs` `7e119b7152500bbc38e8596e6aaff388`、
adapter `lib.rs` `4baf813e138b921f89fc42675549d713`（格式化只动空白，host 414 / adapter 278
与 clippy 在该状态下复跑仍绿）。

> 下一轮候选（本轮不做，避免把门禁改成"只盯空白"）：`docs:check` 的 37 条漂移说明
> **行号锚**在这一条链上是脆的；把这批 adapter 引用改成符号锚定，才是让"文档指得到代码"
> 变成长期性质的修法。

#### 轮 27 重试的第二枪：pid 同号复用时必须让位，不得误杀活 sidecar（2026-10-04）

**缺陷形态（轮 26 自己引入的）**：重试队列记的是 **pid**，驱动腿拿着它再打一次。而
`tauron-proc` 的子进程跟踪表 `children` 也**按 pid 索引**，登记用的是普通 `insert`
（同号即同键，后写覆盖先写）。于是这条链有一个致命组合：

1. 卸载插件 P ⇒ `kill(100)` 失败 ⇒ 队列里留下 `{P, 100}`；
2. 旧进程随后自行退出，OS 把号 100 重用给新进程；
3. 用户重装 P 并起新 sidecar ⇒ 新进程**也拿到 100**（`register` 时表里没有旧租约，
   所以不会触发"先终止旧 pid"那一步）；
4. 主窗轮询 `host_resource_stats` ⇒ 驱动腿对 100 打第二枪 ⇒ 打中的是 `children[100]`
   里**刚登记的那个活进程**。

"清理孤儿"因此变成"在诊断命令里杀掉正在服务的进程"——比轮 26 要修的那个缺陷更糟，
因为它由修复本身引入。跨平台看：Windows 在句柄未关闭时通常推迟重用 pid，但**没有哪条
契约保证如此**（`reap_bounded` 超时后我们仍持有句柄，而 Linux 的 pid 空间回收后即可重用）；
真正让这条链危险的是第 3 步的覆盖语义，那是代码事实而不是 OS 脾气。

**修法（一条判定 + 一条新语义）**：`RuntimeTable::pid_owner(pid)` 在重试**打下之前**查
在册租约表——只要有任何一条租约（**含已标崩溃的**，宁可让位不可误杀）现在持有这个号，
本轮就不打，改为：出队、`skipped_live_pid += 1`、固化一条 `TerminalReapRecord`
（原因写明"现由在册租约 X 持有：重试会误杀活进程"）。三点措辞是刻意的：

- **让位单独计数**：它既不是 `failures`（没失败，是没打）也不是 `recovered`（没回收）。
  少了这个读数，"我们正确地没误杀"与"驱动根本没跑"在 `reap` 上完全同形；
- **不占重试次数**：`retries` 的语义是"打了几枪"，让位一枪没打，因此判定必须排在
  `self.reap.retries += 1` 之前（门禁按代码顺序钉这一点）；
- **照样留终态证据**：让位是这条待重试记录的**终点**，所以进证据环（`terminal` 计它），
  否则又回到"只在日志里发生过"。

**门禁**：`wire-gate.test.ts` 那条 V7 §7/§9 检查扩到五段，第 ⑤ 段专钉这条安全封口——
`fn pid_owner` 存在、重试体内先判定、让位有计数、判定在 `retries += 1` **之前**、
TS 有 `skippedLivePid`、线上有 `["reap"]["skippedLivePid"]`，并新增三条测试名要求。
测试三条：表级"同号在册 ⇒ 让位且不重复打、活租约原样还在"、表级反向"号不同就必须照旧重试
（否则这条封口会变成'永远不重试'的借口）"、注册表级走真路径
`install → 起租约 → Uninstall → 重装 → 复用同号起新租约 → 驱动`（断言终止器只被打过
首试那一次）。

**文档同步**：`ReapStats`／`TerminalReapRecord` 的 Rust 与 TS 注释、`Registry::runtime_retry_pending_reaps`
的注释、`app-layer-wire.md` 的回收段、`plugin-development-guide.md` 的失败回收说明、
CHANGELOG 与 V7 §7 的 Process 行都补上"让位"这条第三态；V7 那一行原来写的是**无重试方**，
轮 26 之后已经不成立，本轮连同轮 26 的落地一起改写，并把"审计当时的事实"作为前缀保留，
避免把历史句悄悄当成现状宣称。

**诚实边界（不粉饰）**：
1. 判定只看**本表在册租约**的 pid：如果那个号现在属于宿主完全不认识的进程，
   `children` 表里也没有它，终止器会返回 `AlreadyGone`（不误杀，也不谎报成功）；
2. 让位＝**放弃对旧进程的追查**，其前提是"同一个 pid 不可能同时属于两个存活进程"（OS 保证），
   旧进程按定义已经退出；若这个前提被破坏，我们会漏掉一个孤儿而不是误杀——那仍是
   `skippedLivePid` 这条读数的用途；
3. **startup orphan sweep 依旧缺席**（V7 §7 处方的第三条腿，Batch 5'）：跨宿主重启的
   pid 台账根本不存在，本轮没有假装用重试队列顶替它。

**轮 27 门禁复跑（本轮日志，含两次变异的同轮实录）**：`cargo fmt --all --check` rc=0、
`cargo clippy --workspace --all-targets --locked -- -D warnings` rc=0、
`cargo test --workspace --locked` rc=0：**1,537 passed / 0 failed**（41 个 result 块；
`tauron-host` lib **417**、`tauron-adapter` lib **278**）。`pnpm format:check`／`lint`／
`command-surface:check`（85 条，孤儿 0）／`version:check`（26 处 1.1.0）／`gates:check`
（孤儿面 **634 ≤ 635**，本轮新增的 `pid_owner` 是私有方法、不进公开面；域台账 35 impl/170 方法、
83 组/357 fn、18 域、lib.rs 顶层 263 封顶——一个都没长大）／PublicSurfaceLedger（85 条无孤儿
元数据）全 rc=0。契约测试 **170 passed**（`wire-gate` **149** + `contract` 21）。
`pnpm verify` rc=0（64 段 Done）。`pnpm docs:check` **第一次 rc=1**：3 条漂移——本轮往
adapter／registry／runtime 里插了代码，行号又动了。这次**没有用 `--fix` 重锚**，而是把这
三条改成符号锚定（去掉 `:行号`），于是引用数从 231 降到 **228**、rc=0：宁可少三条可校验的
行锚，也不留一条"每次编辑都会烂"的锚。轮 26 那条"下一轮候选"因此**部分完成**——剩下 30 余条
行锚仍会在下一次编辑时漂移，仍待同样处理。

| 变异（逐字节恢复，md5 现场核对） | 结果 |
| --- | --- |
| M4：把重试里的归属判定换成永不命中的分支 | `tauron-host` lib 415 passed / **2 failed**——正是"同号在册须让位"（表级）与"重装复用同号 ⇒ 驱动让位"（注册表真路径）两条；反向那条（号不同必须照旧重试）仍绿，证明这条封口没有把重试本身打死。`wire-gate` 同步红：「重试不再核对 pid 归属（会误杀同号活进程）」 |
| M5：把 TS 侧 `skippedLivePid` 改名 | 契约测试 **2 红**：P0-2 的逐字段同构清单（「ReapStats 字段与 Rust ReapStats 不一致」）+ 本轮那条重试腿字段链（有序正则），证明这个第三态读数在前后端两侧都被钉住 |

恢复位点实录：`crates/tauron-host/src/runtime.rs` 恢复后 md5 `e04e3c55bb20fede08468fd240d2835e`、
`packages/tauron-host/src/shell-client.ts` 恢复后 md5 `4972c24c7b9b47288f65b4034a372a2a`
（均与变异前基线一致）；恢复后契约测试 170 passed。


#### 轮 28 逐行复核：把 V7 §7 风险表从"审计当时的事实"变成"可门禁的宣称"（2026-10-04）

**为什么这是断链**：前 27 轮修的是**代码链**（下游有没有人调用上游的承诺）。V7 §7 那张
"仍需修复或补证的风险"表是**宣称链**——它是这套仓库对外的风险结论，而它自 2026-10-04
之前写成、之后再没逐行回看过。按本仓口径「文档宣称与代码相反 = 断」，一张悄悄过期的风险表
与一条断掉的调用链同罪：读它的人会把已闭合的当缺口（少做），或把没闭合的当已闭合（多做误判）。

**做法**：7 行 + §2.2 的 6 行，每行拿本仓判据（真代码 + 真生产消费者 + 门禁）重判一次，
证据一律给**符号**（`fn` 名 / 字段名 / 测试名 / 门禁脚本名）而不是行号——行号锚每次编辑都漂，
本轮已有 3 条被改成符号锚。结论落在 V7 §7 的「轮 28 复核结论」表（原表保留并加"审计当时"前缀，
不把历史句悄悄改写成结论）。

**核对出来的三件事**：

1. **5 行确实已闭合**（Lazy loader / Shell / WASM / Dual-world / Runtime assembly）：各自都有
   生产侧读取方与门禁，且**边界**仍写在表里，没被写成"完全解决"——例如 Shell 行的代际纪律是
   真的，但它保护的四个 `start*` provider 本身仍是 `simulated: true` 的模拟启动；Dual-world
   审计点名的 `pendingCalls` 按"不留第三态"删掉了（没有异步传输就没有可接的表）。
2. **2 行仍未闭合，且第二分支处置是刻意的**：ServiceGraph 按处方的"明确降级为纯 validator"
   走（`shutdown_order()` 在 `crates/` 里除自身定义与测试外**零生产调用者**，本轮现场核对），
   并有一条测试反向钉住"没人伪造 lifecycle 执行器"；Upgrade 判为**部分**——清理腿真的闭合了
   （`with_cleanup_note()` / `remove_tree_if_exists()` 让清理失败上抛），执行器也是真的
   （`UpgradeRunner::run_phases` 真做 `copy_tree_fsynced` + 哈希复核 + `fs::rename` + 回滚），
   **但装配腿断在门外**：`UpgradeRunner` 在 `crates/tauron-distribute` 之外零引用（轮 40 更正：
   装配腿已接——`crates/tauron-adapter` 的 `DistributeUpgradeInstaller` 是唯一仓内消费者，见轮 40 一节）。
3. **§2.2 有两条已经被后续轮次推翻的旧断言**：①"当前仓库没有可执行的正式 sidecar fixture"
   ——`crates/tauron-test-sidecar`（含 `tests/sidecar_e2e.rs` 真进程用例）已经把它证伪，
   该行改为**已验证**并附 `publish = false` 边界；②四平台打包的"MSI 以 CI 为准"——实际发布
   走 `--bundles nsis`，MSI 是主动放弃而非悬而未决，措辞改为事实。

**本轮新发现的断链（登记为轮 29 的输入，不在本轮顺手改）**：同一个"有没有更新"的问题，
宿主里有**两条命令、两个相反答案**。`cmd_updater_check` 是**真实现**（注入端点后直接调
`tauron_distribute::check_for_update`，灰度 + 签名 + 清单校验）；`cmd_market_check` 是
**硬编码桩**（`available: false, simulated: true`，注释与 `tauri.rs` 都写着"源参数暂不使用"）。
而**对外 SDK 的自动更新走的是后者**（`AutoUpdateClient.checkUpdate()` 调 `host_market_check`）。
后果不是"难看"而是**功能级断裂**：宿主就算注入了真端点、`host_updater_check` 真的报出
`UpdateAvailable`，SDK 侧的 `available` 也永远为假、`onProgress` 永远不回调。
另须一并处理：`host_market_check` 收调用方下发的 `endpoints`/`pubkey` 却**一个都没用**
——文档自己说"接入后插件能把宿主指向自己的更新源"是供应链危害，那么"接"的方向就不该是读
调用方参数，而该是让 market 通道去读宿主注入的 sink。

**门禁（本轮新增一条，双向）**：`wire-gate` 的「V7 §7：风险表的复核结论必须与代码同形
（宣称链不得悄悄升级）」。它挡两种漂移——文档把「部分／仍成立」悄悄改成「已闭合」（正则钉住
Upgrade 行的 `**部分**`、`UpgradeRunner::run_phases` 符号，并禁止该行出现裸的"已闭合"），
以及代码悄悄多出 distribute 之外的 `UpgradeRunner` 引用点（遍历 `crates/*/src/**/*.rs`、
剔除注释后要求命中集为空；真接装配腿的那一轮必须**同时**改表，门禁才允许它变红一次）。

**诚实边界（不粉饰）**：
1. 本轮**没有改任何生产代码**：Rust 侧计数与轮 27 相同（没有新增/删除测试），改动面是
   两份文档 + 一条新门禁测试；
2. 复核只覆盖 V7 §7 与 §2.2 两张表的**行**，不是全仓文档审计：`docs/` 其余宣称（尤其
   `docs/api/` 逐命令的"已接入"措辞）本轮按行核对的是 V7 点名的这些，其余仍靠既有门禁
   （`simulated-never-commits` / `success-requires-effect` / 命令面 85 条）兜着；
3. 轮 26 立、轮 27 部分完成的"把行号锚改成符号锚"仍**未完成**：本轮没有继续清扫其余
   30 余条行锚，V7 表内改用符号锚不等于 `docs/` 整体已锚定。

**轮 28 门禁复跑（本轮日志）**：本轮没有改生产代码，验证与轮 29 的改动**合批一次跑**
（同一份 `cargo test --workspace` 同时覆盖两轮的 Rust 侧改动，不拆成两份数字）；
合批数字见下一节的轮 29 复跑日志。本轮新增的那条宣称链门禁在合批里是**先红后绿**的：
第一次跑就抓到自己的正则查错了表（§7 上面的"审计当时"原表里也有 `| Upgrade |` 行，
拿历史句当现状判据），改成"只在复核表里取行"后才通过——这条门禁在写下 30 秒内就证明了
它不是装饰。

#### 轮 29 更新可用性的数据源：SDK 读的桩把真通道挡在外面（2026-10-04）

**断链形态**（轮 28 逐行复核查出）：同一个问题"现在有没有可用的新版本"，宿主里有**两条
命令、两个相反答案**——`cmd_updater_check` 是真实现（宿主注入 `EndpointClient` 后直接
调 `tauron_distribute::check_for_update`：清单校验 + 灰度分桶 + 签名判定），
`cmd_market_check` 是硬编码桩（恒 `available: false`、`simulated: true`）。而**对外承诺面**
（SDK 的 `AutoUpdateClient.checkUpdate()`，`docs/installation.md` 的"点它应该看到什么"表里
就写着这条）读的正是那张桩，并且把 `simulated` 一律折成"没有更新"。后果是功能级的：
宿主就算注入真端点、真通道报出 `UpdateAvailable`，SDK 侧的 `available` 也永远为假，
`'available'` 状态与 `autoDownload` 分支永远进不去。**诚实的桩在这里不是"安全"而是"把
真能力一起挡在外面"**——这是本仓口径里"逻辑连贯存在孤儿逻辑"的反面教材：真实现有生产者、
有命令、有测试，唯独对外消费者的选择把它隔离了。

**修法（只换数据源，不加命令、不改线形）**：
1. `checkUpdate()` 改打 `host_updater_check`，把 `config.currentVersion` 当**入参**下发
   （宿主侧唯一消费点本来就把版本当入参要，`BrandInfo` 里没有 version 字段）；
2. **缺参即抛错**，不返回 `available: false`——后者在 UI 上就是"已是最新版本"，
   那是拿缺参冒充结论；`_fireCheck` 的自发路径仍吸收 rejection，不炸进程；
3. **三种"没有更新"分家**：`Unsupported`（端点未注入）与 `degraded`（端点不可达 /
   签名非法 / 清单非法）→ `info.degraded = true` + `status: 'error'`；只有"确实没有更新"
   （`UpToDate` / 灰度未覆盖）才落 `'idle'`。新增的 `info.degraded` 是让 UI 能区分这两者的
   唯一读数——没有它，"答不了"与"答了没有"在状态机上同形；
4. 宿主返回**空结果**也抛错（旧实现用可选链把它兜成"没有更新"）；
5. **下载 / 安装两条腿刻意不动**：仍打 `host_market_download` / `host_market_install`，
   那两条仍如实 `simulated: true`，客户端照旧抛错。所以本轮之后的真实形状是
   "能如实报有更新，还不能真的装上"——断点从"选错数据源"挪到了它本来的位置：
   `UpgradeRunner` 的装配腿（V7 §7 的 Upgrade 行，仍判**部分**）。

**宿主侧同步的两处措辞**（都是"文档宣称与代码相反"类）：`cmd_market_check` 的 `reason`
从"`tauri-plugin-updater` 不在依赖闭包内"改成指名真通道在 `host_updater_check`；
`host_capabilities` 的 `market-update` 域原因同步。另外把 `host_market_check` 的
`endpoints` / `pubkey` 两个参数从"暂不使用（接入后生效）"改成**按设计永远不用**——
更新端点的权威来源只能是宿主装配，让 webview 指定宿主去哪取更新清单正是该文档自己列出的
供应链危害第一步。`command-surface.md` 由生成器重生成（85 条不变）。

**门禁**：`wire-gate` 新增「轮 29：更新可用性只有一条权威通道」，三段——①SDK 调
`host_updater_check` 且源码里不再出现 `'host_market_check'`、`currentVersion` 作为入参、
缺参分支必须 `throw`、`Unsupported → degraded: true`、`degraded ? 'error' : 'idle'`；
②下载/安装两条腿仍在（防止有人把"真替换"顺手宣称掉），且 `simulated` 守卫不丢、
Rust 侧 `cmd_market_download` 仍 `simulated: true`；③桩必须说清真通道在哪
（`cmd_market_check` 函数体里出现 `host_updater_check`、capabilities 的 `market-update`
原因写"宿主本地桩域 … updater 域"）。客户端测试整文件重写为真通道用例，新增九条
（数据源、入参、缺参抛错且**零命令下发**、Unsupported 非 idle、"宿主确实答最新"才 idle、
签名非法等降级落 error、空结果抛错、发布时间映射，再加一条"真检查 + 桩下载"的实际形状用例）。

**诚实边界（不粉饰）**：
1. 本轮**没有**接通真替换：`UpgradeRunner` 仍零外部引用（那条断言由轮 28 的门禁看着）；
2. `UpdateInfo.simulated` 保留但**真通道的结果一律不设该字段**（不是设成 `false`：客户端
   不替宿主声明"我这次是真的"）；判据换成了 `degraded`。`size` / `releaseNotes` 两个字段宿主
   今天仍不给（`UpdaterCheckOutcome` 里没有），所以它们仍按可选处理，没被写进任何"已支持"清单；
3. `config.endpoints` / `pubkey` 是**保留而不删除**：删掉是破坏性 API 变更，且真通道不需要它们。
   门禁钉的是"SDK 不再从这两个字段取可用性"，不是"它们有用"；
4. 检查与下载/安装**分属两个域**（`updater` / `market-update`）这件事本身没被合并——它是
   命令面冻结下的既有形状，合并需要新命令，属 Batch 2' 的破坏性批次。

**轮 29 门禁复跑（合批日志）**：轮 28/29/30 三轮的改动至今未提交，因此共用**同一份**合批日志
（2026-10-04 采集：`/c/tmp/round30-final-rust.log`、`/c/tmp/round30-final-ts.log`）。
Rust：`cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` +
`cargo test --workspace` rc=0，日志内 `warning|error` 行计数 0，41 个测试二进制合计
**1539 passed / 0 failed**（`tauron-adapter` 280、`tauron-host` 417、`tests/sidecar_e2e.rs` 10）。
TS：`pnpm gates:check` 7 条门禁 + 自测全 OK；contract-tests **173 passed**（wire-gate 152 +
contract 21）；`pnpm docs:check` 228 条行号引用 / 7 个文档全部落在有效范围且指得到符号；
`pnpm command-surface:check` 85 条命令、孤儿 0、未归类 0；`pnpm version:check` 26 处版本号一致。
SDK 检查腿的判据不在这些数字里，而在 wire-gate 那三条断言（必须含 `'host_updater_check'`、
必须**不含** `'host_market_check'`、`currentVersion` 作入参）。

#### 轮 30 投递写侧把四种失败塌成一种码（负载下才露馅的断链，2026-10-04）

**怎么发现的**：轮 29 复跑把 TS 门禁与 Rust 门禁**并行**跑（cargo 与 vitest 抢 CPU），
`sidecar_e2e.rs` 的洪峰用例 `rapid_calls_are_bounded_*` 红了一次；单独跑三次全绿。
把它按原样在 8 个 busy loop 下重跑三次复现两次，抓到的不是"测试不稳"，而是断言
「只该看到『成功』与『受理满』两种结局」里成片的 `E_STATE_INVALID_TRANSITION`
（两次分别 32 条与 73 条）。

**断链的形状**：`ProcessCallDelivery::deliver()` 把 `write_frame` 的**所有** `io::Error`
一律 `map_err` 成同一个码。而写侧是四种完全不同的事实——`CommandSpawner::write_frame`
自己的文档就把它们逐条列了（`NotFound` / `InvalidInput` / `WouldBlock` / `BrokenPipe`）。
空闲机器上 sidecar 跟得上，pid 写队列那道界（32 帧）从不咬，塌缩就只在负载下现身：
"本地绿、并行红"的这一类不是噪声，是被这道界挡住了。

**为什么这不是"码不好看"而是可执行的误判**：

1. **语义假陈述**——走到写侧时状态机**已经放行**（调用已进 pending 表），报
   "状态机无匹配规则"与事实相反，把排查引向"检查插件状态 / 改调用序列"；
2. **丢处置信号**——`NotFound` / `BrokenPipe` 的事实是**租约没了**，与
   `stale_process_call` 同一件事。`E_LEASE_EXPIRED` 的 retry class 是 `AfterReconnect`，
   Rust 与 TS 两侧镜像（`errors.ts` 的 `'after-reconnect'`）都读得到它；塌成 `Never`
   类码等于把这条机器可读的"先重连再谈重试"信号整个删掉；
3. **界不分家但处置相同**——pending 表上限（每插件 100）与 pid 写队列上限（32 帧）对
   调用方是同一句话："额度满了，退避后重投"。所以**同码** `E_CALL_PENDING_FULL`，
   但 message 必须各自交代出处（否则分不清该取件腾位还是该等 sidecar 追上 stdin）。

**落地的分流**（新增 `stdin_write_failure(pid, &e)`）：`WouldBlock → E_CALL_PENDING_FULL`；
`NotFound | BrokenPipe → E_LEASE_EXPIRED`；其余（`InvalidInput`：帧超 `MAX_FRAME_BYTES`）
→ `E_INVALID_MANIFEST`——这是仓内既有的"入参不合协议"码（`host_updater_check` 的空
`current_version`、`host_recover_report` 的表外 `outcome` 都用它），比"状态迁移非法"贴近事实；
`Unsupported`（这个启动器压根没有写 sidecar stdin 的通路）**不再算错误**：落
`delivered: false`，与"没有 runtime 不投递"同一个形状，由上层转成
`ProviderResult::Unsupported`。序列化失败同样改到 `E_INVALID_MANIFEST`。

**门禁**（四条，缺一本轮不算落地）：

- 确定性反证 `process_delivery_write_failures_map_to_their_own_codes`：往 `FakeSpawner`
  注入 `ErrorKind`，逐条验码 + retry class + **pending 归零** + 失败的帧不得被记成已投递。
  为什么放 Rust 层而不是只靠 E2E：E2E 里写队列咬不咬**取决于本机负载**，而码分流是可判定语义；
- `process_delivery_without_write_channel_reports_unsupported`；
- `wire-gate`「轮 30：投递写侧按事实分码」：钉住四条映射、`Unsupported → delivered: false`，
  并要求 `deliver()` 函数体内**不再出现** `ErrorCode::E_STATE_INVALID_TRANSITION`；
- E2E 洪峰用例的断言从"恰好收 100 / 拒 30"改成**上界 + 出处**：`accepted ≤ 100`、
  `rejected ≥ 30`（130 > 100 恒成立，一发不拒就说明上限根本没生效）、
  `pending_for("main") == accepted.len()`（写侧落空不得留影子条目）、每条
  `E_CALL_PENDING_FULL` 的 message 必须点名"pending call 容量"或"写队列"之一。
  改的是**表述**不是强度：它现在测的正是它名字里那条不变量。

**台账副产品（同一轮，证明门禁真的在管事）**：domain-ownership 台账（V7 §10-9）被这次改动判红
两次——`process_delivery.rs` 顶层条目 6 > 基线 5，以及 `fn write` 组凭空多出第二个落点。处置分两件：

1. helper 改名 `write_failure → stdin_write_failure`。台账的归属键是**函数名首词**，`write` 组里原本只有
   `write_plugin_ui_activation`（把 UI 激活记录写进注册表，settings/recovery 域），与"sidecar stdin
   写失败怎么分码"是两回事，共用一个键会让台账把两处无关逻辑读成双 mirror site。新名字同时说清了
   **哪一次写**；
2. `process_delivery.rs` 预算 5→6 并登记 `fn stdin`（域 `process-delivery`），理由写进
   `contracts/adapter-domain-ownership.json` 的 `note`。**`lib.rs` 的 263 分毫未动**——本轮长高的
   是 275 行的投递专文件，不是 17,850 行的单文件，这正是台账希望发生的方向。

顺带一条事实：这条台账本轮确实"拦了新增顶层条目"，它不校验正确性；所以上面四条门禁才是本轮的判据。

**复现证据（同一轮日志）**：修复前 8 负载下 3 跑红 2；修复后同负载 4 跑全绿
（每跑 `test result: ok. 10 passed; 0 failed`）。变异反证：把 `WouldBlock` 那一支改回
`E_STATE_INVALID_TRANSITION`，新用例立刻红（`WouldBlock 分流错了`）。

**诚实边界（不粉饰）**：

1. **没有新增错误码**：85 条命令面与 24 码封闭词表都没动，改的是**既有码的归属**；
2. `E_CALL_PENDING_FULL` 的 retry class 仍是 `Never`——V4 在幂等被证明之前不自动重放，
   本轮不翻案。本轮修的是"报错了对象"，不是"顺手给它加自动重试"；
3. 写队列深度（32）与 pending 上限（100）都没调，负载下的**受理数**仍随调度浮动；
   想要确定值就得给洪峰加 pacing，那是另一种失真，没做；
4. 退避重试本身仍未落地：调用方拿到"额度满了"之后该等多久、最多重试几次，属 §90 的
   `RetryPolicy` / `RetryBudget`——A70 那行今天仍是"**无类型无实现**"（见本文件 §3 Batch 1 表）。

**轮 30 门禁复跑（合批日志）**：与轮 28/29 同一份日志（见轮 29 小节末），本轮相关读数：
Rust rc=0、**1539 passed / 0 failed**（41 个二进制），其中 `tauron-adapter` 单测 **280 passed**
（含本轮两条注入用例）与真进程 `tests/sidecar_e2e.rs` **10 passed**；clippy `-D warnings` 与
`cargo fmt --check` 零输出。contract-tests **173 passed**（wire-gate 152 条，含本轮新增的
「投递写侧按事实分码」那条）。domain-ownership 台账改名 + 调基线后重跑 OK：
「5 source files（35 个 impl 类型 / 170 个方法、84 组顶层 fn / 358 个函数、18 个域、
lib.rs 顶层条目 263 已封顶；数量全部 ≤ 基线）」，自测 6 个变异形状各按规则判定。
`docs:check` 228 条引用 / 7 文档 OK、命令面 85 条 OK、版本 26 处一致。

#### 轮 31 更新对话框的检查腿还读着宿主桩（一次点击两个相反答案，2026-10-04）

**怎么发现的**：轮 29 收口后回头读轮 28 记下的"同源链"——`AutoUpdateClient` 改完了，
但同一件事的**另一个入口**没跟着改。`ShellController` 里 `<oc-updater-dialog>` 的
`oc-updater-check` 监听器仍然调 `marketCheck()`（宿主桩），而且把返回值**整个丢掉**：
只 `void` 掉 promise，既不上屏也不进状态。注释当时写的是"宿主侧检查仍是桩"——那句话
从轮 29 起就是假的。

**断链的形状**：一次"检查更新"在同一个宿主上有两条入口，两条读两个数据源。

| 入口 | 轮 29 后读什么 | 结果 |
|---|---|---|
| SDK `AutoUpdateClient.checkUpdate()`（示例页 `#btn-update`） | `host_updater_check`（真通道） | 能如实报 `available` / `degraded` |
| 壳层 `<oc-updater-dialog>` 的「检查更新」按钮 | `host_market_check`（恒 `available: false` + `simulated: true`） | 返回值被丢弃，对话框永远停在初始文案 |

后果不是"难看"，是**同一次用户动作得到两个相反的答案**，而且宿主注入真端点之后依然如此：
SDK 报"发现新版本"，对话框说没这东西。轮 29 立的口径（"答不了"不得演成"已是最新版本"）
在这条腿上根本没生效，因为它连"答"都没接。

**顺带查出的两处示例回归**（轮 29 的改动把示例页面弄坏了，本轮一并修）：

1. `#btn-update` 自轮 29 起**恒抛错**——`AutoUpdateClient` 现在要求 `currentVersion`
   必填，示例的 `new AutoUpdateClient({...})` 没给。用户看到的是"点检查更新只出红字"；
2. 示例的展示代码把 `degraded` 与"没有更新"合并成同一句话（先判 `available`），
   正是轮 29 禁掉的那种合并——宿主没注入端点时它会显示"已是最新版本"。

**落地的修法**（三件，都要求有真消费者）：

1. `toUpdateInfo`（`auto-update-client.ts`）改为**导出**：判定"什么算答不了"的那段逻辑
   只能有一份。壳层自己再写一遍就是双镜像点，而轮 29 修的就是这种分叉；
2. `ShellController` 新增两个构造参数：`currentVersion`（`host_updater_check` 的必填入参）
   与 `onUpdaterCheck(info)`（把结论交回接入方写 UI）。检查腿 `_checkForUpdate()` 打
   `client.updaterCheck(currentVersion)`；**缺 `currentVersion` 时一个命令都不发**，直接经
   `onError` 报缺参——回落桩会拿到形似结论的 `available: false`；
3. 示例 `<oc-updater-dialog>` **真的挂到页面上**（`index.html` 加元素 + `#btn-updater-dialog`
   打开它），`onUpdaterCheck` 把结论写回 `version` / `message` / `status`，
   `status` 取契约词表（轮 32 收口；本轮初版写的是「只在 `available` 时推进到 `ready`」，
   那是把客户端终态与按钮标签混为一谈——检查阶段没有下载/安装发生，写的就是
   `available`，`ready` 只属于安装成功之后）：`degraded → 'error'`、
   `available → 'available'`（主按钮「开始更新」→ 那两条仍是 `simulated` 桩，点了会经
   `onError` 如实报错）、否则 `'idle'`。版本号取自示例自己的
   `package.json`（`import { version as APP_VERSION }`，`tsconfig` 补 `resolveJsonModule`）：
   宿主不提供版本——`BrandInfo` 没有 version 字段，`host_updater_check` 反过来把它当入参要。

**为什么挂元素算本轮的一部分**：控制器那条腿此前只有单测里没有用户——组件派发方存在、
监听方存在，但仓内没有任何页面装配过它，"检查结论写回 UI"这条腿是纸面的。
`oc-updater-check` 在本仓库正因为"零派发孤儿"被记过一次账（见 `wire-gate` 那对正反门禁）。

**门禁**：`wire-gate` 新增「轮 31：更新对话框的检查腿与 SDK 同源」，五段——①控制器必须
`this.client.updaterCheck(`，且源码里 `marketCheck` **一次都不出现**；②`toUpdateInfo` 必须是
`export function`，控制器必须从 `auto-update-client.js` 导入它；③缺 `currentVersion` 的分支
必须先 `return`；④示例必须声明 `currentVersion: APP_VERSION`、版本号来自 `package.json`、
`describeUpdate` 先判 `degraded` 再判 `available`，**并且**页面挂载
`<oc-updater-dialog id="updater-dialog">` + 打开它的按钮 + 写回 `updaterDialog.message`；
⑤测试侧后门封掉：`shell-controller.test.ts` 的 `CONTROLLER_CAPS` 能力表不得再声明
`'host_market_check'`（声明了它，即使检查腿退回桩，mock 也会照样答绿）。
控制器单测新增四条：真通道 + 入参原样下发（`{ currentVersion: '1.4.2' }`）、缺参时
`invocations` 为空、`available` 与 `degraded` 走 `onUpdaterCheck` 两种读数、空结果不当作"没有更新"。

**变异反证**（同一轮）：把控制器那一行改回 `marketCheck()` → wire-gate 红 + 三条单测红；
改回后恢复。示例侧把 `currentVersion` 删掉 → 门禁 ④ 红。

**诚实边界（不粉饰）**：

1. **下载 / 安装两条腿刻意不动**：`oc-update-start` 仍打 `host_market_download` /
   `host_market_install`，仍是 `simulated` 桩且有守卫拒绝推进。本轮之后"能如实报有更新、
   两条入口同一个口径"，仍**不能**真的装上更新（断点在 `UpgradeRunner` 装配腿，V7 §7；轮 40 已落地该腿，见轮 40 一节）；
2. `onUpdaterCheck` 是**新增可选参数**，不是破坏性变更；不传它时检查照做、结论不上屏
   （控制器不猜接入方想怎么显示），`status` 由接入方驱动，组件本身保持哑；
3. 示例页面挂载对话框只证明**链路通**，不证明更新能力可用：默认装配没有注入
   `EndpointClient`，所以点「检查更新」的诚实答案是 `degraded` 那句"更新通道答不了"。

**轮 31 门禁复跑（本轮采集日志：`/c/tmp/round31-ts-2.log`、`/c/tmp/round31-rust-full.log`）**：
`pnpm verify` rc=0（20 个包：build + `--no-bail` typecheck + test 全过），其中
`@tauron/host` **417 passed / 22 文件**、contract-tests **174 passed**（`wire-gate.test.ts` 153 条、
`contract.test.ts` 21 条）、`@tauron/ui-primitives` 268 passed、`@tauron/shell-events` 4 passed；
示例侧 `pnpm typecheck` rc=0。Rust 本轮无改动，`cargo fmt --all` rc=0、
`cargo clippy --workspace --all-targets -- -D warnings` 零输出（增量缓存在 1.24s 内完成即证明
无待编译改动）、`cargo test --workspace` **41 个测试二进制 1539 passed / 0 failed**。
`pnpm gates:check` rc=0（7 条门禁 + 台账自测 6 个变异形状；孤儿 API 台账 12 条未接线宣称 +
5 条已接线反例复核通过，自动发现 634 ≤ 基线 635；domain-ownership 台账 5 源文件 /
84 组顶层 fn / 358 个函数 / 18 个域，`lib.rs` 顶层条目 263 仍封顶）；`pnpm docs:check`
228 条行号引用 / 7 个文档 OK；`command-surface:check` 85 条（底座 61 / 运行时 22 / 安装 2）、
孤儿 0；`version:check` 26 处版本号为 1.1.0。

**这一轮被门禁抓到的一条（记下来，别把它说成"顺手的小错"）**：第一次合批 `pnpm verify` **是红的**——
`@tauron/host` typecheck 报三条 `TS2322`（本轮新写的 `onUpdaterCheck` 用例里，箭头函数把
`Array.prototype.push` 的返回值 `number` 当 `void` 返回）。它之所以躲过了此前的验证：本包的
`build` 是 `tsc -p tsconfig.build.json`，而那份配置 **`exclude` 了 `src/**/*.test.ts`**，
`test` 走 vitest（转译、不类型检查）——**测试文件只有 `typecheck` 这一层看**。改成
`{ seen.push(info); }` 之后合批 rc=0。结论与轮 30 那条并列：**分层跑门禁时红的那层必须一起看**，
"我这包测试过了"既不等于"类型过了"，也不等于"测试文件被类型检查过"。

#### 轮 32：更新状态词表有三面镜像，「立即重启」在真装配里点不出来（2026-10-04）

**发现路径**（轮 31 的残留面）：轮 31 把检查腿接上之后，同一次点击的**下一步**没查。
顺着"检查完成后用户能点什么"读渲染分支，看到 `status === 'done'` 才显示「立即重启」——
而全仓 `grep` 不到任何一处赋值 `'done'`：`AutoUpdateClient` 安装成功的终态叫 `'ready'`。
于是去数"更新到哪一步了"这个概念在仓内有几套取值，答案是**三套**：

| 位置 | 角色 | 安装成功的终态 | 重启/完成分支 |
|---|---|---|---|
| `AutoUpdateClient.UpdateStatus`（`@tauron/host`） | **写侧**，对外 API | `ready` | — |
| `OcUpdaterDialog.status`（`wc-shell.ts`） | **读侧**，用户看到的面 | 认 `done` | `done` → 「立即重启」→ `oc-restart` |
| `UpdaterStore` / `UpdaterState`（`updater-dialog.ts`） | 平行实现，**零生产消费者** | `restarting` | `restarting` |

**为什么这是断链而不是"文案不统一"**：`ready` 不匹配 `done`，就落进渲染的兜底分支，
显示「开始更新」——用户点它会**把已经装好的更新重新下载+安装一遍**；同时
`oc-restart` → `ShellController` → `windowRelaunch()` → `host_window_relaunch` 这条腿
监听方、命令实现、错误处理全都在**只差派发不出来**。轮 29/31 修的孤儿监听是"有监听没派发方"，
这一条是"有派发方、有实现，但没有能到达它的路径"——三种门禁当时都看不见它：
事件名两侧都写对了（都在契约里）、监听/派发成对。组件侧同样是盲区：轮 32 之前
`wc-shell.test.ts` 里 `<oc-updater-dialog>` 只有四条用例（挂载、文案、「稍后」不派发
`oc-close`），**没有一条设置过 `status`**——所有渲染分支从未被任何测试走过，
所以既没人发现 `done` 是死值，也没人发现 `ready` 会退回「开始更新」。
根因是**跨层的状态取值没有类型**：`status` 是裸 `string`，谁都能写谁都不错。

**落地的修法**（把"状态"当成和"事件名"同级的契约，而不只是文案）：

1. 词表进契约包：`@tauron/shell-events` 新增 `UPDATER_STATUSES`（8 值，沿用写侧那套——
   它已是对外 API，改名属破坏性变更）与 `UpdaterStatus`；
2. **主按钮动作进契约包**：`UPDATER_PRIMARY_ACTION: Record<UpdaterStatus, 事件名>`。
   三段 `if/else` 各写一遍事件名与状态字面量，正是漂移长出来的地方，现在标签与派发都从这张表推导；
3. 写侧收口：`auto-update-client.ts` 的 `export type UpdateStatus = UpdaterStatus;`——
   从"第二套字面量"变成"契约的别名"，改词表会同时打断两侧编译；
4. 读侧收口：`_status: UpdaterStatus` 并 `export type { UpdaterStatus }`，示例的
   `UpdaterDialog` 结构类型也用 `UpdateStatus`（写错状态 = 编译错误，不再是运行时静默）。
   词表**刻意不含** `done` / `updating`：它们从未被任何生产者产出，留着只会再养出一套平行词表。

**第三套镜像怎么处置**：`UpdaterStore` 没有接成事实源，也**没有删**——它随 v1.0.0 发布过，
收口或对齐属破坏性变更，需单独批准。本轮做的是把它登记进
`contracts/orphan-public-api.json`（第 13 条，probe `\bUpdaterStore\b|\bUpdaterState\b|\bUpdaterConfig\b`），
在 `updater-dialog.ts` 文件头写明"本文件不是更新流程的状态契约"，并指出它的
`endpoint` / `retryCount` / `maxDownloadSpeed` 三个配置项没有任何代码读过。

**门禁**：`wire-gate` 新增「轮 32：更新状态词表只有一个事实源」，六段——①词表取值必须与
客户端对外 API 逐字一致；②映射覆盖的状态集合 = 词表集合，值只能是三个主按钮动作，且
`restart` **只**属于 `ready`；③写侧必须是 `= UpdaterStatus` 别名，不得再出现联合字面量声明；
④读侧 `_status` 必须带类型、主按钮派发点必须唯一、渲染分支不得出现 `'done'`/`'updating'`、
组件里 `SHELL_EVENTS.restart` 只允许出现在标签推导那一处；⑤示例写回的是契约状态且
`degraded → 'error'`；⑥`UpdaterStore` 必须仍在孤儿账上（既不许静默删除，也不许静默失踪）。
另外把 `dispatchedKeys()` 扩到认得**从映射派发**这一形态：只有当文件里确实出现
`new CustomEvent(UPDATER_PRIMARY_ACTION[` 时才并入映射表的值域——无条件并入等于替映射表里的
动作凭空发明派发方，那条反向孤儿监听门禁就会放过真孤儿。配套用例：
`shell-events/index.test.ts` +3 条（共 7），`wc-shell.test.ts` +4 条（覆盖 8 个状态的
标签、进度条出现条件、每个状态都有非空主按钮）。

**变异反证**（同一轮，实测）：把契约里 `ready: SHELL_EVENTS.restart` 改成 `updateStart` →
`pnpm -C packages/tauron-contract-tests test` rc=1，**两条**同时红：轮 32 门禁（映射不再唯一）
与「ShellController 监听的每个契约事件必须有派发方」（`oc-restart` 失去唯一派发路径）。
改回后复跑绿。这条反证说明 ② 与反向门禁是两道独立的网，不是一道。

**轮 32 自己也被门禁抓到一条（台账门禁的实现与它自己的规则文档不一致）**：
`pnpm gates:check` 第一次跑是**红的**——
`orphans 条目已接线，必须从台账删除并同步文档：UpdaterStore / UpdaterState / UpdaterConfig
（消费者：packages/tauron-ui-primitives/src/index.ts）`。
台账 `note` 明写接线面要剔除「re-export 行」，而 `check-orphan-public-api.mjs` 的再导出状态机
只认 `export {`，**不认 `export type {`**：那份 index.ts 里
`export type { UpdaterActionResult, UpdaterConfig, UpdaterSnapshot, UpdaterState, UpdateInfo }`
是多行块，成员行 `  UpdaterConfig,` 既不含 `from` 也不是声明行，于是被当成"有人在用"。
补上 `export(\s+type)?\s*[*{]` 一支即绿（不是放宽判据：这条行本来就是再导出）。
**非空洞性反证**（同一轮实测）：把这条 orphan 的 probe 换成 `\bShellController\b`（真消费者在
`examples/minimal-app/src/main.ts` 与 `packages/tauron-host/src/shell-controller.ts`）→
门禁 rc=1 并原样点名这两个文件；换回真实 probe → rc=0。**过程教训**：第一次变异跑的是
`node -e "…'\\\\bShellController\\\\b'…"`，bash 双引号把 `\\b` 变成 JS 里的**退格字符**（0x08），
正则谁都匹配不到，于是"门禁没红"被误读成"检测器坏了"——读回字节（92,98 = `\`+`b`）才立起证据。
**结论与轮 30/31 那条并列：变异测试自己也要被证明有效，否则它只是在给你讲好听的结论。**

**轮 32 门禁复跑（本轮采集日志：`/c/tmp/round32-verify2.clean.log`、`/c/tmp/round32-ts2.log`）**：
`pnpm format` rc=0、`pnpm verify` **rc=0**，其中 `@tauron/host` **417 passed / 22 文件**、
contract-tests **175 passed**（`wire-gate.test.ts` 154 条 = 轮 31 的 153 + 本轮 1 条、
`contract.test.ts` 21 条）、`@tauron/ui-primitives` **272 passed**（`wc-shell.test.ts` 17 条，
轮 31 是 13）、`@tauron/shell-events` **7 passed**（轮 31 是 4）。
`pnpm gates:check` rc=0（7 条门禁 + 台账自测 6 个变异形状；孤儿台账 **13 条**未接线宣称 +
5 条已接线反例复核通过，自动发现 634 ≤ 基线 635；domain-ownership 5 源文件 / 35 个 impl 类型 /
170 个方法 / 84 组顶层 fn / 358 个函数 / 18 个域，`lib.rs` 顶层条目 263 仍封顶）；
`pnpm docs:check` rc=0（228 条行号引用 / 7 个文档）；`command-surface:check` rc=0
（85 条：底座 61 / 运行时 22 / 安装 2，孤儿 0、未归类 0、无代码层判定 9）；
`version:check` rc=0（26 处版本号 1.1.0）。**本轮 Rust 侧零改动**（`git status` 里的 `.rs`
条目来自前几轮未提交的工作），故不重跑 `cargo test --workspace`；最近一次同轮实测是轮 31 的
41 个测试二进制 1539 passed / 0 failed（`/c/tmp/round31-rust-full.log`）。



#### 轮 33：`host_updater_status` 是通道事实的唯一出口，却在 TS 侧零消费者（2026-10-04）

**发现路径**：轮 32 收尾时回扫"更新链上还有哪条读数没人读"，命中
`ShellClient.updaterStatus()` —— 它在 TS 侧**零调用点**（只有 `shell-client.ts` 自己的定义与
`docs/api/command-surface.md` 的一行）。同时读 Rust 侧 `cmd_updater_status`：它把
`ShellExtState::update_state` 盖到返回值上，而 `update_state` 的两个写入方
（`cmd_market_download` / `cmd_market_install`）**都是桩**。桩推进账本是如实行为（返回里带
`simulated: true` 与 `reason`），但 `state` 这个字符串自己不带出处。

**两半都断，且是同一件事**：

| 半截 | 事实 | 用户看到的 |
| --- | --- | --- |
| 读侧 | `available: false` 在宿主侧至少对应四种情况：已是最新 / 不在灰度批次 / 被崩溃门禁停发 / 端点未装配。后三种只写在 `host_updater_status` 里，而它零消费者 | 同一句"没有更新" |
| 写侧 provenance | `state` 现有写入方全是桩，`installed:<v>` 分不出模拟与真实 | "点了一下模拟安装"可以说成"已经装好了" |

第二半正是轮 32 那条"状态词表三面镜像"的同族：**把出处信息丢在字符串里**，读侧只能猜。

**修法（四件，缺一不可）**：

1. **Rust 加 provenance 线字段**：`UpdaterStatus` 增 `state_simulated: bool`，
   `cmd_updater_status` 把账本对（`ext.update_state.clone()` + `ext.update_state_simulated`）
   **一次性交给** `UpdaterSink::status(ledger_state, ledger_simulated)`，由 sink 推导
   （`state_simulated = ledger_state.is_some() && ledger_simulated`）；
   `ShellExtState` 增 `update_state_simulated`，两条桩命令各写下
   `ext.update_state_simulated = true`。做成**独立字段而不是给 `state` 加前缀**：前缀要读侧
   解析，解析一次就漂一次。`state === null` 时本值恒 `false`（没有状态，谈不上它怎么来的）。
   **加字段不是加命令**：85 条命令面冻结不破，`command-surface:check` 仍是 85 条。
2. **TS 同步线形**：`shell-client.ts` 的 `UpdaterStatus` 增 `stateSimulated: boolean`。
   Rust 侧 `#[serde(rename_all = "camelCase")]`，两侧字段集由门禁逐字段对齐（少一半就是前端
   读到 `undefined`）。
3. **诊断口径做成两条入口共用的一份**：`auto-update-client.ts` 新增
   `enrichUpdaterInfoWithChannel(info, readChannel)`（内部私有 `describeUpdaterChannel`），
   `AutoUpdateClient.checkUpdate()` 与 `ShellController._checkForUpdate()` 都调它，读法各自给
   （前者 `_backend.invoke`、后者 `client.updaterStatus()`）。**为什么必须共用**：轮 29 修的是
   判定分叉、轮 31 修的是检查腿分叉，展示口径再各写一份就是第三面镜像——同一个用户动作的两条
   入口不能一个报"灰度 30%"、一个报"没有更新"。
4. **补的读数必须真上屏**：示例 `describeUpdate()` 原先在 `!available` 分支只说
   `已是最新版本（x）`，把刚补进来的 `info.reason` 吞掉——那这轮就只是"接口有、行为无"。
   现在 `!available && reason != null` 说成"没有可用更新（当前 x）——依据：…"，
   "已是最新版本"只在**没有**反向依据时才说得出口。

**三条失败边界都不改结论**：命令缺席（旧宿主）、命令报错、命令答空（`undefined`）都原样返回
`info`。空读数不得渲染成"通道已装配、灰度 NaN%"——这与轮 31 给检查腿立的"空就是答不了"是同
一条口径。有更新时**不**多打诊断命令：附加读数不该让一次点击多付一条 IPC。

**验证时又抓到一条，且它是本轮的第三半**：首版把 provenance 的组装写在 sink 里
（`state: None` + `state_simulated: false`），由 `cmd_updater_status` 事后覆盖——
`gates:check` 的 `check-simulated-never-commits.mjs` cross-check 直接判红：被网关源里出现
`simulated: false` 就是**无中生有地断言"这条状态是真的"**。这条判定不该被绕过（把 needle
改成 `state_simulated` 也算绕过），所以修法是让这种断言**写不出来**：`UpdaterSink::status`
的账本对两个参数变成必传，sink 只做推导。附带收掉一个同类隐患——旧签名下 sink 可以只报
`state` 而不报它的出处，新签名从类型上就拿不到这个自由度。

**门禁**（`wire-gate.test.ts` 新增「轮 33：更新通道诊断只有一个出口，账本的模拟推进不得被读成
真实状态」六段）：① Rust ↔ TS 的 `UpdaterStatus` 字段集逐字段对齐且必须 camelCase；② 读侧
成对，且**成对到签名上**——`status(ext.update_state.clone(), ext.update_state_simulated)`、
`fn status(&self, ledger_state: Option<String>, ledger_simulated: bool)`、
`state_simulated = ledger_state.is_some() && ledger_simulated;` 三条各钉一条，并**反向**钉住
`state_simulated: false` 字面量不得出现在 adapter 源里；③ 写侧
逐条登记——每个 `ext.update_state = Some(…)` 后面必须跟一条
`update_state_simulated = true`，两处数量不等即红（新增写入方漏标就撞这条）；④ 两条入口都调
`enrichUpdaterInfoWithChannel`、`describeUpdaterChannel` 全仓只有一份定义、控制器不再出现
`grayscalePercent` 拼写、三条失败边界与"有更新不多打命令"的写法各钉一条、示例必须打印依据；
⑤ 测试侧不留后门：默认 `CONTROLLER_CAPS` **不**得声明 `host_updater_status`（否则缺席路径没有
用例），诊断后端**必须**声明它（否则消费者断言空转）；⑥ 本轮口径必须成文（缺口方案 +
`docs/api/command-surface.md` 的线形）。

**Rust 单测**：新增 `updater_status_must_say_whether_the_ledger_came_from_a_stub`（新鲜态
`state: None` + `state_simulated: false`；download 后账本 `downloaded` 段 + provenance `true`；
install 后 `installed` 段 + provenance `true`）；扩
`updater_check_runs_tauron_distribute_and_status_reads_the_ledger` 加
`assert!(!s.state_simulated, "未经桩的账本写入不得被标成模拟")`；扩
`plugin_caller_cannot_use_market_commands_but_main_window_can` 确认被身份判定拒绝的那条**不写**
出处位（拒绝的命令不该留下任何账本痕迹）。

**TS 单测**：`auto-update-client.test.ts` +3（补过的 reason 全文钉住且 `client.info` 同份、
有更新时 `invocations` 只有检查命令、缺席/报错/答空三种都不改结论与状态）；
`shell-controller.test.ts` +6（"没有更新"拼灰度读数、degraded 走同一出口、`stateSimulated`
真/假两种文案、有更新不问诊断、诊断三态失败不掩盖结论、不传 `onUpdaterCheck` 时一条诊断命令
都不发）。

**诚实边界（不粉饰）**：

1. `state_simulated` 在桩阶段恒 `true`（有 `state` 时）；决定它的是**账本写入方**的 provenance
   标记——`UpgradeRunner` 装配腿（V7 §7）落地后要把 `update_state_simulated` 写成 `false`，
   本轮**没有**动它，也没有伪造真实推进（轮 40 已落地：装配腿注入后写 `false`，桩路径保持 `true`）；
2. 命令面仍 85 条、错误码词表仍 24 条封闭——本轮只加了一个返回字段；
3. 诊断是**附加**读数：它不改 `available` / `degraded` 的判定，那两个数仍只来自
   `host_updater_check`；
4. `@tauron/host` 的公开类型里 `UpdaterStatus` 有**两个不同含义**（`@tauron/shell-events` 的
   状态词表 vs `shell-client.ts` 的通道线读数）。本轮用导入别名
   （`UpdaterStatus as UpdaterChannelStatus`）绕过，**没有**改名：后者是 1.0 起 `index.ts`
   的公开导出，改名是破坏性变更，要单独走批准。这条漂移已写进包 README，别当它不存在。

**轮 33 门禁复跑**（同一轮日志：`round33g.log`、`round33h.log`；数字全部取自这两份，不凭记忆）：
首版"代码写完"时 `gates:check` 与 `docs:check` **各抓出一条本轮自造的问题**。第一条是上面那句
sink 自述 provenance；第二条更说明行号锚点的脆弱——本轮为解释线形在 `adapter/lib.rs` 顶部加了
十几行文档，方案里 47 条 `symbol:行号` 引用的行号被**整体下推**，`check-doc-line-refs.mjs --fix`
重锚这 47 条（只改"唯一最近解"），另有 3 条区间引用（本方案里指向 `adapter/lib.rs` 第 3125–3140
行的那两条，以及同一行号开头的那条）因起点落在标点行被判不可核对，按判定自身的口径改成
**不带数字的符号锚点**，引用数从 231 降到 225。
收紧之后复跑全绿（`round33h.log`，逐项 rc=0）：`prettier` / `cargo fmt` / `docs:check`
（7 份文档、225 条引用）/ `gates:check`（8 条门禁）/ `command-surface:check`
（85 条命令：底座 61 / 运行时 22 / 安装 2，孤儿 0）/ `contract-tests` 176 passed（其中
`wire-gate` 155）/ `cargo test --workspace --locked` 41 个测试二进制、1540 passed、0 failed /
特性矩阵 `tauri` 311、`plugin-install` 302、`tauri,plugin-install` 337、`runtime-wasm-broker`
285（各 0 failed）/ `cargo clippy --workspace --all-targets -D warnings`。

**反证**（`round33-mutD.log`）：把 sink 改回"自己声明出处"（`state_simulated: false` 字面量、
忽略传入的账本标记）后三条口径**同时**红——① `gates:check` rc=1，
`Simulated-Never-Commits` 在 `adapter/lib.rs` 第 2531 行报
「`simulated: false` asserts a real effect」；② `wire-gate` rc=1（1 failed）：
「state_simulated 必须由"账本有无"推导」那条 needle；③
`cargo test -p tauron-adapter updater_status` rc=101，
`updater_status_must_say_whether_the_ledger_came_from_a_stub` 在 `adapter/lib.rs` 第 17092 行
panic「桩推进的 state 必须带 provenance，否则读侧无从分辨」。三层各管一件事：门禁管"不许无据声明"、
wire-gate 管"装配形状不许退回可只报 state"、单测管"读数本身对"。改回推导式后复跑回绿。

### 轮 34：施工单指路到已删除、且被门禁反向钉住的形状

**发现路径**：轮 33 收尾时按"文档宣称与代码相反也算断链"的口径回扫本方案的**待办**部分
（Batch 0'–4'），不是回扫已闭合条目。抓到的是一条会直接坑掉下一个人的处方：Batch 2' 的 A75
叫人来一份新方案，让装配走 `service_startup_order`、app-exit 走逆序 `service_shutdown_order`——
而这两个字段**轮 11 就删了**，并立了反向门禁「服务拓扑图不得再抄两份无人执行的序」把这两个名字
钉成永不再出现。照字面做的人必然撞红，还会以为是门禁误报。同一口径再扫出两处：

| 位置 | 与代码相反的说法 | 代码事实 |
| --- | --- | --- |
| Batch 2' A67/A68 | 「需 `host_protocol_info` 一条新命令承载协商结果」，未提任何门槛 | 该命令不存在；`wire-gate` 有「V5 §46.3 冻结」断言把根命令面钉在 **85** 条，新增必须显式过账 |
| `multi-plugin-substrate-roadmap.md` M-9（状态仍 🟡） | 「两份注册表互不感知」，并把载体写成带行号的 `bootstrap.ts` 引用 | `bootstrap.ts` 已随 W1-b **删除**（门禁「已删除的死导出，不得回归」）；`maxPlugins: 32` 这个第二事实源也没了（门禁「TS 出现硬编码 `maxPlugins` 字面量」）；生产路径只剩 Rust `tauron-host::Registry` 一份 |
| `capability-closure-plan.md` §2 / P1-1 / P1-10 | 「活的应用层直接 import 死的框架层」，指 `bootstrap.ts` 那行 | 同上：文件不在，"解 P1-1"这个待办已经没有对象；core 侧 `PluginRegistry` / `ConfigManager` 今天是**零生产消费者**的历史 SDK 面 |

**修法**：三处待办/过期行都改成**带依据的改判**，而不是删掉了事——A75 写明"已删除 + 反向门禁 +
诚实前置（先造出服务级回收动作，再谈按拓扑序执行）"；A67/A68 把顺序倒过来：先走"协商结果并入
既有命令的返回字段"（轮 33 的 `stateSimulated` 就是先例，加字段不破冻结），装不下再申请显式解冻；
M-9 与 §2 各自注明删除史与钉住它的门禁名。复核命令 `rg 'service_startup_order|…'` 那行补上
**期望零命中**——本方案的复核命令块此前有个系统性毛病：有的命令写了期望（「FairQueue 应零命中」），
有的没写，跑的人对"零输出到底是通过还是没找到"读不出结论。

**同一把尺再扫本方案自己的现状表，抓到本轮的第二半**：Batch 2 的 A75 行（§123 表）此前仍写着
「两个序只存字段」并给出三个行号，把轮 11 删掉的装饰**当成今天的缺口在描述**；§237 的
「ServiceGraph acyclic」行同样只说"拓扑序不执行"。这与待办里那条处方是**同一个矛盾的两侧**——
待办教人去做，现状表教人相信它还活着。本表的惯例是**原地更新并标轮次**（F3、A109 两行都带
「轮 11」标注），A75 是唯一漏标的一行。现在 A75 行改判为「轮 11 删装饰、轮 34 复核本行」，
并把剩下的真实缺口写清楚：缺的是**可排序的服务级回收动作**，不是那两个字段。

**孤儿账同批登记**（否则"M-9 已消除"只是另一句无人复核的文字）：`PluginRegistry`
（`packages/tauron-core/src/registry.ts`）与 `ConfigManager`（同包 `config.ts`）新增两条
`contracts/orphan-public-api.json` 条目，台账复核从 13 条增至 15 条。这两条的探针在接线面内
除声明文件与 core 自己的再导出之外零命中，故门禁同时证明"零消费者"这句话今天仍然成立；
将来谁把它们接上，门禁会红并要求删条目 + 同步文档。

**门禁**（`wire-gate.test.ts` 新增「轮 34：施工单不得再把已删除、且被门禁反向钉住的形状写成待办」）：
① A75 旧处方字面量不得回归（`not.toMatch(/让拓扑序\*\*真的执行\*\*——装配走/`），且必须同时出现
"这两个字段已在轮 11 删除"、反向门禁名、以及 rg 复核行的"期望零命中"；② A67/A68 那条**按 bullet
抽取**后必须含 `host_protocol_info` + 「§46.3 冻结…85」+ "返回字段…轮 33"（证明解冻不是默认路径）；
③ 两条新台账条目必须在账；④ M-9 必须是"已消除（轮 34 复核）"改判形态、收口方案横幅必须点名
`bootstrap.ts` 已删除；⑤ 现状表那条"两个序只存字段"的旧描述不得回归，且 A75 行必须带上
轮 11 删除与"服务级回收动作"这条真实缺口（现状表和待办必须同向，否则下一轮又会只改一侧）。

**诚实边界**：本轮**没有**动代码——这三条是文档与门禁的矛盾，不是新的运行期断链；拓扑序的真实
执行点（服务级 shutdown）仍未落地，A75 仍是待办，只是现在写的是"要先有什么才能做什么"。
`PluginRegistry` / `ConfigManager` 的收口（删除或对齐）属破坏性变更，随 v1.0.0 发布过，需单独批准。

**验证（同一轮日志，非记忆）**：`pnpm docs:check` rc=0、`pnpm gates:check` rc=0（孤儿账
「15 条未接线宣称复核通过、5 条已接线反例复核通过、自动发现 634 ≤ 基线 635」）、
`pnpm command-surface:check` rc=0（**85 commands**：底座 61 / 运行时 22 / 安装 2，孤儿命令 0）、
`pnpm -C packages/tauron-contract-tests vitest run` rc=0（Test Files 2 / **Tests 177 passed**）。
本轮门禁新增 **15 条断言**，其中 10 条逐条做过变异证明（每次只改文档/台账一处，跑 `vitest run -t "轮 34"` 看红，
还原后全绿）：现状行写回"两个序只存字段＋行号"的旧描述 → 红；删掉"轮 11 删"标注 → 红；把"服务级回收动作"
换成同义词 → 红；待办的"这两个字段已在轮 11 删除"改成"待确认"→ 红；反向门禁名改成"一条并行门禁"
→ 红；"期望零命中"改成"跑一遍看看"→ 红；A67/A68 的「§46.3 冻结」改名 → 红；台账里
`ConfigManager` 改名 → 红；M-9 的"已消除（轮 34 复核）"改成"待复核"→ 红；收口横幅的
"已随 W1-b **删除**"改成"迁动"→ 红。还原后复跑：Test Files 2 / Tests 177 passed。
未逐条变异的 5 条如实记下边界：`A67/A68 那条待办必须抽取到` 的 notNull 在上一轮写门禁时就因正则漏 `m`
标志**真实红过**；m2 变异（只动 A67/A68 的冻结点名）让断言单独红一次，这同时证明"按 bullet 抽取"
确实只圈住那一条待办、没有把全文当 needle 池；`PluginRegistry` 那条台账断言只变异了**同批的兄弟**
（`ConfigManager` 改名 → 红），自身未单独变异；剩下 3 条（A75 旧处方字面量的 not.toMatch、
`host_protocol_info` 落在抽取块内、"返回字段…轮 33"）**未单独变异**，它们是本轮新写正文的直接镜像，
下轮若要声称"已证"需补变异。

### 轮 35：iframe 桥的两半各丢一次码，前后端最后一跳不再丢码

**发现路径**：不是新扫出来的，是**上一轮留下的诚实清单里读出来的**。轮 34 收口"文档与门禁
矛盾"时，`@tauron/host/src/errors.ts` 的 `UNWIRED_BOUNDARIES` 注释里正写着一条已定位缺口：
`packages/tauron-plugin-sdk/src/bridge.ts` 把 handler 抛出的码硬换成通用 `SC-9001`，因为
插件侧 SDK 不能依赖 `@tauron/host`（依赖方向）。这句话读起来像"知道缺口、暂无解"，但它其实
给了两条约束同时成立的解：**形态判据不需要词表**。于是本轮把"谁都能碰的那一层"当判据的家。

**断链形状（两半各断一次，缺一不可）**：

| 半边 | 修复前 | 后果 |
| --- | --- | --- |
| 写侧 `PluginBridge` 的失败分支 | `error: { code: 'SC-9001', message: ..., retryable: false }` | 轮 30 在宿主写侧按事实分好的四个码（`E_CALL_PENDING_FULL` / `E_LEASE_EXPIRED` / `E_INVALID_MANIFEST` / `E_STATE_INVALID_TRANSITION`），穿过桥塌成一个；插件作者按 `code` 分支永远只看到"内部错误" |
| 写侧的 `retryable: false` | 硬编码常量 | 与已发布码表**相反**：`SC-9001` 在 `RETRYABLE_ERROR_CODES` 里就是可重试码 |
| 读侧 `PluginContext.invoke` | 局部类型声明了 `error: { code, message }`，实际只 `new Error(result.error?.message)` | 声明了却没人接——桥上带没带码都一样，插件拿到的裸 `Error` 没有 `code` |
| 读侧本地失败 | 超时/取消/销毁都 `new Error(...)` | 取消与超时对插件是同一种东西，无法区分"我撤了"与"宿主没答" |

**修法（四件，都带真实消费者）**：

1. **形态判据搬进 `@tauron/types`**：新增 `APP_LAYER_ERROR_CODE_PATTERN` /
   `isAppLayerErrorCode` / `isCodeLike` / `extractCodeLike`。这一层是两套词表都能触到的最低
   依赖点，`@tauron/host` 与 `@tauron/plugin-sdk` 都用它，因此 `/^SC-\d{4}$/` 全仓只有**一份**
   定义——宿主侧改为再导出（公开面不变，1.0 兼容）。判据只认**形态**：合法集仍各归各表
   （`HOST_ERROR_CODES` / `PluginErrorCode`），否则就又造了第三个事实源。
2. **写侧保码**：新增 `@tauron/plugin-sdk/src/errors.ts`——`codeFromThrown` 先取错误自带的
   `code`（形态合法才算，`code: 500` 这类业务字段不当码），再退消息文本里的码形态，两处都没有
   才落 `SC-9001`；`isAppRetryable` 只对 `SC-####` 查 `RETRYABLE_ERROR_CODES`，`E_*` 一律
   `false`（**不猜**——那份重试四档在 `@tauron/host`，插件包不复制）。
3. **读侧带码**：`PluginBridgeError extends Error`（`code` / `retryable` / `fallbackApplied`），
   `invoke` 的失败拒绝、取消（`SC-2002`）、超时（`SC-2001`）、上下文销毁（`SC-2004`）全部改用它。
   `fallbackApplied: true` 是"这一条原始失败没有任何可用码"的显式痕迹，与轮 33 的 provenance
   同一口径：宁可标记降级，不静默假称有码。
4. **同轮抓到并用同一把尺修掉的两处用词反了**：①`SC-####` 在 TS 注释里被称作"应用层词表"——
   按 `docs/architecture/overview.md` 的层表，`SC-####` 属**框架层**（`tauron-shell` 用的正是
   `PluginErrorCode`），`E_*` 才是应用层宿主底座；同一个词在两层上反着用，下一个人必然接错码表，
   所以按文档口径统一改注释与测试标题（导出名 `APP_LAYER_*` 是 1.0 公开面，**不改**，并在注释里
   写明这层反差）。②`docs/api/plugin-development-guide.md` 一处仍写"TS 侧 `HOST_ERROR_CODES`
   按**声明顺序**比对（wire-gate 门禁）"——门禁实际按**码名集合**比对（`...declaration order
   不属于协议`，V4 A69），同一文档另一处已是新口径，属自相矛盾。

**门禁**：`wire-gate` 新增「轮 35：iframe 桥两侧都必须把错误码带过去，不得塌成通用码」，八段
——①写侧取码链（含 `code: 'SC-9001'` 的反向 needle）②读侧带码拒绝（含裸 `Error` 的反向 needle）
③形态判据只有一份（types 定义、host 再导出、SDK 不得出现第二份正则）④依赖方向（`@tauron/plugin-sdk`
的 `dependencies` 与 `devDependencies` 两处都不得出现 `@tauron/host`，源码四个文件不得 `from '@tauron/host'`）
⑤重试语义不复制第三份表⑥层归属用词（五个文件里
`SC-####` 不得称作应用层、`E_*` 不得称作框架层）⑦两半各有真实测试⑧面向插件作者的文档已写。
同批改动的既有门禁：`UNWIRED_BOUNDARIES` 那条从"未记录 bridge.ts 的 SC-9001 硬换码缺口"改成要求
文档写出**轮 35 收口 + 仍算未接线的理由（依赖方向）**；R2-c 那条对 `isAppLayerErrorCode` /
`isCodeLike` 的断言改为"自家定义**或**再导出"两种形态都认（搬家的代价，公开面未变）。

**对外文档**：`docs/api/plugin-development-guide.md` 新增「跨 iframe 桥的保码行为（轮 35）」
（字段表 + 取码顺序 + 本地失败带哪些码 + 为什么 `E_*` 的 `retryable` 恒 `false`）；
`docs/architecture/app-layer-wire.md` §7.1 的未接线清单段落改写为"缺口已由轮 35 收口，但这两个
边界值**仍**算未接线，理由是归一语义（`translated`/`foreignVocabulary`）仍只在宿主侧"。

**验证（同一轮日志，非记忆）**：

- **门禁确实会咬**——15 个变异全部把 `wire-gate` 打红，还原后两条门禁各自复跑为绿。逐个对应：
  写侧不取码 / `retryable` 退回硬编码 `false` / 读侧不校验码形态 / 读侧退回裸 `Error` /
  宿主不再再导出 / 宿主自定义第二份形态 / SDK `dependencies` 加 `@tauron/host` /
  SDK `devDependencies` 加 `@tauron/host` / SDK 源码 `from '@tauron/host'` /
  SDK 把重试判断改成硬编码码名 / 注释用词退回"应用层 `SC-####`" / 使用文档把
  `fallbackApplied` 改名 / 写侧测试改名 / 读侧测试改名 / 未接线清单删掉轮 35 的收口句
  （最后一条打在 R2-c 门禁上）。变异由脚本施加并逐文件精确还原，还原后用 `grep -c` 逐锚点复核
  无残留（含 `@tauron/plugin-sdk/package.json` 内 `@tauron/host` 命中数为 0）。
  两条**第一次没咬住**的如实记录：`m7` 原本把 `devDependencies` 插在 `sideEffects` 之后，
  JSON 同名键后者覆盖前者，等于没变异——改成往**已存在**的依赖对象里加键后才变红；
  以及变异脚本自身第一版用 `spawnSync('npx', …)` 取不到 vitest 输出，14 条全部报
  `passed=undefined`（是运行器坏了，不是门禁全绿），改为直接 `node …/vitest/vitest.mjs` 并剥 ANSI
  后才有效。**教训**：`GREEN` 与"读不到计数"必须分两栏写，否则运行器故障会被读成门禁失效。
- `pnpm -r --no-bail typecheck` rc=0（轮 35 内曾红过一次：`bridge.test.ts(218,27)` 的局部类型
  没有 `retryable` 字段，补齐该测试的 payload 类型后转绿）。
- `pnpm -r --no-bail test` rc=0，20 个包全部 `Test Files … passed`；本轮点名三条：
  `@tauron/plugin-sdk` 4 files / 46 tests passed，`@tauron/contract-tests` 2 files / 178 tests passed，
  `@tauron/host` 22 files passed。
- `pnpm gates:check` rc=0（7 道语义门禁 + 自测）；孤儿台账读数为
  「15 条未接线宣称复核通过、5 条已接线反例复核通过、自动发现 631 ≤ 基线 635」。
- `pnpm docs:check` rc=0（7 个文档、226 条行号引用）；`pnpm command-surface:check` rc=0
  （85 commands，孤儿 0）；`generate-public-surface-ledger.mjs --check` OK；
  `pnpm version:check` rc=0（26 处 1.1.0）。
- `cargo test --workspace --locked` rc=0：25 个测试套件全部 `test result: ok`，合计 431 passed、
  0 failed（本轮未改 Rust 代码，跑它是为了给"前后端贯通"这句话留一份同轮证据）。

### 轮 36：通知「发出去了」而没人听——推送腿补上接收方，读端 API 不再是孤儿

**发现路径**：轮 35 收口的是**请求-应答**方向（前端 → 宿主 → 前端）。本轮把尺子掉过来量
**推**方向：宿主主动 `emit` 的那些 topic，到底有没有监听方。逐条读 Rust 的 `emit` 点并回 TS 侧
找订阅，得到的不是"都接上了"，而是**一条通知链路两头都断**：`TauriDispatchSink::send` 每次
都把通知 `emit` 到 `NOTIFICATION_TOPIC` 并返回 `Ok`，而全仓**没有任何代码监听这个 topic**；
与此同时读端 `ShellClient.notificationsList()` 只有它自己的单测在调。发出端一路绿灯、
接收方不存在，用户看不看得到通知与"这条命令成功了吗"完全无关——与轮 31 那条
「检查腿打桩且把返回值整个丢弃」是同一类静默断链，只是这次断在事件面。

**断链形状（四处，两处代码两处文档）**：

| 位置 | 修复前 | 后果 |
| --- | --- | --- |
| `NOTIFICATION_TOPIC` 的接收方 | 仓库内零监听方；只有文档写"前端据此刷新通知中心" | 通知永远到不了用户，且不报错——`emit` 返回 `Ok` 被当成"已送达" |
| `ShellClient.notificationsList()` | 零生产消费者（轮 22 的孤儿判据正是这一类） | 环形缓冲只写不读，未读计数只增不减；`dispatchLog` 无人看 |
| 线值的两份潜在出处 | Rust `pub const` 与 TS 侧各写一份字面量，编译期无联系 | Rust 改一个字符，TS 监听静默零命中而发出端仍 `Ok`（本仓 `SHELL_EVENTS` 有过同型事故） |
| `docs/integration/incremental-adoption.md` 的同一事实 | §2.2 与 §5 两处镜像，其中 §2.2 仍写「仓库内**没有任何 `DispatchSink` 实现**、也无人注入」 | 文档与 `tauri.rs` 的实现、R7-3 门禁「DispatchSink 必须有 Tauri 实现」**相反**；照文档读会以为不用接 |

**修法（五件，都有真实消费者）**：

1. **线值单点镜像**：新增 `packages/tauron-host/src/host-topics.ts`，`NOTIFICATION_TOPIC`
   是 Rust `pub const` 的**镜像**并从 `@tauron/host` 公共入口导出。文件头写明它与
   `SHELL_EVENTS` 的区别（后者是同一 webview 内的 DOM `CustomEvent` 名，前者是跨进程的
   Tauri topic），两套混用是本轮之前差一点重演的错。
2. **补上接收方**：`ShellControllerOptions` 新增 `onNotification` / `notificationLimit`
   （**只加字段，不加命令**）。`start()` 订阅信号 → 拉 `host_notifications_list` → 把快照交给
   接入方；`stop()` 退订并推进代际令牌，**晚到的回帧就地退订、晚到的快照不上屏**（轮 25 判据）。
3. **保留轮 11 的安全性质**：事件载荷仍是 `{ id, pluginId, kind, ts }`，**不含正文**
   （`Manager::emit` 广播给所有 webview，正文进广播=跨插件内容泄露）。所以监听端**必须**是
   "信号 → 拉取"，正文的唯一权威出口仍是按调用方身份过滤的 `host_notifications_list`。
   这条不是实现细节而是**契约**：门禁把 Rust 载荷的字段集合钉成封闭集。
4. **突发合并与三个失败出口**：在途只允许一个拉取，期间到达的信号合并为"收尾补拉一次"
   （5 条信号 = 2 次命令，测试逐条数命令数）；订阅失败 `notification.subscribe`、
   拉取失败 `notification.pull`、**接入方回调抛错** `notification.render` 三个 context 各归各，
   全部走 `onError`。其中"渲染错"与"拉取错"的分开是测试逼出来的：写成
   `Promise.resolve(cb(x)).catch(...)` 时 `cb(x)` 的**同步**抛出会归给外层，
   于是排查的人去查命令面而不是渲染层。
5. **生产消费者**：`examples/minimal-app` 接 `onNotification` 把未读快照上屏（按 id 去重 +
   集合上界 200），并向来只用控制器的这条腿，**不**自己监听 topic——绕开控制器就等于绕开
   合并与退订。`notificationsRead` 仍**不**自动调用：自动清未读会把"用户还没看见"变成
   "用户看过了"，那是产品语义不是接线细节。

**同轮顺手改掉的三处"发出=送达"叙事**（都在 `crates/tauron-adapter`，注释级，无行为改动）：
`TauriDispatchSink` 文档第 1 条不再写"前端据此刷新通知中心"而是点名轮 36 的接收方；
`TauriDialogSink` 第 2 条改写成**可核对的事实**——仓库内没有任何代码监听该 topic，
权威结论在命令返回值自身（`simulated` / `reason` / `native: false`）；R9 菜单块把
"前端按既有 Tauri 事件监听消费"换成**逐 topic 监听方清单**（菜单与通知=有监听方，
`DIALOG_DEGRADED_TOPIC` / `DEEP_LINK_NATIVE_TOPIC`=**零监听方**，best-effort 诊断）。
V4 的 W11 行同步为逐 topic 口径（改在同一行内，不动 226 条行号引用）。

**门禁**：`wire-gate` 新增「轮 36 通知推送腿（信号 → 拉取 → 上屏，逐段可核对）」八段——
①线值两侧同值 + TS 全仓只许 `host-topics.ts` 出现该字面量 + 必须从公共入口导出；
②订阅的是常量、回调**不得吃载荷**（反向 needle）、正文只能来自带 limit 的拉取、
`onNotification` 入参必须是快照类型；③Rust 广播载荷字段集合封闭为
`[id, kind, pluginId, ts]`（多一个正文字段即红）；④代际令牌 / 晚到回帧就地退订 /
合并补拉 / 三个失败出口且渲染错不得并进拉取错；⑤示例接的是控制器且不许自己监听；
⑥`notificationsList` 与 `onNotification` 的**消费者集合逐文件冻结**（回到孤儿即红）且宿主
测试名在册；⑦README、V4 W11 行、Rust 监听方清单三处镜像同口径，并扫描 `docs/` 全部
Markdown 反向钉住「本仓没有任何 `DispatchSink` 实现、`notify_sink` 无人注入」这句旧事实，同时要求真实装配确有
`notify_sink.set(TauriDispatchSink::new(app))`；⑧`@tauron/host` 源码里出现的每条 `host_*`
字面量必须在 `docs/api/command-surface.md` 里（**这条腿不许偷偷加命令**）。

**验证（同一轮日志，非记忆）**：

- **门禁确实会咬**——20 个变异全部把 `wire-gate` 的「轮 36 通知推送腿」打红，还原后该组
  8 段复跑为绿（`passed=8`）。逐段对应：①TS 镜像线值改一个字 / TS 侧多出第二份字面量 /
  常量不再从入口导出；②监听回调开始吃载荷 / 拉取不再带 limit / `onNotification` 入参不再是
  快照类型；③Rust 广播载荷多一个正文字段；④`stop()` 不推进代际令牌 / 晚到回帧不再就地退订 /
  合并标记设了不补拉 / 渲染错并进拉取错；⑤示例摘掉 `onNotification`；⑥控制器不再调
  `notificationsList` / 宿主测试名被改；⑦README 少一个失败出口 / V4 的 W11 行不再写零监听方 /
  文档再写一遍那句旧事实 / Rust 逐 topic 清单被删 / 真实装配不再注入 sink；⑧用一条命令面里
  不存在的命令。两条如实记录：`m13` 的锚点与 ② 共用同一行，因此它**先**打在 ②（回调与拉取
  同处一段）而非 ⑥——判据仍成立，但不要把它记成"⑥单独咬住"；以及**门禁在同一轮把我自己
  刚写进施工单的一句话判红了**：⑦ 的描述行把 sink 类型名与那句旧判词写在同一行，正好命中
  它自己规定的反向 needle。改法是引用旧事实原句（「本仓没有任何 `DispatchSink` 实现、
  `notify_sink` 无人注入」）而不是重述它。**这条不是巧合而是规则**：文档要描述一个被禁的口径，
  就得引用它、并显式标为旧事实，否则描述本身会变成第二处宣称。
- `pnpm -r --no-bail typecheck` rc=0；`pnpm -r --no-bail test` rc=0，20 个包全绿——点名三条：
  `@tauron/host` 22 files / 436 tests passed（含本轮新增的 11 条通知腿测试），
  `@tauron/contract-tests` 2 files / 186 tests passed（wire-gate 单文件 165 条），
  `@tauron/plugin-sdk` 4 files / 46 tests passed。
- `pnpm gates:check` rc=0（7 道语义门禁 + 自测）；孤儿台账读数为
  「15 条未接线宣称复核通过、5 条已接线反例复核通过、自动发现 631 ≤ 基线 635」。
- `pnpm docs:check` rc=0（7 个文档、226 条行号引用）；`pnpm command-surface:check` rc=0
  （**85 commands**：底座 61 / 运行时 22 / 安装 2，孤儿 0，未归类 0）；
  `generate-public-surface-ledger.mjs --check` OK（85 public commands, no orphan metadata）；
  `pnpm version:check` rc=0（26 处 1.1.0）。命令面本轮**未新增一条**：通知腿只加控制器字段。
- `cargo test --workspace --locked` rc=0：41 条 `test result: ok`，合计 1540 passed、0 failed。
  本轮 Rust 侧只改注释与文档口径，跑它是为了给"前后端贯通"这句话留一份同轮证据。

### 轮 37：菜单/托盘点击「按文档接就收不到」——共享路由表补上两条 lane 与接收方

**发现路径**：轮 36 量的是"宿主 `emit` 有没有人听"，收口时顺手把逐 topic 监听方清单写进
`tauri.rs`，其中"菜单/托盘点击=有监听方"那一行**当时就是假的**（清单里没有任何 `listen`）。
本轮从这条假行回查实现，得到的不是"漏了一个订阅"，而是**文档指着一条根本不通的路**：
四处叙述都写着菜单点击"经 `host_events_*` 总线回传、`host_events_drain` 取件"，而
`TauriMenuSink` 从第一天起就是 `AppHandle::emit`。两条通道在仓库里长得极像（都叫 topic、
都有 `host_events_*` 命令），照文档接线的人会在一条永远为空的队列上等一次点击，且不报错——
命令返回值确实成功了。继续往下查又挖出两条：`TauriTraySink` **从未**登记过任何路由，
托盘右键菜单点了必然没反应；`ShellClient` 那六条菜单/托盘方法既无生产消费者也无测试。

**断链形状（五处，三处代码两处文档）**：

| 位置 | 修复前 | 后果 |
| --- | --- | --- |
| 点击回传的通道口径（`lib.rs` / `tauri.rs` / `shell-client.ts` / 生成的 `command-surface.md`） | 四处同写「经 `host_events_*` 总线、`host_events_drain` 取件」 | 与实现相反；照它接线=静默零命中。`command-surface.md` 是**生成物**，所以假话有一个上游、四个镜像 |
| `TauriTraySink` 的路由 | 只 `tray.set_menu(...)`，一次都没往路由表登记 | 托盘点击被丢弃且无从排查（监听查不到 id 就 `None`，best-effort 连日志都没有） |
| 路由表的归属 | 每个 sink 各存一份私有 `Mutex<HashMap>` | Tauri 只有**一张**全局菜单监听表，一次点击只能落进一个查询点；私有表结构上就决定了"托盘那条 lane 没人登记" |
| `MENU_CLICK_TOPIC` 的定位 | 本轮初稿四处把它说成"宿主会替你补的那个值" | 宿主**不回填**：`event: None` 一个帧都不发。那种措辞会让接入方以为不填也有回传（同轮被自己的门禁判红，见下） |
| `ShellClient` 六条菜单/托盘方法 | 零消费者 + 零测试（轮 22 的孤儿判据） | 上面三条坏掉时，仓库里没有任何东西会变红 |

**修法（六件，都有真实消费者）**：

1. **共享路由表落地为独立模块** `crates/tauron-adapter/src/menu_routes.rs`：
   `MenuRouteTable` 按 `MenuLane`（`AppMenu` / `TrayMenu`）分道，`replace()` 整条 lane
   替换（累加会让旧 id 一直可点）、`lookup()` 命中即返回 `(lane, topic)`、
   `menu_routes_of()` 只为**显式带 `event`** 的项建路由。放独立文件有两个硬理由：
   `lib.rs` 的项数预算（域归属门禁）已顶格；更重要的是本模块**不依赖 `tauri`**，
   于是 `cargo test --workspace`（不带 feature）就能测它——放在 `tauri.rs` 里这些用例
   永远进不了默认测试集。
2. **一个监听、两条 lane**：`TauriMenuSink::new` 仍只注册**一个**全局 `on_menu_event`
   （vendored tauri 2.11.6 里 `TrayIcon::register` 与 `AppHandle::on_menu_event` 推的是
   同一个 `manager.menu.global_event_listeners`，事件循环把 `MenuEvent` 发给全部监听，
   所以第二个"专用托盘监听"会把同一次点击 emit 两遍）。命中后 `emit` 的 `source` 取自
   lane，两条 lane 各替换各的：`host_menu_reset` 只让 `AppMenu` 失效，`tray_remove` 只让
   `TrayMenu` 失效。
3. **登记时机 = 生效时机**：托盘 lane 只在 `tray.set_menu()` 真的挂上之后替换，
   `remove()` 只在真的移除后清空；`build_and_route` 失败不登记（菜单没建成却留着路由，
   前端会收到一次"点了不存在的项"的回传，比静默更难解释）。
4. **TS 侧同词表**：`MenuClickFrame` / `MenuClickSource` / `MENU_CLICK_TOPIC`
   （Rust `pub const` 的镜像，`host-topics.ts` 单点、包入口导出）。线值字面量在 TS 全仓
   只许出现一次，`MenuLane::as_str()` 与 `MenuClickSource` 的词表由门禁逐字比对。
5. **生产消费者**：`examples/minimal-app` 第 9 节——先 `listen(MENU_CLICK_TOPIC)` 再建菜单，
   按 `source` 与 `id` 上屏（`menu-minimize` / `tray-minimize` 真调 `windowMinimize`），
   五个按钮走完六条命令；`Unsupported` / `applied:false` / `itemCount` 三种结论分开显示。
6. **文档在唯一事实源改**：先改 Rust `///`（`command-surface.md` 由它生成，改生成物会被
   `--check` 打回），再改 TS 类型与方法注释，最后是 README / roadmap / 接入指南三处镜像。
   roadmap 那句"`host_menu_on_select` 从未注册、这条回调链仍是缺口"的旧判词改为**指向
   真实形状**（路由表 + 事件通道），并明确它不是缺口、也不必按那个提案名补命令。

**同轮被自己的门禁判红的两条**（都值得记成规则）：

- **① 描述被禁口径的句子本身会命中反向 needle——连"逐字列出被禁字符串"也会**。段③ 的禁令
  最初我是按"行内含 菜单 ∧ `host_events_drain`"设计的——那我刚写下的"它**不经**
  `host_events_*` 总线"立刻会被判红。改为只禁**形状**（三条旧叙事的原话形状，字面清单的
  唯一权威出处是 `wire-gate.test.ts` 的段③ 本体；本文**故意不复述**，理由见下），引用旧事实
  时用「」并显式标为旧口径。本轮最后一次 `pnpm format` 之后复跑门禁，段③ 又把我自己判红了
  一次：就是这一小节里"逐字列出被禁形状"那行。**形状禁令天然无法区分引用与宣称**，
  所以结论比轮 36 更硬：被禁的字符串在文档里连作为反例出现都不允许，只能指向门禁文件。
  这是同一条规则的第三次触发（轮 36 第一次、本轮设计时第二次、本轮收尾第三次）。
- **② 我自己写下的新假话：把约定值说成"宿主会替你补的那个值"**。四处注释称
  `MENU_CLICK_TOPIC` 会被宿主回填进没填 `event` 的菜单项，
  而 `menu_routes_of()` 跳过 `event: None` 的项、宿主从不回填。修法是把措辞在唯一事实源
  改成"只有显式填了 `event` 的项才发帧，宿主不补默认 topic"，重新生成命令面，并给段③
  **加一条新的反向 needle**（两条形状的字面清单只存在于 `wire-gate.test.ts` 段③ 本体，
  本文不复述——不复述的理由就是本节这条；比对前折叠全部空白，跨行的 `///` 也抓得到）。
  变异验证时它确实把这条新判红。
- **③ 两条 needle 曾被证明太松**（第一次变异跑出 2 个"未变红"）：`replace(\s*crate::MenuLane::TrayMenu,`
  会被 `set_menu` 顶包，删掉 `tray_create` 的登记仍然是绿的；README 只查 `MenuClickFrame`
  时，删掉解码那一行也还是绿的（`import type` 那行仍在）。两条都改成盯**各自那行**
  （create 必须匹配 `spec.menu.as_ref().map(...).unwrap_or_default()`，README 必须含
  `payload as MenuClickFrame`）。**结论：没有变异证明的 needle 不算门禁。**

**门禁**：`wire-gate` 新增「轮 37 菜单/托盘点击腿（路由 → emit → listen，逐段可核对）」八段——
①线值两侧同值 + TS/Rust 各自全仓只许一个文件出现该字面量 + 必须从公共入口导出 +
`MenuLane::as_str()` 与 `MenuClickSource` 词表逐字相等；②全局监听只注册一次、路由查询点唯一、
两条 lane 的登记/清空各自钉住具体形状，并反向钉住 `self.routes.lock()` / `routes_for_handler`
（回到私有路由表即红）与两个 sink 的装配句；③四条形状禁令（三句旧通道口径 + 那句"宿主会替你
补值"，字面清单只写在门禁本体里，文档不复述——见上①）；
④宿主 `emit` 载荷字段与 `MenuClickFrame` **同集合**且封闭为 `[id, native, source]`
（多一个正文字段即红）；⑤监听方集合冻结为示例、且示例只挂一次，六条方法的消费者
集合逐文件冻结（回到只剩单测即红，扫描时先剥注释，文档里的示例调用不算消费者），并钉住
**订阅失败的出口**——那段 catch 必须把失败上屏到 `menu-out`，摘掉 catch、或把它只丢进
console，即红；
⑥Rust 路由表 `#[test]` 数量与用例名 + TS 三条用例名同时在册（单边绿不算绿）；
⑦README / 命令面三行 / roadmap / 接入指南同口径，且 roadmap 不许再把已存在的回调链写成缺口；
⑧`host_menu_on_select` 不许在 Rust 里成为真函数、也不许出现在命令面文档里。

**验证（同一轮日志，非记忆）**：

- **门禁确实会咬**：16 次定向变异把「轮 37」八段逐段打红，还原后 `passed=8` 复跑为绿
  （`/c/tmp/r37-mutation3.log`：`BASELINE: rc=0, 轮 37 passed=8` → `变异：16 次，按段变红 16 次`
  → `RESTORED: rc=0, 轮 37 passed=8`，日志里 `RED-OK` 恰 16 行、没有 `NOT-RED`）。
  第一次跑只有 13/15 咬住，那两条就是上面③的松 needle，补强后复跑全咬。
  **第 16 次是被门禁自己逼出来的**：示例那条腿当初写成裸 `void backend.listen(...)`，
  而 `TauriBackend.listen()` 原样返回 Tauri 的 Promise，事件系统不可用时会拒绝——
  届时只剩一条 unhandled rejection，"前端接上了回传"又成了核对不了的承诺，正是本轮在治的
  同一形状。补上出口 + 补 needle 后，摘掉 catch 的变异确实把⑤打红。
  顺带一条同构教训：prettier 把这条链拆成 `void backend` / `.listen(...)` / `.catch(...)` 三行后，
  变异脚本里两条 ⑤ 的锚点命中数变成 0（脚本按"锚点须恰好命中 1 次"直接判 SKIP-BAD-ANCHOR，
  不会误报成"门禁没咬"）——needle 会腐坏，变异锚点一样会。
- **同轮收尾读数**（`/c/tmp/r37-close.log`）：`pnpm -C examples/minimal-app run typecheck` rc=0、
  `wire-gate.test.ts` 整文件 **173 passed (173)**、`prettier --check` 本轮两个改动文件全绿。
- **降级形态在浏览器里真点过**：以 vite 起示例（非 Tauri 环境），§7 渲染正常，五个菜单/托盘按钮
  逐个触发，`#menu-out` 每次都如实打出"当前宿主未注册 `host_menu_set`（底座形态或缺省装配），
  无法操作菜单/托盘"这类 fail-closed 事实，console 除 6 条能力警告外无新增噪声。
  **边界要说清**：**原生点击链路（真 Tauri webview 里宿主 `emit` → 前端 `listen` 收帧）本轮没有跑过**——
  没有启动 Tauri 应用。已证的只有 Rust/TS 单测、八段门禁、类型层与降级形态的浏览器证据。
- `cargo test -p tauron-adapter --features tauri,plugin-install --locked` rc=0；
  `cargo test --workspace --locked` rc=0：**41 条 `test result: ok`、1546 passed、0 failed**
  （轮 36 同口径是 1540——多出的 6 条正是 `menu_routes.rs` 的六个用例）。
- `pnpm -r --no-bail typecheck` rc=0（前置：先 `pnpm --filter @tauron/host build`——示例
  消费的是 `dist` 类型，不 build 就会把"新导出的 `MENU_CLICK_TOPIC` 不存在"演成红）；
  `pnpm -r --no-bail test` rc=0，**20 个包全绿**。点名三条：`@tauron/host` 22 files /
  **439 tests**（含本轮新增的 3 条点击腿用例）、`@tauron/contract-tests` 2 files /
  **194 tests**（轮 36 同口径 186，差的 8 条就是本轮八段）、`@tauron/plugin-sdk` 4 files / 46 tests。
- `pnpm gates:check` rc=0（7 道语义门禁 + 自测各一条）。两份读数照抄同轮日志：
  孤儿台账「15 条未接线宣称复核通过、5 条已接线反例复核通过、自动发现 631 ≤ 基线 635」；
  域归属「37 个 impl 类型 / 174 个方法、84 组顶层 fn / 360 个函数、19 个域、
  lib.rs 顶层条目 263 已封顶」+ self-test「6 个变异形状各按规则判定」。
  本轮**没有抬任何既有文件预算**：`menu_routes.rs` 作为新文件按 R5 单独封顶 4 条，
  lib.rs / tauri.rs 仍停在原值；`fn menu` 一族从 2 个文件变 3 个（高点 2→4）是那两个
  新访问器落在新文件的既有形态，不是清理成果。
- `pnpm docs:check` rc=0（7 个文档、226 条行号引用）；`pnpm command-surface:check` rc=0
  （**85 commands**：底座 61 / 运行时 22 / 安装 2，孤儿 0，未归类 0，无代码层判定 9）；
  `generate-public-surface-ledger.mjs --check` OK（85 public commands, no orphan metadata）；
  `pnpm version:check` rc=0（26 处 1.1.0）。命令面本轮**未新增一条**：点击腿靠的是路由表 +
  既有事件通道，roadmap 那个 `host_menu_on_select` 提案名仍然只存在于提案里。
- 两条过程记录（都是门禁抓到**我自己**）：一次 Edit 的锚点只取了一行文档注释，结果把
  `impl crate::MenuSink for TauriMenuSink` 粘成了两份——域归属门禁按"同一 key 两个域"
  直接判红才暴露，那是编辑事故不是设计问题；另一次在 `lib.rs` 顶部插 5 行，让 226 条
  行号引用里 57 条集体漂移，`docs:check` 判红后用官方 `--fix`（只重锚唯一最近解）才复位。
  **这两条都不该靠人眼发现**：一个抓到了类型重复，一个抓到了引用失效，正是门禁该有的形状。

### 轮 38：topic 词表两侧同源——深链接腿的裸字面量与两条无镜像的诊断帧

**发现路径**：轮 36 为通知 topic 立了"线值只许写在一个文件"，轮 37 又为菜单点击立了一次——
两条都是出事之后**逐条**补的。本轮把 Rust 侧全部 `pub const *TOPIC*` 与 TS 侧摊开对照，
同一形状还在两处：深链接投递腿在 TS 侧压根没有常量（监听点写着裸字面量，宿主改一个字就
静默零命中，而发出端 `emit` 照旧成功）；两条诊断帧（降级对话框与原生深链接注册）连 TS
镜像都没有，接入方只能手打线值。两处都不会让任何既有测试变红——这正是它们能活到今天的原因。

**断链形状（两处，同一病根：词表没有全量规则，只有逐条补丁）**：

| 位置 | 修复前 | 后果 |
| --- | --- | --- |
| `deep-link-client.ts` 的投递腿 | 裸字面量直接写进 `listen(...)` | 线值在 TS 侧没有单点，宿主改名即静默零命中，无人可查 |
| 降级对话框 / 原生深链接注册两条诊断帧 | Rust 有 `pub const`，TS 侧无镜像、无导出 | 接入方想核对只能手打字符串；与 Rust 漂移时同样静默 |

**修法（把逐条规矩升级成全量表）**：

1. `host-topics.ts` 从"两条 topic 的家"升级为**全部五条 A 层 topic 的唯一 TS 镜像**
   （新增 `DEEP_LINK_TOPIC` / `DIALOG_DEGRADED_TOPIC` / `DEEP_LINK_NATIVE_TOPIC`），
   全部从 `@tauron/host` 入口再导出；每条镜像的文档块各自写清**仓库内有无监听方**的
   事实与**权威结论落在哪里**（两条诊断帧零监听，结论在命令返回值里，不在某条订阅上）。
2. `deep-link-client.ts` 改从 `./host-topics.js` 导入 `DEEP_LINK_TOPIC` 并按常量监听，
   裸字面量清零。
3. 门禁把"每条线值在 TS 非测试源里**恰好出现一次**、且只落在它的声明文件"覆盖到全量五条
   （含同文件内二次出现的计数），B 层（`HOST_SETTINGS_CHANGED_TOPIC`，镜像在
   `@tauron/app-plugin-sdk`）同规则；C 层三个片段（调用/事件 topic 的前后缀）必须**不是**
   完整 topic（含 `://` 即红），防止有人把完整通道塞进豁免层。
4. 监听方集合逐条冻结：三条真有接收方（通知 / 菜单点击 / 深链接），两条诊断帧确实**零监听**。
   零监听是写进文档的**事实**而不是缺陷遮掩，钉成断言是为了"以后有人补了监听、口径没跟上"
   时这条会红，而不是口径悄悄过期。

**门禁（`wire-gate` 新增「轮 38 topic 词表两侧同源」六段）**：①Rust 常量全集 ↔ 三层表
逐字相等，且 C 层不许出现完整 topic；②A 层五条线值两侧逐字相等、镜像集合与表相符、
五条都从入口导出；③线值在 TS 非测试源恰好出现一次且只落在声明文件，B 层从 SDK 入口导出；
④深链接腿不许含裸字面量、必须从词表模块导入并按常量监听；⑤每个 topic 的监听方文件集合
逐条冻结（两条诊断帧为空集）；⑥宿主侧逐 topic 清单与**每条镜像各自的文档块**都写明零监听
事实与权威落点。14 次定向变异逐段变红。

**同轮被自己的门禁判红 / 被工具腐坏的三处**：

- **①"恰好一次"规则先红了我的文档注释**：新镜像的文档块里顺手引用了自己的线值，段③ 的
  "全仓恰好一次"当场判红——注释引用线值也算第二份。修法不是在门禁开洞，而是注释改为
  **不复述线值**（只写事实与权威落点）；这条从此适用于今后所有词表注释。
- **②第一次变异 13 次只红 11 次**：一处只查**文件集合**（同文件里再写一遍照样漏），
  一处用整文件级 needle（被相邻常量的注释顶包）。补强成"全局出现次数计数"与"按声明位置
  反向找最近文档块（非贪婪正则会从上一条常量的块起锚，照样顶包）"后 14/14。
  **没有变异证明的 needle 不算门禁**——轮 37 起的规则再次生效。
- **③格式化把手写的门禁/锚点腐坏了两处**：`pnpm format` 把入口的整组导出收成一行，
  轮 38 变异②的锚点命中归零，脚本按"锚点须恰好命中 1 次"判 `SKIP-BAD-ANCHOR`；
  收尾 `cargo fmt` 又把轮 37 的 `tray_set_menu` needle 折行打红（`cargo fmt --check`
  初跑 rc=1、7 个 hunk 全是轮 37 手写 Rust 的格式债）。修 needle 为带 `\s*` 的容空白
  形状后复跑 16/16。**结论**：needle、变异锚点、门禁钉的具体代码形状，三者都会被格式化
  工具腐坏，复跑前都要重查。

**读数（全部照抄同轮日志）**：

- `pnpm format:check`、`pnpm build`、`pnpm -r --no-bail typecheck`、`pnpm -r --no-bail test`
  全部 rc=0，20 个包全绿；点名两条：`@tauron/contract-tests` **200 tests**（轮 37 同口径
  194，差的 6 条就是本轮六段）、`@tauron/host` **439 tests**。
- `pnpm gates:check` rc=0（7 道语义门禁 + 自测；孤儿台账自动发现 **631 ≤ 基线 635**）；
  `pnpm docs:check` rc=0（7 个文档、**226** 条行号引用）；`command-surface:check` rc=0
  （**85** 条，未新增）；`generate-public-surface-ledger.mjs --check` OK；`version:check`
  rc=0（**26** 处 1.1.0）。
- `cargo test --workspace --locked` rc=0：**41 条 `test result: ok`、1546 passed、0 failed**
  （在 `cargo fmt --all` 之后的树上重跑，与格式化前读数一致）。
- `cargo fmt --check` 初跑 rc=1（**7 个 hunk**）；`cargo fmt --all` 只改空白后 rc=0。
  轮 38 变异证明 **14/14**（基线 6/6 → 14 次定向破坏逐段变红 → 恢复后 6/6）；轮 37
  变异证明复跑 **16/16**（基线 8/8 → 16 次定向破坏逐段变红 → 恢复后 8/8）。

### 轮 39：能力可见性——「主窗特权命令对插件主体不可见」此前只在注释里成立

**发现路径**：`isAvailable()` 的 JSDoc 与 `host_capabilities` 相关注释都宣称插件侧
`available('host_registry_admin')` 为 `false`，摊开实现却是三层各自成立、合起来不成立：
`cmd_host_capabilities`（Rust）不带调用方参数、返回**构建级**命令集（宿主不替插件 webview
少报，属设计如此）；SDK 侧 `isAvailable()` / `capabilityMatrix()` 的判据只有「注册与否」
一条（`backend.capabilities().has(command)`），插件主体拿到的主窗特权命令全是 `true`；
而原有的"插件视图"契约测试是**循环夹具**——把 `CAPABILITIES.filter(c => c.consumer ===
'plugin')` 预筛后喂给 mock，断言"插件只看到 20 条"是夹具自己筛出来的，不是被测代码判出来的。

**断链形状**：

| 环节 | 宣称 | 实际 |
| --- | --- | --- |
| `isAvailable()` / `capabilityMatrix()` | 主窗特权命令对插件主体不可见 | 只看注册位，不看调用方主体 |
| `cmd_host_capabilities`（Rust） | — | 构建级全集（无调用方参数；宿主侧不区分主体） |
| 插件视图契约测试 | 验证"插件只看到 20 条" | 夹具按 `consumer === 'plugin'` 预筛，循环论证 |
| `ShellClient.supports()` | 插件侧能力判断入口 | 只答注册与否（保留——属构建/协商事实） |

**修法**：

1. 过滤落在 SDK `isAvailable()`（唯一能力判定入口），双判据 = 注册位 ∧ 主体白名单：
   `cap.consumer !== 'main-window' || backend.principal().kind === 'main-window'`。
   写成白名单而不是黑名单，`invalid` 主体（畸形 label）不会被误按主窗放行。宿主侧不改：
   构建级返回是刻意设计，过滤职责归 SDK 调用侧。
2. `capabilityMatrix()` 不再自带第二份过滤，逐条委托 `isAvailable()`；全仓非测试 TS 源里
   `.kind === 'main-window'` **恰好出现一次**（单点判定，防第二份主体逻辑长出来）。
3. 契约测试去循环：全量 30 条命令注册 + `pluginId` → 恰 20 条可见；主窗主体 → 全量反证；
   畸形主体 → 特权命令仍不可见（`capabilities.test.ts` 12 → 13 条）。
4. 口径划清：`supports()` **只答注册与否**（构建/协商事实，feature-gated 命令默认构建下为
   `false`）；可见性判断走 `isAvailable()` / `capabilityMatrix()`；真正的执行判定仍在宿主
   代码层（`require_main_window`）。README 同步写明两个探测入口的差异。

**门禁（`wire-gate` 新增「轮 39 能力可见性（主窗特权命令对插件主体不可见）」四段）**：
①`isAvailable` 双判据 needle（注册检查在前、白名单式主体判定）；②矩阵逐条委托
`isAvailable`，且 `.kind === 'main-window'` 在非测试 TS 源恰好一次、他处为零；③契约测试
钉住全量夹具（30 条注册 + `pluginId: 'com.example.x'`）、循环式预筛（`consumer === 'plugin'`
取材）缺席、畸形主体与主窗对照两组反证在场；④三处口径文档（能力表注释写明**构建级**与
**可见性快照**、`shell-client.ts` 写明**注册与否**、host README 写明插件主体**一律报**
`false`）逐条 needle。9 次定向变异逐段变红。

**同轮自查**：门禁文件里新写的注释把 `packages/*/src` 写进了块注释——`*/` 提前终止注释、
prettier 当场解析失败。改语义等价措辞（"两棵树 `src/` 下"）后通过；教训与轮 38 同向，
**门禁文件自己也在腐坏半径内**（格式工具、注释语法都算）。

**读数（全部照抄同轮日志）**：

- `pnpm format:check`、`pnpm build`、`pnpm -r --no-bail typecheck`、`pnpm -r --no-bail test`
  全部 rc=0：`@tauron/contract-tests` **204 tests**（两文件；`wire-gate.test.ts` **183**，
  轮 39 过滤跑 **4 passed**）、`@tauron/host` 22 个测试文件全绿（`capabilities.test.ts`
  **13 tests**）。本轮为纯 TS 改动（未动 `crates/`），cargo 套件未重跑。
- `pnpm gates:check` rc=0（7 道语义门禁 + 自测；孤儿台账自动发现 **631 ≤ 基线 635**）；
  `pnpm docs:check` rc=0（7 个文档、**226** 条行号引用）；`command-surface:check` rc=0
  （**85** 条 = 底座 61 / 运行时 22 / 安装 2，未新增）；`generate-public-surface-ledger.mjs
  --check` OK（85 public commands）；`version:check` rc=0（**26** 处 1.1.0）。
- 轮 39 变异证明 **9/9**（基线 4/4 → 9 次定向破坏逐段变红 → 恢复后 4/4；9 条锚点全部
  一次命中），脚本 `C:\tmp\round39-mutation-proof.mjs`、日志 `C:\tmp\r39-mutation.log`。

### 轮 40：升级装配腿落地——市场下载/安装「装配即真」，授权档位补两席

**发现路径**：轮 28/30/33 三轮的结论都指向同一处断链——`tauron-distribute` 的
`UpgradeRunner`（真状态机：Download/Extract/Swap 分相位推进 + 超时/取消控制 + 失败分类）
在仓库里**没有任何生产消费者**（`create_upgrade_runner(` 构造点 0 处）；而
`host_market_download` / `host_market_install` 两条命令**只有模拟一条实现**，即使宿主侧
装配了真实下载器也没有「装配位」把腿接进链路。文档口径此前已先纠正（轮 28/30 小节的
「断点在装配腿」注记），本轮把代码补齐、并把两条命令按「改状态」归位。

**断链形状**：

| 环节 | 宣称 | 实际 |
| --- | --- | --- |
| `UpgradeRunner`（distribute） | 真状态机、可被宿主装配 | 仓内 0 个构造点，纯库面 |
| `host_market_download` / `host_market_install` | 未装配时如实模拟、装配后为真 | 只有模拟一条实现，装配位不存在 |
| 授权与审计 | 改状态的特权命令过 `admin_gate` + 进审计表 | 两条市场命令非特权、不在审计表（6 条） |
| 下载 / 验签实现 | 单一事实源 | `run_phases` 内联 `sha256_file` 校验，无共享 SSOT |

**修法**：

1. **接线缝**：host 侧新增 `UpgradeInstaller` trait（`native_supported()` / `download()` /
   `install()`），经 `SubstrateState` 注入；缺省装配仍是模拟——`native_supported()` 为
   `false` 时两条命令走 `_simulated` 孪生，行为与旧桩一致（`simulated: true`、
   `update_state_simulated = true`）。唯一仓内生产消费者是 `crates/tauron-adapter` 的
   `DistributeUpgradeInstaller`（V7 §7 风险表钉住「消费者恰一处、构造点恰一处」）。
2. **分派孪生**：`cmd_market_download` / `cmd_market_install` 各自变为分派器（按
   `native_supported()` 二选一），wired 腿**先执行真实效果再翻 provenance**——
   `download()` 走 `download_bounded` 边界下载、`install()` 走 runner 的 `run()`；效果
   落地后才写 `simulated: false`、`update_state_simulated = false`。失败路径**零账本**：
   ledger 不动、provenance 不变，不会出现「没做事却记了账」。
3. **授权升级**：两条命令升为 `Privileged`（`_as` 包装改走 `admin_gate`），
   `AUDITED_ADMIN_COMMANDS` 6→8；`cmd_market_check` 保持桩 + `require_main_window`
   （只读探测，刻意不进审计表以免挤掉真实授权事实）。
4. **SSOT 提取**：distribute 抽出 `download_bounded`（边界下载）与 `verify_package`
   （`sha256_file` → `PackageHashMismatch` → `verifier.verify`）两个自由函数，
   `run_phases` 改调它们；哈希不符走类型化错误，不再有第二份内联校验。
5. **口径边界**：缺省装配仍如实模拟；网络下载器 / 验签器 / 健康检查 / 重启提供者仍由
   装配方注入——不对外宣称「应用更新器开箱可用」，只宣称「装配即真、未装配如实模拟」。

**门禁（三道脚本门禁 + `wire-gate` 四段升级，14 次定向变异逐条按声明门禁变红）**：

1. `check-simulated-never-commits.mjs`：桩不许悄悄提交——桩孪生必须带模拟标注、分派器
   与孪生必须成对、钉住的测试名在场、wired 腿的效果先于 provenance 翻转（6 个源文件）。
2. `check-success-requires-effect.mjs`：wired 腿的 success 必须以真实效果为前提——必须
   含 `/upgrade_installer\.(download|install)\(\)/` 调用（9 个源文件）。
3. `check-upgrade-file-hash.mjs`：`run_phases` 调用点次序 + 反向 SSOT needle +
   `verify_package` 块次序（1 个源文件）。
4. `wire-gate` 四段同步升级：轮 29 分派 needle（含 `native_supported()` 与模拟孪生
   needle）、轮 33 逐相位×标记 needle（写与 marker 计数相等）、V7 §7 风险表（消费者
   集合 `toEqual(['crates/tauron-adapter/src/lib.rs'])`、`create_upgrade_runner(` 恰
   一次、`pub trait UpgradeInstaller` 在场）、每条命令的授权档位一致（特权档 pin 表
   给两条市场命令各留一席）。

**同轮自查（格式化腐坏半径再次生效）**：

- `pnpm format:check` 抓到两处手写文件（`wire-gate.test.ts`、
  `check-simulated-never-commits.mjs`）未过 prettier；先 `prettier --write` 再复跑
  门禁三件套，字符串字面量未动、rc=0。
- `cargo fmt --check` 初跑 rc=1（手写 Rust 的格式债），`cargo fmt --all` 后 rc=0；
  **fmt 把全部行号挪位**，`pnpm docs:check` 当场判红 40 条行号引用（上一批修复之后
  fmt 又漂了一次），脚本重锚 40 条后复跑绿（226 条），并抽点核对
  `record_admin_audit` / `cmd_settings_set` / `cmd_recover_trial_enable` 三条重锚落在
  函数定义行而非文档注释。
- 因此本轮在 `cargo fmt` **之后**的树上重跑了完整变异证明（不是引用 fmt 前的读数）。
  教训并入轮 38/39：needle、变异锚点、行号引用全在格式化工具的腐坏半径内。

**读数（全部照抄同轮日志）**：

- `pnpm format:check`、`pnpm lint`、`pnpm -r --no-bail typecheck`、`pnpm -r --no-bail test`
  全部 rc=0：`@tauron/contract-tests` **204 tests**（`wire-gate.test.ts` **183**）、
  `@tauron/host` **440 tests**（22 个文件）；`command-surface:check` 初跑 rc=1（档位
  变化未同步），`pnpm command-surface:gen` 重新生成后复跑 rc=0。
- `pnpm gates:check` rc=0（7 道语义门禁 + 自测；孤儿台账自动发现 **629 ≤ 基线 635**；
  domain-ownership **40 个 impl 类型 / 183 个方法、85 组顶层 fn / 365 个函数、19 个域、
  lib.rs 顶层条目 272 已封顶**）；`pnpm docs:check` rc=0（7 个文档、**226** 条行号引用）；
  `command-surface:check` rc=0（**85** 条 = 底座 61 / 运行时 22 / 安装 2，孤儿 0、未归类 0、
  无代码层判定 9）；`generate-public-surface-ledger.mjs --check` OK（85 public commands）；
  `version:check` rc=0（**26** 处 1.1.0）。
- `cargo fmt --all -- --check` rc=0；`cargo clippy --workspace --all-targets --locked --
  -D warnings` rc=0；`cargo test --workspace --locked` rc=0：**41 条 `test result: ok`、
  1552 passed、0 failed**（轮 39 口径 1546，差的 6 条就是本轮新增的
  `round40_market_wiring_tests`）。
- 轮 40 变异证明 **14/14**（基线 7/7 → 14 次定向破坏按声明门禁逐条变红 → 恢复后 7/7），
  脚本 `C:\tmp\round40-mutation-proof.mjs`、日志 `C:\tmp\r40-mutation.log`；`cargo fmt`
  之后在 `C:\tmp\r40-battery2.log` 内复跑一次，同结论。

### 轮 41：跨重启孤儿扫描（V7 §7 startup orphan sweep）——durable 回收台账 + 平台存活探测（只探测不杀）

**发现路径**：V7 §7 Process 行是审计链上最后一个写明「未落」的工程项
（「未落：startup orphan sweep（Batch 5'）」）。轮 26/27 的两条腿（失败入队 + 让位封口）
都建立在**同一进程还活着**的前提上——「唯一驱动腿是主窗轮询 `host_resource_stats`」。
宿主一退出，内存里的待重试队列连同每条 pid/原因一起蒸发：上一轮"杀不掉"的进程成了
**跨重启黑洞**——没人探测、没人销账、没人能查（连"曾经失败过"这个事实都没了）。

**断链形状**：

| 环节 | 宣称 | 实际（轮 41 前） |
| --- | --- | --- |
| 跨重启回收 | 终止失败"有下文" | 队列纯内存，宿主退出即蒸发；那些 pid 从此无人再提 |
| pid 台账 | 可查的失败事实 | 不存在（`terminalRecords` 也只在内存环里） |
| 平台探测 | —— | 不存在；重启后无从知道旧 pid 还在不在 |
| 装配腿 | 恢复数据目录已就位（A101/设置域） | 回收域没接：谁也没在目录里开过账 |

**修法（五件）**：

1. **持久化台账 `reap-ledger.json`**（恢复数据目录内，与 `admin-audit.json` 同域不同
   文件）：`DurableEnvelope` 信封（schema `tauron.reap-ledger/1` + 校验和 + 代数单调）；
   载荷 = 待重试池镜像（`pluginId/pid/attempts/reason`）+ 终态证据环。**脏即写**：
   `enqueue_pending` / `push_terminal` / 重试推进 / 扫描四处标脏，`Registry` 在四个生产
   变更点（状态离场回收、卸载/清除、重试驱动、显式移除）后自动落盘——**锁内取快照、
   锁外写盘**（`write_durable` 原子替换：tmp + fsync + rename），写失败计
   `ledgerWriteFailures`（不静默，也不打断回收本身）。
2. **平台存活探测**（`crates/tauron-host/src/liveness.rs`，模块与入口 `pub(crate)`）：
   Unix `kill(pid, 0)`（`ESRCH`=Gone、`EPERM`=Alive）；Windows
   `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess`
   （`ERROR_INVALID_PARAMETER`=Gone、`STILL_ACTIVE`=Alive）；`pid == 0` 哨兵与不支持
   平台一律 Unknown。只读、无副作用。
3. **启动扫描，安全封口是核心**：读取失败（撕裂/篡改/IO）→ `Err` → 装配期
   `AssemblyError::ReapLedgerRejected` **拒绝启动**——这份台账是"上一轮是否有孤儿"的
   唯一跨重启来源，静默重置成空账等于把孤儿洗白。读回的条目进**独立** `restart_reaps`
   池（`retry_pending_reaps` 永不触碰它），`sweep_restart_reaps` 逐条定性：`Gone` →
   销账（`sweepResolved` + 终态证据）；`Alive` → `sweepSurvivors` + 证据（明写"跨重启
   无法验明进程身份，刻意不盲杀"）；探测不可用 → `sweepUnknown` + 证据。**扫描体里没有
   任何终止调用**（门禁钉死）：跨重启 pid 可能已被无关新进程复用，盲杀比留孤儿更糟；
   正常退出路径已由 Job Object `KILL_ON_JOB_CLOSE` / 进程组 containment 覆盖，剩下的
   逃逸残骸交操作者按证据处置。
4. **装配腿**：`with_substrate_and_spawner` 固定注入 `set_system_pid_probe()`；
   `recovery_data_dir` 存在时 `open_reap_ledger(dir)`（读回 → 扫描 → 落一份"已扫描"
   快照；首启空账也落——"扫描过"本身是可查事实）。生产宿主该目录由 Tauri
   `app_config_dir` 兜底（`tauron.rs` 既有装配），无盘宿主（未配置目录）时落盘是
   空操作，不 panic、不伪造写失败。
5. **口径边界**：宿主退出途中仍无驱动腿（那次失败必进台账，下次启动可查、可销账）；
   `Alive` / `Unknown` 只留证不盲杀——"跨重启自动清理"是本次**刻意不做**的事。

**门禁**：`wire-gate.test.ts` 新增「轮 41：跨重启孤儿扫描只探测不杀，台账可校验且
读不开不洗白」一段（分池铁律：重试腿体内不得出现 `restart_reaps`；扫描体无 `.kill(`、
三分支各自计数 + "不盲杀"证据措辞；`configure_reap_ledger` 走 `decode_durable` 且拒绝
`Integrity`；注册表 `open_reap_ledger` + 原子写；装配腿三 needle；TS 四字段 + 线上两
needle；九条新测试名；V7 §7 Process 行同步钉住"轮 41"与"不盲杀"），主检查 ①-④
段与 V7 风险表段同步扩围。

**测试（十二条）**：租约表级六条——`reap_ledger_round_trips_across_a_restart_scan`
（往返 + 分池 + 销账 + 代数单调 + 证据读回不虚增计数）、
`restart_sweep_never_kills_even_when_the_probe_says_alive`（核心封口：记录型终止器在
扫描后仍零调用）、`unknown_or_missing_probe_is_evidence_not_a_guess`、
`torn_or_tampered_ledger_is_rejected_not_reset`（垃圾字节 + 合法 JSON 改载荷两种）、
`empty_sweep_still_records_a_scanned_ledger`、`unconfigured_table_yields_no_snapshot_and_no_fake_failure`；
生存探测三条——`own_process_probes_alive_and_zero_is_unknown` /
`a_pid_above_the_platform_maximum_probes_gone` / `probe_trait_impl_delegates_to_the_platform`；
注册表级一条 `reap_ledger_is_flushed_and_swept_across_registry_restarts`（真路径
Uninstall → 自动落盘 → 重启扫描 → 三次重启不复活）；adapter 两条
`assembly_opens_the_reap_ledger_on_the_recovery_dir` / `assembly_rejects_a_torn_reap_ledger`。

**同轮自查与读数**（全部照抄同轮日志）：`cargo fmt --all --check` rc=0、
`cargo clippy --workspace --all-targets --locked -- -D warnings` rc=0、
`cargo test --workspace --locked` rc=0：**1,564 passed / 0 failed**（41 个 result 块；
`tauron-host` lib **427**、`tauron-adapter` lib **295**）。轮 41 定向变异 **30 次逐段
变红**（`C:/tmp/round41-mutation-proof.mjs`：基线绿 → 30/30 → 恢复绿；含 V7 §7
Process 行"轮 41 / 不盲杀"两条）。`pnpm format:check`／`lint`／
`command-surface:check`（85 条，孤儿命令 0，未归类 0）／`version:check`／
`gates:check`（7 门 + self-test 全绿；孤儿面自动发现 **629 ≤ 基线 635**，
Adapter-Domain-Ownership 40 impl/183 方法、85 组/365 fn、19 域、lib.rs 顶层 272
全部 ≤ 基线）／`docs:check`（7 个文档，**226 条引用**；本轮 47 条符号锚重锚 +
1 条路径锚修正后 rc=0）全 rc=0。契约测试 **205 passed**。`pnpm verify` rc=0
（64 段 Done）。**同轮复核补记（负载抖动实录）**：两枚电池并行时
`call_admission_is_released_by_cancel_timeout_and_owner_teardown` 抖红过一次
（426/427）——该用例整条挂在 1ms TTL 上，`call_begin` 与 `call_cancel` 两条相邻
语句之间一次调度抖动就会让取消腿拿到 `E_CALL_TIMEOUT`。已加固为"timeout 腿独立
1ms 注册表（睡够再 GC，方向对负载不敏感）、cancel/teardown 腿回缺省 TTL"；
单跑与全量复跑均 427/427。

### 轮 42：A85 便携路径五维收口——zip 侧与宿主共用同一份判定

**发现路径**：A85 台账行长期停在「未落（核心维度缺）」：`portable_path.rs` 只挡
绝对路径 / `..` / 盘符 / UNC / NUL 五类**逃逸**，而便携路径的另五个口子
（Windows 保留设备名、非 NFC 归一、尾点尾空格、长度越界、集合级大小写冲突）
在 Unix 上静默通过、只在目标平台咬人。Batch 4' 的「五维、zip 侧同步」措辞
挂了几轮，判定却一直是最初的形状检查。

**断链形状**：

| 环节 | 宣称 | 实际（轮 42 前） |
| --- | --- | --- |
| 保留设备名 | 便携 manifest 可在 Windows 释放 | `nul.txt` 落到设备、`aux/` 整棵目录失败——清单与落盘不一致 |
| Unicode 归一 | 条目名只有一种规范形态 | `é` 的合成式与分解式各自成条，大小写不敏感 FS 上还互相覆盖 |
| 尾点 / 尾空格 | 解包后条目名与清单一致 | Windows 落盘静默剥离 `name.` / `name `，出现清单里不存在的名字 |
| 长度 | —— | 段 >255 字节（POSIX `NAME_MAX`）、相对路径 >1023 字节（macOS `PATH_MAX`）目标 FS 直接拒绝 |
| 条目集 | zip 侧常量校验能防冲突 | 逐条合法、两两却在大小写不敏感 FS 上打成一团（`Icon.png` / `icon.png`） |

**修法（三件）**：

1. **宿主五维判定续写**（`crates/tauron-host/src/portable_path.rs`）：
   `validate_portable_relative` 在原五类逃逸之后追加四维，错误面各自具名——
   `ReservedName` / `NotNfc` / `TrailingDotOrSpace` / `TooLong`；新增集合级
   `validate_portable_entry_set` 承担第 5 维（ASCII 折叠冲突 → `CaseCollision`）。
   判定保持**词典序、平台中立**：段切分按 `['/', '\\']` 双分隔符（不依赖运行宿主的
   `std::path` 方言，同一输入在任何宿主给出同一判定）；22 个 `WINDOWS_RESERVED_STEMS`
   同时挡裸名与带扩展名形式（`nul.txt` 的 stem 是 `nul`）；NFC 用
   `icu_normalizer` 的 `ComposingNormalizerBorrowed::new_nfc()`（`url`→`idna` 早已把
   该实现带进依赖图，工作区只开 `compiled_data` 特性——缺省特性要拉的 `utf16_iter`
   不在锁图内——零新增锁文件包）；长度上界 `MAX_SEGMENT_BYTES = 255` /
   `MAX_RELATIVE_BYTES = 1023`。`.` / `..` / 空段跳过：逃逸判定仍归既有的
   `components` 检查，两层不重复表态。
2. **zip 侧委派同一份判定**（`crates/tauron-market/src/lib.rs`，`signing` 特性内）：
   `sanitize_entry_path` 逐条过 `validate_portable_relative`，`validate_zip_constants`
   对整份条目集过 `validate_portable_entry_set`——**签名/常量阶段就拒**，而不是等
   解压到一半再按平台随机失败。市场→宿主依赖方向经 `signing`（`dep:tauron-host`）已有
   先例（`package_signature.rs`），无环；非 `signing` 构建的常量校验保持原形状。
   既有 `end_to_end_malicious_path_traversal` 升级为 cfg 分流断言：`signing` 开着时
   同一载荷**更早**在常量层被拒（分层前移的有意行为变更，注释已写明）。
3. **门禁与记账**：wire-gate 新增轮 42 段（五维错误面具名、22 设备名常量、NFC 调用、
   尾缀判定、长度常量、折叠调用、zip 侧两处委派、7 条测试名、plan A85 行状态）；
   CHANGELOG 新增条目。

**测试（七条新增）**：宿主 5 条——`rejects_windows_reserved_device_names_in_any_case_or_extension`、
`rejects_trailing_dot_or_space_that_windows_would_strip`、
`rejects_non_nfc_forms_but_accepts_the_precomposed_one`（分解式拒绝 / 合成式与 CJK 放行）、
`rejects_segments_or_paths_over_the_portable_length_bounds`、
`entry_set_rejects_ascii_case_collisions_and_duplicates`；
市场 2 条（`signing` 门后）——`zip_side_rejects_five_dim_paths_before_extraction`、
`zip_side_rejects_case_colliding_entry_sets`。

**同轮自查与读数**（全部照抄同轮日志）：`cargo fmt --all --check` rc=0、
`cargo clippy --workspace --all-targets --locked -- -D warnings` rc=0、
`cargo test --workspace --locked` rc=0：**1,569 passed / 0 failed**（41 个 result 块；
`tauron-host` lib **432**、`tauron-adapter` lib **295**）；feature 腿
`cargo test -p tauron-market --features signing` **78 passed**、
`-p tauron-adapter --features plugin-install` **316 passed**、
`-p tauron-adapter --features tauri,plugin-install` **350 + 1 passed**（4 ignored），
全部 rc=0。轮 42 定向变异 **24 次逐段变红**（`C:/tmp/round42-mutation-proof.mjs`：
基线绿 → 24/24 → 恢复绿；含五维具名、22 设备名表、NFC 归一器、尾缀判定、两条长度常量、
双分隔符切分、ASCII 折叠、zip 侧两处委派与 signing 门、七条测试名、台账行翻篇）。
`pnpm format:check`／`lint`／`command-surface:check`（85 条，孤儿命令 0，未归类 0）／
`version:check`（26 处 1.1.0）／`gates:check`（孤儿面自动发现 **629 ≤ 基线 635**、
Adapter-Domain-Ownership 全量 ≤ 基线 + self-test 各形状判定）／`docs:check`
（7 个文档，**223 条引用**，本轮无需重锚）全 rc=0。契约测试 **206 passed**。
`pnpm verify` rc=0（64 段 Done）。

### 轮 43：A83 尾段——破坏性管理操作的令牌链与两域 TTL 的可信时间

**发现路径**：A83 落地时只覆盖 install 域，Batch 4' 的「token 覆盖 uninstall/purge/update；
TTL 从墙钟改走 A100 的可信时间」在台账里挂了几轮——`host_registry_admin` 的
uninstall/purge 在生产档只需主窗身份即执行，与 install 域「预览→一次性令牌→提交重核」
的承诺不对称；而 install 域自己的令牌 TTL 由 `mint_install_review` 与消费判定直接取
`unix_time_seconds()`（墙钟），A100 的 `TrustedTimeProvider` 不参与——回拨宿主时钟
就能拉长或掐掉审批窗。

**断链形状**：

| 环节 | 宣称 | 实际（轮 43 前） |
| --- | --- | --- |
| 管理域 uninstall/purge | 破坏性操作须用户审阅后才能执行 | `host_registry_admin` 一路直执行 `cmd_registry_admin`；令牌链只在 install 域存在 |
| 令牌 TTL 的时间源 | TTL 建立在可信时钟上 | install 域铸发与判定都取墙钟；A100 provider 即使装配也不参与 |
| 前端审批形状 | —— | 管理面与壳层卸载腿直连 `host_registry_admin`：`PluginManagerStore` 没有可注入的确认钩子，「用户取消」与「操作失败」在返回值上不可区分 |

**修法（三件）**：

1. **管理域令牌链**（`crates/tauron-adapter/src/lib.rs`）：新增 `AdminReviewToken`
   （camelCase + `deny_unknown_fields`；绑 `plugin_id` / `op` / 预览版本 / `issued_at` /
   `expires_at` / `nonce`）、`ADMIN_REVIEW_TTL_SECS = 600`、`MAX_ADMIN_REVIEWS = 64`；
   `mint_admin_review` 用 `review_now` 取时间；`validate_admin_review` **先按 nonce
   摘除再比对**（伪造/重放不留可用条目），随后过期判定 + op/plugin_id/预览版本三重
   事实重核——全部发生在注册表迁移之前。旧面无令牌入口 `cmd_registry_admin_as` 在
   `admin_review_required`（feature+Production）档直接拒 uninstall/purge；新面
   `cmd_registry_admin_reviewed_as` 的 `preview: true` 只铸令牌（零注册表副作用）、
   令牌提交才执行。wire 返回改为判别形 `RegistryAdminResponse`（internally-tagged：
   `executed` 分支既有 `TransitionOutcome` 字段全留顶层、只多 `kind`，老读者不破；
   `review` 分支携带预览事实与一次性令牌）。`HostAdminOp` 增 `preview` / `reviewToken`
   两字段，都带 `#[serde(default)]`——旧载荷（无这两键）反序列化仍合法，85 条命令面
   冻结不破。

2. **两域 TTL 统一走 A100**：新增 `review_now` / `review_unexpired` 两个咽喉——provider
   在则取 `trusted_time`（非 Trusted 失败关闭）、None+Production 直接拒、仅开发/测试档
   无源回落墙钟；install 域的 `mint_install_review` 与消费判定、管理域的铸发与校验全部
   改走它们。生产档运行期被降级（`demote`）则下一次预览/提交直接 `E_AUTH_DENIED`；
   生产就绪门（`TRUSTED_TIME_REQUIRED`）在启动期就要求 provider 存在且 Trusted。

3. **前端审批链**（`packages/tauron-host` / `packages/tauron-ui`）：`events.ts` 镜像
   `AdminReviewToken` / `RegistryAdminOp.preview?` / `RegistryAdminOp.reviewToken?` /
   判别联合 `RegistryAdminOutcome`；`HostClient.registryAdmin` 返回判别联合。
   `PluginManagerStore._destructiveAdminOp`：preview → 可注入确认钩子（默认
   `defaultAdminConfirm`，无 DOM 环境失败关闭）→ 带令牌提交；用户取消返回
   `{ ok: false, cancelled: true }`（与失败区分）；老宿主（预览即执行、返回无
   `kind: 'review'` 判别）不重复提交。壳层 `_uninstallPlugin` 同形（预览 →
   `window.confirm` → 提交令牌 → 通知列表刷新）。`wire-gate` 新增「轮 43」段
   （Rust 形状 / wire 形状 / TS 两侧 / 测试在场 / 台账行）。

**测试**：Rust 侧（`tauron-adapter`，轮 43 新增/改造）——`admin_review_preview_binds_facts_and_commits_once`、
`admin_review_rejects_op_mismatch_and_stays_consumed`、
`admin_review_version_drift_forces_a_fresh_preview`、
`admin_review_protocol_rejects_non_destructive_mixing`、
`production_destructive_admin_op_requires_the_review_path`、
`admin_review_ttl_follows_the_trusted_provider`、
`demoted_clock_after_startup_fails_closed_for_admin_review`、
`install_review_ttl_follows_the_trusted_provider`、
`production_without_trusted_time_is_rejected_at_startup` /
`production_with_suspicious_clock_is_rejected_at_startup`（启动期失败关闭）与
`production_preview_fails_closed_without_trusted_time_in_substrate_builds`
（缺省构建下 None+Production 分支的真实可达性证明）；TS 侧——`plugin-manager.test.ts`
六条（预览→确认→提交 / 取消→cancelled / 无 DOM 失败关闭 / 钩子抛错按取消 / 老宿主不
重复提交 / purge 同链）与 `shell-controller.test.ts` 三条（两步链 / 用户拒绝不提交 /
老宿主不重复提交）。

**同轮自查与读数**（全部照抄同轮日志）：`cargo fmt --all --check` rc=0、
`cargo clippy --workspace --all-targets --locked -- -D warnings` rc=0、
`cargo test --workspace --locked` rc=0：**1,574 passed / 0 failed**（41 个 result 块；
`tauron-host` lib **432**、`tauron-adapter` lib **300**）；feature 腿
`cargo test -p tauron-market --features signing` **78 passed**、
`-p tauron-adapter --features plugin-install` **326 passed**、
`-p tauron-adapter --features tauri,plugin-install` **360 + 1 passed**（4 ignored），
全部 rc=0。轮 43 定向变异 **21 次逐段变红**（`C:/tmp/round43-mutation-proof.mjs`：
基线绿 → 21/21 → 恢复绿 + 树完整性终检 7/7 文件一致；含 mint 落账、validate 三重
事实重核、两域 TTL 判定改读可信时间、失效/重放/版本漂移、wire 判别联合与 TS 两侧、
台账行）。`pnpm format:check`／`lint`／`command-surface:check`（85 条，孤儿命令 0，
未归类 0）／`version:check`（26 处 1.1.0）／`gates:check`（孤儿面自动发现
**630 ≤ 基线 635**、Adapter-Domain-Ownership lib.rs 顶层 **281** 已封顶 + self-test
各形状判定）／`docs:check`（7 个文档，**242 条引用**）全 rc=0。契约测试 **207 passed**。
`pnpm verify` rc=0（64 段 Done；20 段 vitest 共 1,931 passed）。

**同轮复核补记**：其一，复跑电池时市场腿以缺省特性执行（64 条），signing 门后的
安全用例缺席；核查发现**任何 workflow 都从未执行过它们**——`cargo test --workspace`
走缺省特性、`-p tauron-adapter` 不跑 market 自己的用例、`--all-features` 只挡编译
（与 wasm_delivery 同类，正好落在写有该教训的 job 里）。`ci.yml` 已补
`cargo test -p tauron-market --features signing --locked` 一步；上方 78 passed 即
该命令在冻结树上的新鲜日志。其二，本轮变异证明的前两次运行都撞到 Windows 写入锁
（errno -4094；一次与并发编辑抢锁自造、一次外部瞬时锁），恢复写入失败分别把一条
台账行与 C4 变异留在盘上——台账行残留被复跑电池的 wire-gate 抓红，残留本身成了
门禁咬合力的现场证据。脚本已改为「重试 + 回读校验」写盘、逐文件兜底恢复与终检
快照比对；第三次运行 21/21 逐段变红、树完整性 7/7，唯复跑绿阶段一条 cargo 腿的
链接被占用文件挡了一次（LNK1104，同族瞬时锁），重跑该腿即绿。其三，两枚 TTL 用例
在 `review_now` 回落墙钟的变异下曾**不变红**（时间源基线≈墙钟，针太弱）——已改为
「时间源回拨 6 小时后 pin `issued_at` 到时间源读数」，重证明红。

### 轮 44：A94 内存门禁收口——读数搬到真实安装流，整包内存入口删净

**发现路径**：A94 的缺口有三条——① `verify_tpkg(&[u8])` / `verify_tpkg_file` 两个整包
入口零调用者却仍公开（谁再调它就把整包读回内存）；② RSS 门禁错位：
`performance-budgets.json` 的 `process.maxPeakRssKb=32768` 断言的是 `perf_probe`（wire
往返压测）的 VmHWM，与「内存最重的安装路径」毫无关系；③ 把探针接到真实安装链时抓出
**真缺口**：安装的激活腿 `collect_plugin_activation_records` 用 `std::fs::read` 把每个
文件整读进内存再算摘要——验签/解包早已流式，摘要计算却仍随包内最大文件线性增长。

**修法（四件）**：

1. **流式摘要**：`ContentIdentity::from_reader`（`crates/tauron-host/src/activation.rs`，
   固定 64 KiB 缓冲），`from_bytes` 委托它；安装腿改为 `File::open` + `from_reader`——
   全链（验签 / 解包 / 摘要）不再有整读点。安装完成后逐资产服务复核（A84）的
   HMAC 密封清单不受影响：密封记录仍按新摘要值落账。
2. **删整包入口**：`verify_tpkg` / `verify_tpkg_file`（`crates/tauron-market/src/package_signature.rs`）
   删除（零调用者）；流式 `verify_tpkg_reader[_with_time]` 单源保留。
3. **门禁移位**：`contracts/performance-budgets.json` 的 `process` 块换成 `installStream`
   （`minPayloadBytes: 67108864`（64 MiB）、`maxPeakHeapKib: 16384`、`maxPeakRssKb: 32768`）；
   新增 `install_stream_probe` example（tauron-adapter；~96 MiB 载荷＝3×32 MiB `Stored` blob
   + manifest/js/html）走**生产同一条链**（`cmd_registry_install_preview_as` →
   `cmd_registry_install_reviewed_as`），双读数：计数全局分配器的堆高水位（`fetch_max`，
   跨平台）+ Linux `/proc/self/status` VmHWM；打包阶段本身也流式（64 KiB 块生成 + 增量
   哈希），VmHWM 不被构建阶段污染。`check-performance-size.mjs`：wire 断言保留；新增
   `--install-probe` 必需参数与三道安装断言（status=installed / payload ≥ 下限 / 堆、RSS
   双上界；RSS 缺读数＝失败而非跳过）。`perf_probe` 的 VmHWM 读数删除（装饰性未断言读数）。
4. **CI 与反向门禁**：performance-size job 加 release 构建与运行
   （`cargo build -p tauron-adapter --example install_stream_probe --release --locked
   --features plugin-install` → `--install-probe=/tmp/tauron-install-probe.json`）；
   `wire-gate` 轮 44 段钉住以上全部形状，含「探针载荷常量（3×32 MiB）乘积 ≥ 预算下限」的
   结构保证——防「探针包缩小后门禁按自报数空转」这条绕过路径。

**同轮读数**（照抄同轮日志）：缺省特性（空壳 fallback）与 `plugin-install` 两模式
`cargo check` rc=0；Windows release 实跑（第二次运行）：

```text
{"entryCount":6,"installElapsedMs":2052,"packageBytes":100664213,"payloadBytes":100663575,"peakHeapKib":74,"peakRssKb":null,"pluginId":"com.install.probe","schemaVersion":1,"status":"installed"}
```

（`payloadBytes=100,663,575 B` ＞ 64 MiB 下限 ＞ 32 MiB RSS 上界；`peakHeapKib=74`
即计数分配器整个安装过程的高水位；`peakRssKb=null` 是 Windows 无 VmHWM 的如实缺席。）
门禁脚本五连跑：绿样例 exit 0（`failures=[]`）；真实 Windows 输出 exit 1 且唯一失败
`install peak RSS was not observed on the Linux performance runner`（RSS 断言是真断言、
只在 Linux runner 上可过）；三条红样例各自命中具名失败——`install peak heap 20480KiB >
16384KiB`、`install payload 1048576B < floor 67108864B`、
`install stream probe did not complete: status=failed (boom)`。

**同轮自查与读数（电池全绿，照抄同轮日志）**：`cargo fmt --all` 与 clippy（`-D warnings`）
rc=0；`cargo test` 六腿全绿——workspace **1,574 passed / 0 failed**（41 个 result 块）、
`-p tauron-market --features signing` **78 passed**、`-p tauron-adapter --features plugin-install`
**326 passed**、`-p tauron-adapter --features tauri,plugin-install` **361 passed + 4 ignored**；
定向变异 **10/10** 逐段变红（`round44-mutation-proof.mjs`：C1 双侧把激活摘要回退整读——
先红 wire-gate 文本针、再红 release 运行期堆读数；S1–S8 盖整包入口复活 / 预算值被放大 /
RSS 缺读放宽成跳过 / CI 参失踪 / 台账行回写「部分」/ `perf_probe` 读数复活 /
`fetch_max→fetch_min` / `from_bytes` 自调），恢复后复跑绿、终检「全部 9 个文件与快照一致」；
`wire-gate` **187 passed**（contract-tests 全量 2 文件 208 passed），`pnpm verify` exit 0；
`prettier --check` 全绿、eslint rc=0、`version:check` 26×1.1.0、
`command-surface` 85 命令 / 0 孤儿、`gates:check` 绿（孤儿自动发现 628 ≤ 635 基线）、
`docs:check` 7 文档 / 239 引用全绿。

### 轮 45：A65 Profile V2——Tier↔Bundle↔命令子集三张表与命令面同源门禁

**发现路径**：V4 §87 立了「Tier 与形态正交」的判词（Tier S != Window / Tier E != Desktop /
Tier P != Marketplace UI），§87.2 钉了 10 个 bundle 名，但全仓无 `ProductTier`/
`CapabilityBundle`——判词没有机器载体。TS 的 `AuthTier`（`capabilities.ts:10`）是**授权档位**，
与「能力档」不是一个语义；没有任何表回答「哪些命令属于哪个能力面」。

**修法（三件）**：

1. **三张表落地**（`contracts/tier-bundles.json`）：① tiers——S/E/P 三档深度，requires
   严格单调（S={core} ⊂ E={core, plugin-runtime} ⊂ P={+marketplace}）；② bundles——
   §87.2 的 10 个能力/形态捆绑各带**命令子集**与落地状态：85 条命令完全划分
   （core 28 / desktop-ui 27 / plugin-runtime 20 / marketplace 7 / observability 3）；
   ③ surfaceBundles——五个形态捆绑中仅 desktop-ui 有命令面，mobile-ui/headless/service/
   remote-client 与 enterprise-policy 如实标 roadmap 空表（不白挂命令）。
2. **一致性门禁**（`scripts/check-tier-bundle.mjs`，即表的机器读者）：I1 表结构、
   I2 与生成命令面同源（`**61+22+2 条**` 标记与解析计数相等，防解析空转）、
   I3 完全划分（遗漏/重复/未知名各自具名失败）、I4 状态诚实（已落⇔非空；requires
   只引已落 capability）、I5 深度严格单调、I6 **正交性机器证明**
   （requires ∩ surfaceBundles = ∅——§87 判词从修辞变断言）。
3. **布线**：`gate:tier-bundle` 进 `gates:check` 链 + ci.yml 独立步骤（与其余语义门禁
   同形）；`wire-gate` 轮 45 段钉表形状（3/10/5 键集、requires 字面、状态诚实、
   脚本在场、链与 CI 在场、A65 台账行翻篇与轮 11 清单划线）。

**同轮自查与读数（照抄同轮日志）**：门禁首跑与恢复后复跑均为
`Tier-Bundle gate OK: 3 tiers / 10 bundles / 85 commands 全部单属一个 bundle；S⊂E⊂P 严格单调；requires 与 surfaceBundles 零交集。`
（exit 0）；`gates:check` 全链绿（新门禁位末尾）。定向变异 **12/12** 逐段变红
（`round45-mutation-proof.mjs`：M1–M6 语义六不变量各自具名失败——未知名 / 重复 /
遗漏 / 形态必备 / 单调断裂 / roadmap 假宣称；M7–M12 布线钉——gates:check 链、CI 步骤、
台账行、requires 字面、脚本失败口径、10 键集），恢复后复跑绿、终检「全部 5 个文件与
快照一致」。`wire-gate` **188 passed**（新增轮 45 段）；`prettier --check`、eslint、
`version:check`（26×1.1.0）、`command-surface:check`（85 命令）、`docs:check` 全 rc=0。

### 轮 46：A66 零 Surface 就绪——ReadinessSet 六组事实 + 真跑 conformance 三条

**发现路径**：V4 §87.3 的「headless 可零 Surface 运行」只命中 `headless` 注释
（`production.rs:261`、`adapter/lib.rs:297`）；`ProductionReadiness` 是生产**安全检查**
清单（tls/audit/trust 之类），回答不了「应用此刻是否就绪」；CI 的
`Minimal substrate profile` 与 `最小底座依赖门禁` 是**构建门禁**，不是「零 IPC 面
启动 → 健康」的运行时证明。就绪判定散落为各子系统布尔量，没有单一可求值集合。

**修法（三件）**：

1. **`tauron-host/src/readiness.rs`**：`ReadinessFact { id, ready, reason }`（工厂
   `ready()` / `blocked()`）+ `ReadinessSet` 六组必备事实（kernel / contract /
   required_providers / required_runtimes / required_surfaces / product_predicates）。
   `headless()` 构造把 Surface 组留成**空集**——空集不阻塞是零 Surface 形态的核心
   语义；`evaluate()` 把每个未就绪事实**具名**收进 `blockers`（有 reason 时
   `id: reason`，没有时裸 id，禁止静默跳过）；`health()` 桥回 A103 的 `HealthReport`
   （就绪 → `ready()`；未就绪 → `alive_but_not_ready(blockers 合并)`），
   liveness/readiness/degradation 分离由此有了单一入口。
2. **真跑 conformance 三条**（`crates/tauron-host/tests/v4_host_conformance.rs`，
   `cargo test --workspace` 与本地电池均实跑）：①**零 Surface 启动→健康**——底座
   服务图（contract → policy → runtime）`startup_order` 可排序且无环，
   `ReadinessSet::headless(...)` 求值 `ready == true`、`blockers` 空、
   `health().can_accept_work()`；②**声明了 Surface 就必须兑现**——往
   `required_surfaces` 推入 `blocked("main-window", "window-not-mounted")` 后
   `blockers == ["main-window: window-not-mounted"]`、`Readiness::NotReady` 但
   `Liveness::Alive`（缺 Surface 是就绪问题不是存活问题），诊断字段同步携带具名
   reason；③**kernel 未就绪零 Surface 不豁免**——`kernel: substrate-graph-invalid`
   具名拦下。另配 lib 单测钉「无 reason 的未就绪事实报裸 id」的口径。
3. **导出与门禁**：`lib.rs` `pub use readiness::{ApplicationReadiness, ReadinessFact,
   ReadinessSet};`；`wire-gate` 轮 46 段钉模块形状（`pub struct ReadinessSet` /
   `required_surfaces` / `blockers` / `pub fn headless(`）、三条 conformance 测试名
   在场、导出行、A66 台账行翻篇与轮 11 清单划线、CHANGELOG 轮 46 条目在场。

**同轮自查与读数（照抄同轮日志）**：`cargo test -p tauron-host --locked` 全绿
（lib **433 passed**；`v4_host_conformance` **10 passed**——既有 7 条 + 新 3 条
`conform_*` 在列，0 failed）；孤儿公共 API 台账复核通过
（自动发现 **630 ≤ 基线 635**，新增 `pub` 项未破预算）。定向变异 **15/15** 逐段变红
（`round46-mutation-proof.mjs`）：语义五条走 cargo 实跑判红——forced-ready 静默跳过
（`不得静默跳过`）/ 具名 reason 丢失（`window-not-mounted` 不符）/ 裸 id 口径破坏
（lib 单测名报红）/ Surface 组不参与求值（声明被静默跳红）/ headless 假非空
（零 Surface 前提断言红）；布线十条走 wire-gate——ReadinessSet 形状三件
（结构名 / Surface 组字段 / blockers 字段）、headless 入口、lib 导出行、
conformance 测试名、台账行翻篇、轮 11 划线、CHANGELOG 条目。**变异第一遍抓出真实
假绿**：`/### 轮 46：/` 未锚行首时 `#### 轮 46：` 以子串命中（W8 未变红）——已改
`/^### 轮 46：/m` 并复跑至 15/15；恢复后四命令复跑绿、终检「全部 5 个文件与快照
一致」。`wire-gate` **189 passed**（新增轮 46 段）。

**完整电池快照（14 腿，承 ci.yml + 本地惯例）**：首跑 13 腿即绿、`docs:check` 抓出
一条**真实漂移**——本轮给 conformance 文件加导入行把全文件后移一行，A87 行的
conformance 引用旧锚 153 随之落到空行；改锚 155（fn 行）后复跑绿；随后 14 腿
全绿：`fmt:rust:check`、`lint:rust`（clippy `-D warnings`）、`format:check`、`lint`、
`version:check`、`command-surface:check`、`gates:check`（tier-bundle 位尾在场）、
`pnpm verify` rc=0；`cargo test --workspace --locked` **1578 passed**（0 failed，
含 host conformance 10/10）；market `--features signing` **78**、adapter
`plugin-install` **326**、adapter `tauri,plugin-install` **361**；契约测试 vitest
**2 files / 210 passed**；`docs:check` 7 文档 **239 引用** OK；gates 链读数：orphan
**630 ≤ 635**、domain-ownership **lib.rs 281 已封顶**、tier-bundle OK。`pnpm verify`
**20 包 / 1934 tests**。

### 轮 47：A91 故障边界扩到 events/approval/registry——三条子系统各自的确定性修复语义

**发现路径**：A91 判据「一个子系统 panic 不得升级为全宿主风险」在 Batch 1' 里点名了
settings 之外的三条子系统（事件/审批/注册表），但生产面只有 settings 一条边界
（轮 11 拆出 `ensure_ready` / `record_panic` 两段式后沿用到今天）。事件总线槽位
panic 一次，宿主此后所有事件命令都可能继续踩坏的一半状态；approval / registry 同理。
而且三条子系统**没有共同的修复形态**——逐个回答"这个子系统的确定性修复是什么"正是
本轮的落地内容。

**修法（三件）**：

1. **三条边界同一套机制**：`run_subsystem_boundary` 三段式（ensure_ready →
   catch_unwind → record_panic，锁不跨闭包）+ 修复前置判定 `begin_subsystem_reconcile`
   处理三态（Ready=幂等无操作、悬空 `Reconciling`=确定性重做、Faulted/Quarantined=
   推进）。接线：7 条事件命令、4 个令牌消费点（mint/consume × 用户域/install 域）、
   5 条注册表命令。
2. **三条修复语义各自诚实**：①events=会话态清零且**保留 topic 声明**——本轮最重的
   一个设计决定：`EventBus::default()` 整重置会把装配期写入的 topic 声明一起清掉，
   而发布端对未声明 topic 只**静默丢弃并计数**、不报错（防存在性探测），清声明等于
   让修复动作把可工作的消息面悄悄打哑；新增 `EventBus::reset_session_state()`
   只清会话态（订阅/审批/队列/排序/统计），host 单测钉住声明存活。②approval=
   重新 preview 清空两域令牌表（一次性瞬态物，清空让"消费了一半"的令牌彻底失效，
   比留下更安全）。③registry=**没有会话内确定性重建**（条目无持久镜像，重装是启动期
   动作）——修复尝试如实推成 Quarantined 并在错误里写明"重启重装"，不造"看起来修好了"
   的重建。
3. **驱动点与读数**：`cmd_recover_boot` 驱动 events/registry 修复；审批修复骑在两条
   preview 命令上（重新预览=操作者对审批域的显式恢复动作）；四条边界的
   state/generation/lastFault 并入 `host_resource_stats` 的 `faults`；`wire-gate`
   轮 47 段钉形状、修复文案、布线、台账行翻篇与 CHANGELOG。

**同轮自查与读数（照抄同轮日志）**：`cargo fmt --all` 退出码 0；host 单测
`session_reset_clears_subscriptions_but_keeps_topic_declarations` 实跑 ok 后全量
`cargo test -p tauron-host --locked` 绿（lib **434 passed**，conformance 10/10）；
adapter 三条边界测试两套 feature 组合各 4 passed（299/325 filtered），全量三套
303 / 329 / 363 passed、0 failed；孤儿公共 API 台账复核 **630 ≤ 基线 635**；
domain-ownership 记账后 lib.rs 顶层条目 **290 已封顶**；`docs:check` 抓出本轮插入
导致的 **60 条**行号漂移（`--fix` 自动重锚 29 条 + 手工重锚 31 条，含 `should_execute`
这类本地绑定锚），复跑 7 文档 **251 引用**全 OK。

**变异证明（v2，13/13 逐段变红）**：语义 7 条走
`cargo test -p tauron-adapter --locked fault_boundary` 定向实跑——C1 不重置会话态
（订阅残留断言红）、C2 整重置 `EventBus::default()`（cargo 红 + 负向回潮钉红，声明被清
两侧齐抓）、C3 前置判定把 Faulted/Quarantined 当 Ready（修复永不发生，红）、C4 panic
登记进一次性边界（真边界 Faulted 断言红）、C5 审批修复不清令牌（"彻底不认识"断言红）、
C6 注册表修复伪造 Ready（Quarantined 断言红）、C7 boot 不驱动注册表修复（cargo 红 +
驱动钉红）；布线 6 条走轮 47 段定向跑——W1 事件命令脱闸门（接线数 7→6）、W2 两处
`~~A91` 划线被撤、W3 台账行回写「未落」、W5 事件修复文案丢「topic 声明保留」、W6
CHANGELOG 条目被改，各命中其钉。**W4 第一遍假绿**：形状钉原是裸名
`pub fn reset_session_state`——入口改名 `..._x` 后它仍是裸名的字符串前缀，`.includes()`
照过（与轮 46 未锚定 regex 同属"针不够长"一类的假绿）；改成带签名收尾
`pub fn reset_session_state(&self)` 后 v2 全红。收尾：复跑两条基线命令全绿、5 文件
树完整性终检全部一致。

**电池快照（14 腿全 rc=0，照抄同轮日志）**：首跑三腿红，均与本轮真改动相邻——
clippy 抓出 `reconcile_registry_boundary` 文档注释里行首 `+` 被 Markdown 解析成列表符、
其后两行成了"懒惰续行"（`doc_lazy_continuation`；`+` 改成「与」后复绿）；`--record`
写出的 `contracts/adapter-domain-ownership.json` 未过 prettier（`--write` 后
`format:check` rc=0）；`docs:check` 把读数里 EXIT 冒号 0 的写法当 `symbol:NNN` 引用
解析（≥3 字符单词 + 冒号数字即命中——改写「退出码 0」后 7 文档 251 引用全 OK）。修复后
全量电池 14 腿全绿：fmt / clippy / prettier / eslint / version / command-surface /
gates / docs 各 rc=0；workspace **1582**（0 failed bins）/ market **78** /
adapter plugin-install **329** / tauri,plugin-install **364**；契约测试 2 文件 **211**
（wire-gate 全量 190）；`pnpm verify` 20 包 **1935**；孤儿台账 **630 ≤ 635**、
ownership lib.rs **290 已封顶** + 自检 6 形、tier-bundle 3/10/85。

### 轮 48：A96 重定向/DNS 复检接生产——单跳 sink 契约 + 宿主侧逐跳授权

**发现路径**：A96 判据要求「重定向/解析尊重 scope」。`network_policy.rs` 里
`authorize_redirect` / `authorize_resolution` 实现完整（跨源凭据不转发、私网/字面 IP
拒、hop 上界），但**除模块自测外零调用者**——生产命令 `cmd_http_request` 只做首跳
`authorize_url`，逐跳与 DNS 复检在生产路径上不可达；换一个角度说：即便接入方注入
自己的 provider，当时也没有任何契约要求它把「实际解析地址」交回宿主复检，
`NetworkEnforcement::RedirectAndDns` 档没有任何实现者。

**修法（三件）**：

1. **单跳 sink 契约**：`HttpSink` 文档改为——provider **不得**自行跟随重定向
   （3xx 原样返回 + `location` 头），**必须**在响应里回报本跳实际解析地址
   （`HttpResponseSpec.resolved_addrs:2481`）；redirect 跟随与解析复检全部归宿主。
   `NetworkEnforcement::RedirectAndDns` 升为**能力闸**：声明其它档的 provider 在
   `cmd_http_request:9306-9313` 直接拒发（`E_AUTH_DENIED`）——这不是"降级跑"，是
   策略不可执行时拒绝执行。
2. **宿主逐跳循环**：`cmd_http_request:9306` 重写——每跳响应先按 provider 回报地址
   `authorize_resolution:9303` 复检（不可解析地址失败关闭、私网/字面 IP 拒）；
   3xx 且带 `location` 时 `resolve_redirect_location:260`（`tauron-host` 新公开 fn，
   `Url::join` 做 RFC 3986 相对解析）→ `authorize_redirect:9301`（`max_redirects`
   hop 上界；跨源 `forward_credentials=false` 时剥离 `authorization`/`cookie`/
   `proxy-authorization` `:9298-9303`）→ 下一跳；3xx 无 `location` 按最终响应交还
   调用方（没有可授权的下一跳，不臆造）。
3. **注入面 + TS 线形**：`AdapterConfig::with_http_sink:424` 闭合此前"接入方注入自己
   的实现即可启用"的悬空承诺；TS 侧 `HttpResponseSpec.resolvedAddrs?`
   （`shell-client.ts:490`）随线形可选回传。五条行为测试（四条走 `ScriptedHttpSink` +
   一条 `UrlOnlySink` 拒发）：
   `http_redirect_flow_follows_hops_with_rfc_3986_resolution:19060` /
   `http_redirect_cross_origin_strips_credentials_and_denied_target_never_sent:19085` /
   `http_redirect_loop_is_bounded_by_policy_max_redirects:19115` /
   `http_provider_resolution_is_rechecked_against_private_network_policy:19131` /
   `http_provider_without_redirect_enforcement_is_rejected_before_any_request:19149`。

**同轮自查与读数（照抄同轮日志）**：`cargo fmt --all` 退出码 0；adapter HTTP 六条
测试全绿（`6 passed; 0 failed; 302 filtered out`）；domain-ownership 记账后 lib.rs
顶层条目 **291 已封顶** + 自检 6 形；孤儿公共 API 台账 **629 ≤ 635**
（`resolve_redirect_location` 有生产调用者，不改基线）。

**变异证明（v4，18/18 逐段变红）**：语义 5 条走
`cargo test -p tauron-adapter --locked http_` 定向实跑——C1 能力闸反向（UrlOnly
provider 被放行、「必须是 Supported」断言红）、C2 跨源凭据剥离块被移除（cargo 跨源断言
红 + wire-gate 单跳 needle 环红，双侧齐抓）、C3 解析复检被吞（私网复检断言红）、C4
RFC 3986 相对解析丢失（cargo 断言红 + wire-gate「RFC 3986 相对解析被掏空」）、C5 能力闸
整块删除（UrlOnly 测试 `E_HOST_PANIC` 红）；布线 13 条走轮 48 段定向跑——W1 调用点改名、
W11 加塞第二调用点（接线数 1→2）、W12 strip 段三头串缩短（接线数 2→1）、W2 TS 线形被删、
W3 台账行回写「部分」、W4 CHANGELOG 条目被改、W5 两处 A96 划线全撤（计数 2→0）、W13
只撤 Batch 5 一处（计数 2→1）、W6 tauron-host 导出被撤、W7 契约文案丢「必须回报解析
地址」、W8 with_http_sink 改名、W9 能力矩阵行回退、W10 命令面文档 `resolvedAddrs`
被改，各命中其钉。收尾复跑两条基线命令全绿、8 文件树完整性终检全部一致
（`18/18 逐段变红，全部达到预期`）。**两遍补救实录（变异证明连抓两类假绿）**：
其一 v1 15/16——C2 首版把期望红理由写在计数钉「跨源凭据剥离三头不在」，但整块删除先撞
needle 环（`if !authorization.forward_credentials` 随块消失），计数钉轮不到命中；改两件：
C2 期望改 needle 环文案，另立 W12 只缩短 strip 段三头串（needle 全保留）专测计数钉，
v2 即 17 条全红。其二 v3 16/17——本节读数成段后，段内描述划线变异的文字恰好含与门禁
相同的 A96 划线前缀子串，W5 全撤两处真划线后门禁仍被读数段文字喂饱（裸子串钉被自家
文档浸透）；修两件：门禁上收为**计数钉**（两处划线恰计 2），另立 W13 专测「只撤一处」
的部分移除方向，读数段措辞同步避开该子串，v4 即 18 条全红。两遍缺口与修复均可用
`C:/tmp/r48-mutation-v4.log` 复核。

**电池快照（v2，14 腿全 rc=0，照抄同轮日志）**：首跑三腿红，均与本轮改动相邻——
clippy 抓出新测试里两处 `get(..).is_none()` 写法（`unnecessary_get_then_check`，
改 `!contains_key(..)` 后复绿）；prettier 抓出轮 48 段的行式（`--write` 重排后复绿）；
`command-surface:check` 抓出「登记语义」列被手改而生成源没动——该列真源是 `tauri.rs`
命令 `///` 注释（把单跳契约文移进 `host_http_request` 注释再用
`pnpm command-surface:gen` 重生成后复绿；同轮教训：**生成物只向真源改，不手抄**）。
修复后全量 14 腿全绿：fmt / clippy / prettier / eslint / version（26 处=1.1.0）/
command-surface / gates / docs 各 rc=0；workspace **1587**（0 failed bins）/
market **78** / adapter plugin-install **334** / tauri,plugin-install **369**；
契约测试 2 文件 **212**（wire-gate 全量 **191**）；`pnpm verify` 20 包 **1936**；
孤儿台账 **629 ≤ 635**、ownership lib.rs **291 已封顶** + 自检 6 形、tier-bundle
3/10/85、docs 7 文档 **275 引用**。

### Batch 1'（对齐 §135.1 Batch 1，2–3 天）


- ~~A65 Profile V2：建模 `ProductTier` × `CapabilityBundle`，与命令面子集做**一致性门禁**（新 `contracts/tier-bundles.json` + `scripts/check-tier-bundle.mjs`）；不做「声明了但没人读」的表~~ **轮 45 已落**：三张表 + 六条不变量进 `gates:check` 与 CI（85 命令单属划分、S⊂E⊂P 单调、正交性机器证明）。
- ~~A66 `ReadinessSet`：定义零 Surface 就绪集合，落一条真跑 conformance（无 window/tray/menu 启动 → doctor → liveness/readiness 分离）~~ **轮 46 已落**：六组事实 + `headless()` 空 Surface 不阻塞 + `blockers` 具名；conformance 三条真跑（零 Surface 就绪 / 声明必兑现 / kernel 不豁免）。
- ~~A91 扩边界：把 settings 之外的子系统（事件/审批/注册表）逐个包 `FaultBoundary`，每包一个就补一条 quarantine→reconcile 测试~~ **轮 47 已落**：三条边界各带自己的确定性修复语义与实测——events 会话态清零且保留 topic 声明、approval 重新 preview 清令牌、registry 如实 Quarantined 待重启。

风险：Tier/Bundle 一旦进 ledger 就是对外承诺面，须与 §137 支持等级措辞同时更新。

### Batch 2'（Universal Contract 固化，1 周）

- A67/A68：命令面穿 envelope + codec 协商握手（**破坏性**：本条原写「需 `host_protocol_info` 一条新命令承载协商结果」——那个命令今天不存在，而 `wire-gate` 有「V5 §46.3 冻结：新增根 `host_*` 命令必须显式过账」的断言把命令面钉在 **85** 条。落地顺序因此必须是：**先**走「协商结果作为返回字段并入既有命令」这条路（轮 33 的 `stateSimulated` 就是先例：加字段不破冻结），确实装不下再申请显式解冻并把 85 的断言连同 §137 支持等级措辞一起改）。方案已点名「不硬凑」的两条里这是第一条。Manifest 引入 `manifestVersion` 与 extensions 通道，配 V2→V3 兼容判定。
- A71：把 `resolve_best` 接进安装/更新路径（变体过滤），`ArtifactVariantResolver` 从注释变实现。（轮 34 复核：`resolve_best` 在 `tauron-host/src/target.rs`，接线面内零生产消费者，已登记 `contracts/orphan-public-api.json`——本条落地时**必须**同步删除那条台账，否则门禁会红。**轮 49 已落**：`resolve_best` 泛型化 + `ArtifactVariantResolver` 接进 `DistributeUpgradeInstaller` 的 download/install，台账条目已删除。）
- A75：让拓扑序**真的执行**——本条原写「装配走 `service_startup_order`、app-exit 走逆序 `service_shutdown_order`」，**这两个字段已在轮 11 删除**，且有反向门禁「服务拓扑图不得再抄两份无人执行的序」把 `service_startup_order|service_shutdown_order` 钉成永不再出现。删除的理由不是省事：图的边描述**运行期能力依赖**（`message` 依赖 `capability` 的审批面），不是构造顺序，而本仓当时（到今天仍是）**没有可排序的服务级回收动作**——真回收按窗口走（`cleanup_closed_window`），app-exit 没有逐个服务的钩子。所以这条的诚实形态是：**先造出执行点**（服务级 shutdown 动作 + 一个真按逆序回收它们的地方），再谈"按拓扑序执行"；在那之前装配期只做图的有效性校验（`canonical_substrate_service_graph().startup_order()` 校验失败即 panic，测试 `substrate_assembly_validates_the_canonical_service_graph`），不要为了看起来像拓扑序而恢复那两个字段。

风险最高的一步：动的是已发布命令面形状。必须在独立轮次做，并配 N-1 兼容门禁（这也是 2.0 判据之一）。

### Batch 3'（并发与资源，3–5 天）

- A80 公平调度接线：**从空白重新落**（轮 10 已删除零消费者的 `FairQueue` 死类型，见 V4 台账轮 10 小节）。要求一次性带齐三件——`call_begin` 路径真的按 owner 轮转、端到端饿死压测（慢 owner 不拖垮快 owner）、`wire-gate` 反向钉住「准入门必须有消费者」。只补类型不接消费点会重演 F4。
- A79 其余三类资源 credit 建模；A81 撤销三档策略成类型，并把 revoke 后果扩到「既有订阅退订 + provider 重检」。
- A78 补「调用环 + 事件回流」复合用例。

### Batch 4'（安装/存储一致性，1 周）

- ~~A83 token 覆盖 uninstall/purge/update；TTL 从墙钟改走 A100 的可信时间~~ **轮 43 已落**（管理域 uninstall/purge 走 `AdminReviewToken` 预览→提交链，wire 判别形不破老读者；install/管理两域 TTL 均改走 A100 可信时间，生产档无 provider 直接拒启；update 域＝install+uninstall 两腿组合、无独立通道）。
- A84 逐资产服务复核摘要（`read_installed_plugin_asset`）。
- ~~A85 PortablePathValidator 补齐保留名/归一/尾点/大小写/长路径五维，zip 侧同步~~ **轮 42 已落**（五维各自具名失败面 + 集合级 ASCII 折叠冲突；zip 侧 `signing` 生产面委派同一份判定）。
- ~~A94 安装路径 RSS 门禁（把 `maxPeakRssKb` 断言从探针迁到真实安装流；删或降级 `verify_tpkg(&[u8])`）~~ **轮 44 已落**（`verify_tpkg`/`verify_tpkg_file` 整包入口已删除、零调用者；门禁改跑真实安装流——96 MiB 载荷探针 `install_stream_probe` 走 preview→reviewed 提交链，堆峰值（计数分配器）+ Linux VmHWM 双读数写进 `performance-budgets.json` 的 `installStream`；同轮修掉激活摘要整读的隐藏缺口，全链 64 KiB 流式）。
- ~~A101 `MigrationSnapshot` 落盘 + 进程重启级回滚 E2E~~ **轮 11 已落**（改为宿主设置域的磁盘回滚镜像 + 三轮重启 E2E；死结构 `MigrationSnapshot` 删除）。同批的 A84 逐资产摘要复核亦已落。
- 剩余：无（本批逐项均已翻篇：A83 轮 43、A85 轮 42、A94 轮 44、A101/A84 轮 11）。

### Batch 5'（跨宿主与安全隔离，2 周，最大投入）

- A86+A106：把 `LocalHostBroker` 从 example 提升为**可分发的第二官方 Host**（真长进程、命令面路由、Windows named-pipe 腿进矩阵），并把 `local_host_reference` 的 OS 凭据实现下沉到 broker。
- A95 Windows handle-based enforcement（或明确把 `Partial` 写进对外能力矩阵，二者必居其一，不能沉默）。
- ~~A96 把 `authorize_redirect/authorize_resolution` 接进真实 sink 路径~~ **轮 48 已落**（宿主侧单跳循环：sink 只发单跳并回报解析地址，逐跳授权 + 跨源凭据剥离 + 私网复检全部在生产命令路径上）；A97 明确「Hard 隔离」是实现还是能力边界声明。
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

### 轮 49：A71 变体过滤接进安装/更新装配腿——`resolve_best` 从在册孤儿变消费者

**缺口**：`resolve_best`（`tauron-host/src/target.rs`）此前只有自测，安装/更新路径不过滤
变体；`ArtifactVariantResolver` 仅存在于 `CapabilitiesBody.target` 的注释里；该函数因此在
`contracts/orphan-public-api.json` 里挂着「未接线」条目——文档层面也承认它是孤儿。

**落地**：

- `resolve_best` 泛型化（`T: AsRef<TargetSpec>`；`TargetSpec` 自身与含目标字段的载荷都能
  直传；并列时 `max_by_key` 取后者），新增 `ArtifactVariantResolver` 作为「任何 OS 加载
  动作之前」的具名解析入口；
- 适配器新增 `UpdateArtifactVariant`（目标 + 平台专属清单，`impl AsRef<TargetSpec>`）与
  `DistributeUpgradeInstaller::with_variants`：变体列表非空时，`download()` / `install()`
  都先经 `select_manifest` 按**本机编译目标**（`current_target_spec`，cfg 推导、调用方不可
  伪造）解析最具体的兼容变体；解析不出即 `E_INVALID_MANIFEST` 硬拒——硬拒发生在任何下载
  动作之前，不静默回退单包默认；
- `install()` 以同一份被选中清单重建 runner options：runner 会用被选中清单重验 staged
  包的 sha256/签名，若这里回退单清单，变体下载的包会在重验时被判不一致（download/
  install 必须同解析口径）；空列表保持既有单清单语义（已装配宿主行为不变）；
- 孤儿台账中 `TargetSpec::resolve_best` 条目删除——代码已接，条目留着就是撒谎。

**同轮自查与读数**（v1，照抄同轮日志）：三条产线证据各自独立——
`cargo test -p tauron-host --locked target::` 里 `artifact_variant_resolver_returns_none_when_no_variant_is_runnable`
与 `artifact_variant_resolver_keeps_payload_of_the_most_specific_variant` 通过（选中最具体、
无兼容取 `None`）；`cargo test -p tauron-adapter --locked round49_variant_resolution_tests`
两条通过——无兼容变体在 0 次下载调用下 `E_INVALID_MANIFEST` 硬拒且无 staged 残留；变体
列表按本机目标选中（断言下载器收到的 URL 恰为兼容变体，而非列表首位或单清单默认），
且 `install()` 返回被选中版本并以新归档内容完成真实交换；`node scripts/check-orphan-public-api.mjs`
绿（自动发现 629 ≤ 基线 635，且不再有 `resolve_best` 在册条目）。

**变异证明**（v2，18/18，照抄同轮日志）：仓外脚本 `C:\tmp\round49-mutation-proof.mjs`
先做针自检（18 条变异、6 个文件、每个 from 串恰 1 次），基线 HOST/ADAP/GATE 三跑全绿；
随后 18 条逐段变红——C1（绕过「选中最具体」仍回单清单）/C2（硬拒文案判定短语被替换）/
C3（`resolve_best` 退化为「首个兼容」）/C4（删 `cpu_features` 子集判定）在 cargo 腿
**按名报红**；W1–W14（台账重挂、host/adapter 测试名、结构名、impl 行、签名、调用点、
再导出行各自后缀式改名、A71 台账行翻回『部分』、CHANGELOG 抽编号、小节降级）在门禁上
判红；每条恢复后终检三跑全绿、6 文件 sha 与开局一致。

v1（15 条）曾抓出三类假绿，均按「门禁不咬就修门禁」修掉并复跑：① W8 的
`impl AsRef<…> for UpdateArtifactVariant` 针被 `UpdateArtifactVariantX {` 后缀改名骗过
——针补 ` {` 收尾；② W11 的裸名 `ArtifactVariantResolver` 针被再导出行里的
`ArtifactVariantResolverX,` 骗过——针改带逗号分界的名单片段；③ C2 的短短语针被
**测试文件自己的断言文本**喂饱（单文件 includes 针分不清生产者与消费者文本）——门禁上收为
完整生产者文案；另 v1 的 C2 变异整句掏空使 `format!` 参数悬空而编译错（「按名报红」
判不到），改写为保留占位符的语义变异。W12–W14 系本轮新补的三条针证明（两条 adapter
测试名 + 结构名）。

**电池快照**（照抄同轮日志）：14 腿镜像 `ci.yml`（fmt / clippy / workspace / market signing /
adapter plugin-install / adapter tauri+plugin-install / prettier / eslint / version /
command-surface / gates / docs / contract-tests / verify）。首跑 13 腿全绿、`gates:check`
一腿红——轮 49 新增的 `impl UpdateArtifactVariant` 未登记域归属、`DistributeUpgradeInstaller`
方法数 8 > 基线 6、lib.rs 顶层条目 292 > 基线 291。按轮 48 先例**手改**
`contracts/adapter-domain-ownership.json`（不跑 `--record`，保住 note 史）：补 impl 登记
（域 distribute）、`maxCount 6→8`、`maxItems 291→292`，并把轮 49 记录追加进 `note`；
复跑 `pnpm gates:check` 绿——「Adapter-Domain-Ownership gate OK across 6 source files
(41 个 impl 类型 / 187 个方法、91 组顶层 fn / 384 个函数、19 个域、lib.rs 顶层条目 292
已封顶；数量全部 ≤ 基线)」+ self-test 6 形状 + 孤儿台账 629 ≤ 635 + tier-bundle OK；
`pnpm format:check` 复跑绿。最终读数：workspace 1591 / market 78 / adapter(plugin-install)
336 / adapter(tauri,plugin-install) 371 / contract-tests 2 文件 213（含轮 49 门禁段）/
`pnpm verify` 20 包全绿 / docs 7 文档 277 引用 / 命令面 85（孤儿 0）。

### 轮 50：无效历史文档清理——删除 0.4 / 1.0 两份方案，并把挂在它们身上的门禁针与台账指向搬到现行载体

**缺口（文档侧断链，不是代码侧）**：`docs/architecture/` 里还放着两份**状态宣称已与代码相反**的历史方案——
0.4 收口方案自陈「仅 A1–A3 落地」且 §2 把 `tauron-acl` / `tauron-market` 写成「迁移中/孤儿」（早已进依赖表）、
`plugin-install` 默认特性口径也被 1.1 反向更正；1.0 重构方案的状态行停在「W1-b / W10 部分实施，其余待做」。
按本仓口径「文档声称 = 代码相反即断链」，这两份是给下一个人的**错误施工单**。
另有一条实测抓到的现状谎报：`crates/tauron-proc/src/lib.rs` 顶部注释写「真实链路的心跳监控尚未实现」，
而 `crates/tauron-host/src/generation.rs` 的 `heartbeat()` 续租 + 过期租约回收是真实现（该 crate 内零 heartbeat 符号）。

**处置**：
- 删除 `capability-closure-plan.md`（665 行）与 `full-architecture-refactor-plan.md`（684 行）。
  **保留** `multi-plugin-substrate-roadmap.md`：它是 §0「已达成、不得回退的硬不变量」与 M-9 现行判定的唯一载体，
  且轮 34 / 轮 37 两条门禁针钉在它身上。删除理由与耐久产出去向写进 `docs/architecture/README.md` 的清理表。
- 门禁针搬迁 + **收紧**：轮 34 段④ 原钉 0.4 横幅的「`bootstrap.ts` … 已随 W1-b … 删除」，改钉 roadmap M-9 同一句话
  （该句已在，无需 transplant）。首轮变异**不变红**——原正则 `已随 W1-b[\s\S]{0,40}删除` 把「已随 W1-b 待删除」也判通过；
  改成终点锚定 `已随 W1-b 删除` 后变异才咬（见下方读数）。
- 台账指向改挂：`contracts/orphan-public-api.json` 四条 `docRef` 原本指向被删文档——
  `AclStore::load_unverified`、`build_approval_rows`、`is_downgrade / is_monotonic` 三条改挂本台账（登记口径不变，
  （轮 54 更正：`is_downgrade` 已接线并销账，该条改为只登记 `is_monotonic`）
  **三条本轮未接线**，见下「诚实边界」），`ConfigManager` 一条改挂 roadmap（M-9 正文点名该类）。
- 其余悬挂引用逐处改到现行载体：`overview.md` 接线状态段、`competitive-analysis.md` 的分包判定口径段、
  roadmap 两处指向已删文档的链接（改为「原登记在 0.4 §6 / 0.4-A2，文档已清理」的过去时叙述）、
  `tauron-proc/src/lib.rs` 心跳注释、`docs/architecture/README.md` 文档地图两行。

**变异证明**：基线 contract-tests 2 文件 213 例全绿 → 把 roadmap 的「已随 W1-b 删除」改成「已随 W1-b 待删除」后
轮 34 段整段判红（失败消息点名该 needle）→ `cmp` 逐字节确认恢复 → 复跑 213 例全绿。

**读数（本轮日志）**：`docs:check` rc=0、`gates:check` rc=0、`format:check` rc=0、`command-surface:check` rc=0；
contract-tests 终态 2 文件 213 例通过；`cargo test -p tauron-proc --locked --lib` 30 passed。

**诚实边界**：本轮只动文档与注释，**没有**接任何孤儿 API，也没跑 workspace / market / adapter 全量腿
（唯一 Rust 源改动是 `//` 注释，无语义变化）；两份被删文档在 git 历史中仍可取回，本次删除只落文件系统、未进 index。

**发布链状态（轮 50 收尾时实测，不是推测）**：代码侧已提交并推送（`74bf2cc..52881de main`），
但**发布链在这里断在凭据与标签上，不是断在代码上**：
`node scripts/check-published-versions.mjs` 判「共 37 项在 1.1.0 上未通过核验：npm 缺 20 个、
crates.io 缺 9 个、未能核实 8 个（网络/超时，重跑可复核）」。
两个发布脚本都硬性要求环境变量令牌（`NPM_TOKEN` / `CARGO_REGISTRY_TOKEN`），本机 `npm whoami` 为 `ENEEDAUTH`，
所以真发布只能走仓库自带的 `Publish SDK packages` 工作流（`workflow_dispatch`，`publish=true`）。
另有一条必须在发布前定死的不一致：`v1.1.0` 标签已在 origin 上、指向 `6388953`（轮 13–18），
而 main HEAD 是 `52881de`（轮 22–50），且 GitHub Releases 列表为空。
按「先 publish、registry 校验通过后才推 `v*` tag」的规矩，**不能**让 registry 里的 1.1.0 源码与标签指向的
提交不是同一份：要么为轮 22–50 另起版本号（`pnpm version:sync --set`），要么显式同意移标签（破坏性，需单独批准）。

### 轮 51：registry-check 恒红的根因——`publish = false` 的 crate 被算进核验清单

**断链（发布主链上的死锁，不是文档问题）**：`scripts/check-published-versions.mjs` 用
`readdirSync('crates')` 枚举全部 crate 目录，**没有**任何 `publish = false` 过滤（npm 侧有
`private !== true` 对偶过滤）。轮 41 之后 `crates/tauron-test-sidecar/` 进来了（清单里
`publish = false`），于是这条门禁开始要求一个**永远不可能出现在 crates.io 的 crate**。
而 `release.yml` 的 `registry-check` 正是创建公开 Release 的门槛——门槛被一个不可满足的条件钉死，
这就是「tag `v1.1.0` 已推、GitHub Releases 却一条都没有」的真实原因。

**修法**：与 npm 侧同构——解析每个 crate 清单，`publish = false` 或 `registry = false` 的**剔除**，
并把剔除项**打印出来**（不静默少查），核验清单只用可发布的那些。

**读数（本轮日志）**：修复前 `node scripts/check-published-versions.mjs` = 「共 **37** 项未通过核验：
npm 缺 20、crates.io 缺 9、未能核实 8」，其中 crates 列表面含 `tauron-test-sidecar`；
修复后同一命令 = 「剔除 1 个 publish=false 的 crate（不参与 registry 核验）：tauron-test-sidecar」+
「共 **36** 项在 1.1.0 上未通过核验：npm 缺 20 个、crates.io 缺 9 个、未能核实 7 个」——
与本台账先前登记的 36 项口径（npm 20 + crates 16）重新对齐。
`pnpm gates:check` rc=0、`pnpm docs:check` rc=0、`pnpm format:check` rc=0（脚本改动经 prettier 重写后复验）。
**复跑消除了唯一的含糊读数**：首跑有 7 个 crate 因网络超时判「未能核实」，重跑后是
「共 36 项在 1.1.0 上未通过核验：npm 缺 20 个、crates.io 缺 16 个」，且 0 项 `: published`、
0 项未能核实——registry 上 1.1.0 一个都没有，是**确证**而不是「大概没发」。
同一轮把发布载荷也实测了一遍：`node scripts/publish-npm.mjs`（--check 模式）= 可发布包 20 个、通过 20、失败 0；
`node scripts/publish-crates.mjs`（--check 模式）= crate 共 16 个、产物校验失败 0。

**诚实边界**：恒红死锁已解，但**1.1.0 仍未发布**（rc=1 是真的缺，不是门禁坏）——真发布要 `NPM_TOKEN` / `CARGO_REGISTRY_TOKEN`，两个脚本在没有令牌时都硬拒绝，本机 `npm whoami` 为 `ENEEDAUTH`，
所以这一步只能由带令牌的 `publish-sdk.yml`（publish=true）完成；上面 37→36 的差值也只证明「门禁不再钉死」，
不证明「Release 已建出」。另有一条未做的核对：`docs/installation.md` 里「15 个 crate」的旧读数段（§发布相关，
写于 `tauron-ffi`/sidecar 之前）与本轮的 16 未逐句对齐，留作下一轮的文档对账项。

### 轮 52：卸载插件会漏下一个空的 locale 资源包——把 `remove_resource_bundle` 从孤儿接进生产路径

**断链（孤儿逻辑，泄漏类）**：`crates/tauron-i18n` 的 `I18nEngine::remove_resource_bundle` 在
`contracts/orphan-public-api.json` 里挂着「只有本包测试能调用」。它不是多余 API——`cleanup_plugin`
（宿主卸载插件时清理该插件命名空间 `plugin:<id>.` 下的所有键）删完键之后**从不回收语言包本身**：
某个 locale 的资源若全部来自被卸载的插件，删完后会留下一个 `texts` 为空、却永久驻留在
`self.bundles` 里的包。反复装卸插件 = 无界增长的空包，且 `get_bundle()` 返回 `Some(空包)`，
让上层误判「该语言仍受支持」。这正是「接口在、事实不在」的反向形态：事实（回收）代码里根本没有。

**修法（真实接线，不是补测试）**：`cleanup_plugin` 在遍历各包删键时记录**本次被清空过键的** locale，
遍历结束后只对「本次被清空且现在 `texts` 为空」的包调用 `remove_resource_bundle`。
刻意不加「顺手回收所有空包」的宽口径——卸载 A 插件不该删掉本来就空着的 B 语言的包。
台账里该孤儿条目随之删除（`gates:check` 接受删除，孤儿数 14 → **13**）。

**读数（本轮日志）**：新增两个行为测试——
`locale_bundle_whose_only_texts_came_from_the_plugin_is_dropped`（卸载后 `get_bundle("de")` 为 `None`）与
`cleanup_keeps_a_locale_that_still_has_non_plugin_texts`（混有非插件键的 `fr` 包保留、剩 1 键）。
`cargo fmt --all` rc=0；`cargo test -p tauron-i18n --locked` = **34 passed; 0 failed**；
`cargo test -p tauron-adapter --locked --lib` = **310 passed**；
`pnpm run gates:check` rc=0；`pnpm run format:check` rc=0。
收尾时复核：孤儿台账现存 **13** 条（`node -e` 计数，见上），i18n 34 passed 为同一轮重跑实测。

**诚实边界**：这一轮只消掉**一个**孤儿，且它是 Rust 侧的泄漏修复；它**不**证明 i18n 的宿主链路完整——
`cleanup_plugin` 目前仍只有本包测试与 Rust 侧调用者，TS/前端卸载路径是否走到这里未在本轮验证。
台账余下 13 条（`AclStore::to_capability`、`build_approval_rows`、`NotifyStore::trim_to`、
`is_downgrade`/`validate_group_key`（轮 53 已消 `validate_group_key`、轮 54 已消 `is_downgrade`，见下文）、
TS 侧 `createAutoUpdateClient…`/`UpdaterStore`/`PluginRegistry`/
`ConfigManager` 等）仍是未接线公开 API，逐条口径见台账文件本身。
另：本轮顺手补上轮 51 遗留的 `installation.md` 对账（§3.5 的「全部 15 个 crate」改为本轮实测的
`publish:crates --check` 16 个/失败 0 口径；§3.7 的 2026-09-27 段是**带日期的历史读数**，按惯例不改写）。

### 轮 53：分组键校验是纸面防线——接进 `NotifyStore::push`，坏键条目此前永久不可回收

**断链（孤儿逻辑 + 边界不校验）**：`crates/tauron-notify` 的 `validate_group_key` 在孤儿台账里登记为
「只在 notify 内部与单测里跑，宿主接收任意 group 字符串不经它」。核对调用链后确认边界洞是真的：
`cmd_notify` 对 `title`/`body` 有字节预算校验，`plugin_id` 却**原样**进 `NotifyEntry`，
而 `group_key()` 无条件拼成 `plugin:<id>`。坏键一旦入环就落在回收路径之外——
`cleanup_plugin(plugin_id)` 按真实插件 id 拼键取条目，`plugin:`（空 id）这类分组没有任何入口能指名它；
`by_group` 同样查不到，条目只能等容量裁掉 = 静默泄漏。

**修法**：①`push` 在**动任何状态之前**（重复 id 检查之前）校验 `entry.group_key()`，
非法即 `NotifyError::InvalidGroup` 且零副作用——与轮 52 同一取向：宁可拒绝写入，也不留下取不回的数据。
②`validate_group_key` 补一条「整键校验必须连 `plugin:` 后面的载荷一起看」：前缀后为空即拒。
不补这条它对派生键形同虚设——派生键永远以 `plugin:` 开头，`is_empty` 与 `starts_with('$')` 都不成立，
只剩 `..` 一条会红。③`dispatch` 的既有契约不变（入缓冲失败不阻断即时通道），
其文档注释把错误类别从「重复 id」扩为「重复 id / 非法分组键」。孤儿台账相应删除（13 → **12**）。

**读数（本轮日志）**：新增 `push_rejects_group_keys_no_cleanup_can_reach`（空 id 与 `a..b` 各拒一次，
`len()==0`、`group_counts()` 空、`eviction_total()==0`）与
`bad_group_key_does_not_evict_history_on_a_full_ring`（满环上坏键写入不得先驱逐真实历史）；
既有 `validate_group_key_rejects_empty_and_dollar` 补 `plugin:` 断言。
`cargo fmt --all` 已跑；`cargo test -p tauron-notify --locked` = **48 passed**；
`cargo test -p tauron-adapter --locked --lib` = **310 passed**（宿主侧零回归 ⇒ 现有链路没有合法的空插件 id 生产者）；
`pnpm run gates:check` rc=0（输出「12 条未接线宣称复核通过…自动发现 629 ≤ 基线 635」）、
`pnpm run format:check` rc=0。

**诚实边界**：本轮补的是**存储边界**的校验，`cmd_notify` 自身仍不预检 `plugin_id`
（错误从 `push` 冒出、由 `guard` 统一包成 `HostError`）；`cmd_notify_as` 的署名保证「不能冒充别人」，
不等于「id 形状合法」。另外这只消掉 12 条孤儿里的 1 条：`NotifyStore::trim_to`（无用户侧收缩入口，
命令面 85 条冻结 ⇒ 只能挂配置）、`is_downgrade`（轮 54 已接线，台账改为登记其非对偶的 `is_monotonic`）、
`build_approval_rows`/`AclStore::to_capability`
等仍是未接线公开 API。

### 轮 54：「降级门禁另有实现（distribute 侧）」是一句假宣称——`is_downgrade` 接进升级执行器，任何字节落地之前拒绝降级

**断链（宣称与代码相反 + 核心链路缺门禁）**：孤儿台账把 `is_downgrade / is_monotonic` 登记为「只有测试调用」，
而本轮复核时在别处读到「降级门禁另有实现（distribute 侧）」的措辞。逐条核对 `tauron-distribute`：
`UpgradeRunner::validate()` 校验的是清单版本非空、staged 产物 sha256、签名、目标 `current` 目录，
**没有任何版本序比较**；`UpgradeOptions.installed_version` 这个字段的唯一作用是往 journal 写 `old_version`。
也就是说宣称是假的——一条目标版本低于已安装版本的清单（灰度包错发、清单指回旧版、手工装错包）
会被当成正常升级一路走完 download→extract→swap，并在 journal 里留下一次「成功」的降级。
`is_downgrade` 因此在台账之外还多了一层危害：它让文档敢写「已实现」。

**修法（单一算术源，不造第二套比较）**：

1. 新依赖边 `tauron-distribute → tauron-market`。选它而不是在 distribute 里再写一份解析，
   是因为版本序的**唯一算术源**在 market（`cmp_version`，含 2³¹ 截断边界的回归测试；轮 10 credit window
   的先例就是「绝不允许第二个实现」）。这条边不引入新的重依赖：market 的非可选依赖只有
   serde/serde_json/thiserror/sha2，全部已在本 crate 依赖集内，`signing` 特性不开。
2. `tauron_distribute::ensure_not_downgrade(installed, target)`（`upgrade.rs`）+ 新错误
   `DistributeError::DowngradeRejected { current, target }`。`installed = None` 表示无基线，**如实放行**
   （不猜 `0.0.0`，也不假装拦得住）。
3. 门禁挂在 `UpgradeRunner::validate()` 的纯检查段——即「零文件系统副作用」那一段：坏清单不会留下
   staged/backup/journal 任何痕迹。
4. 装配腿同样收口：`DistributeUpgradeInstaller::select_manifest` 在选中清单（含轮 49 的变体解析）之后、
   **download 与 install 两条腿任何字节移动之前**跑同一门禁，失败经 `map_distribute_error` 映射为
   `E_INVALID_MANIFEST`。download/install 同判据是轮 49 定的口径，本轮沿用。
5. 台账相应收口：`is_downgrade` 条目删除，改登记其**非对偶**的 `is_monotonic`——两者对「版本相等」判定不同
   （`is_monotonic` 含相等，门禁要求严格不倒退），不能拿 `is_monotonic` 当替身；`cmp_version` 不是孤儿，
   它是这两个谓词与 `FrameworkRange::matches` 的底座。market 侧文档注释同步更正
   （`is_downgrade` 标为已接线并点名两处调用者；`min_allowed_version` 那条与本轮无关，仍是无生产读者）。

**新增行为测试（cargo 实跑，不是文本针）**：`tauron-distribute` 的
`validate_rejects_downgrade_with_zero_file_effect`（`installed_version=2.0.0`、清单 `1.0.0` ⇒
`DowngradeRejected{current:"2.0.0",target:"1.0.0"}`，且 `download`/`backup` 目录都没被创建）与
`downgrade_gate_uses_numeric_version_order_not_strings`（`1.9.0→1.10.0` 放行——字符串序会误拦、
`1.10.0→1.9.0` 拒、相等放行、`None` 放行、`2147483648.0.0→2147483647.0.0` 拒）；
`tauron-adapter` 的 `downgrade_manifest_is_rejected_before_any_download`（兼容本机目标的变体清单指向 `0.5.0`，
已装 `1.0.0` ⇒ `E_INVALID_MANIFEST` + 消息含「拒绝降级」与实际两侧版本，`ScriptedHttpSink` 的 `urls` 为空
= 一个请求都没发，staged 路径不存在）。

**读数（本轮日志，`/c/tmp/r54-counts.log`）**：`cargo fmt --all --check` rc=0；
`cargo test -p tauron-distribute --locked` = **71 passed; 0 failed**（含本轮 2 条）；
`cargo test --workspace --locked` = **1598 passed; 0 failed**；
`cargo test -p tauron-market --features signing --locked` = **78 passed; 0 failed**；
`cargo test -p tauron-adapter --features plugin-install --locked` = **337 passed; 0 failed**；
`cargo test -p tauron-adapter --features tauri,plugin-install --locked` = **372 passed; 0 failed; 4 ignored**
（含本轮新增的下载腿用例）。

**变异证明（`/c/tmp/r54-mut.log`、`/c/tmp/r54-mut-w1.log`）**：门禁 = 轮 54 的 wire-gate 用例 + 三条 cargo 行为测试。
语义腿 C1 删执行器调用（红 `validate_rejects_downgrade_with_zero_file_effect`）、C2 谓词掏空（红
`downgrade_gate_uses_numeric_version_order_not_strings`）、C3 删装配腿调用（红
`downgrade_manifest_is_rejected_before_any_download`）；布线腿 W2–W12 各破一根针（签名后缀改名、台账把
`is_downgrade` 登记回来、测试名改名、错误映射分支删除、market 宣称改回未接线、小节标题降级成 `####`
（验证 `/^### 轮 54：/m` 的锚定）、CHANGELOG 抽编号、两处生产调用点计数钉各加一个、crate 不再导出、
错误形制改元组）。首跑 **14/15**：W1「注释掉 `tauron-market` 依赖行」在 `toContain` 针上**假绿**——
`# ` 前缀不改变子串存在性，而那条边事实上已经没了。针改成行锚定正则
`/^tauron-market = \{ path = "\.\.\/tauron-market", version = "1\.1\.0" \}/m` 后，W1a（注释）与
W1b（整行删除）各自变红复跑 → **16/16 逐段变红**，15 个文件 `cp` 还原 + 读回逐字节一致，复跑门禁全绿。
（教训入档：依赖/声明类针一律行锚定，裸 `toContain` 会被注释前缀浸透。）

**终检读数（同一轮日志）**：`pnpm run gates:check` rc=0——孤儿台账「**12 条**未接线宣称复核通过、5 条已接线
反例复核通过、自动发现 **628 ≤ 基线 635**」（本轮把一个自动发现条目让给了真实生产消费者）；
adapter 域归属 41 个 impl 类型 / 187 个方法 / lib.rs 顶层条目 292 封顶；tier-bundle 3 tiers / 10 bundles /
85 commands 单属；`format:check` / `docs:check`（7 文档 **275** 条引用）/ `command-surface:check` /
`version:check` / `lint` 全 rc=0；契约测试 **214 passed**（wire-gate 193 + contract 21）；
`pnpm verify` 20 包全 Done、**1938 passed**。文档同步：`overview.md` 依赖图新增
`tauron-distribute → tauron-market` 一条边、接线表与 `canonical-owners.md` 的 distribute 行登记本轮门禁，
`incremental-adoption.md` 的「市场监管 / 更新链」行补上降级门禁的对外语义。

**诚实边界**：①这是**执行器 + 装配腿**的门禁，命令面仍是 85 条冻结，没有新增 `host_*`；
②基线来自 `UpgradeOptions.installed_version`，由装配方提供——传 `None` 时门禁如实放行
（本仓库今天**没有**生产构造点：`DistributeUpgradeInstaller::new` 只出现在适配层测试里，
宿主 `host_market_download/install` 拿到的安装器仍由装配方注入 = Batch 5' 缺口。
所以本轮闭合的是「执行器与装配腿内部不再有第二条放行路径」，不是「线上已经在拦」）；
③比较口径是 market 既有的数字语义版本序，预发布/构建元数据的排序沿用 `cmp_version` 现状；
④插件级「最低允许版本」（`min_allowed_version`）仍无生产读者，本轮无关；
⑤降级被拒对外映射为 `E_INVALID_MANIFEST`（清单事实非法），不新开错误码。
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
# A75：序算而不用（**轮 11 处置后期望零命中**——两个字段已删除，反向钉在
# wire-gate「服务拓扑图不得再抄两份无人执行的序」；命中即说明有人恢复了没有执行点的声明）
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

轮 22–25 新增项的复核命令（同一口径：命令能自己复算本文的判定）：

```bash
# 轮 22：孤儿公共 API 台账——未接线宣称查不到消费者、反例查得到、棘轮不许放宽
pnpm gate:orphan-api
node scripts/check-orphan-public-api.mjs --discover          # 每 crate 的自动发现分布
rg -n 'stripRustTestModules' scripts/check-orphan-public-api.mjs   # 大括号配对删测试模块（假红根因）
# 轮 22：sidecar 写路径有界 + 有界收尸
rg -n 'WRITE_QUEUE_FRAMES|reap_bounded|KILL_REAP_TIMEOUT_MS' crates/tauron-proc/src/spawner.rs
rg -n 'tx\.try_send' crates/tauron-proc/src/spawner.rs        # 期望：write_frame 只入队，命令线程不碰管道
# 轮 22：框架层未接线口径必须与台账同源（改一处必须两处同改）
rg -n '未接线\*\* \| `tauron-shell`' docs/architecture/overview.md
rg -n 'overviewWiring' contracts/module-maturity.json
# 轮 23：落点表解析不得依赖排版——rustfmt 折一行曾把 10 个键读成 2 个
pnpm --filter @tauron/host exec vitest run src/gates.test.ts -t "Rust 落点表覆盖结构体的每个字段"
#   变异自证：把 CLIENT_CONFIG_LANDING 任意一条表项折成三行 → 三条断言仍须绿；
#   删掉任意一条表项（或删掉 TS 镜像里对应键）→ 必须红。
# 轮 23 曾把「`active` 只进不出」当作防句柄复用的必要代价；轮 25 证明它与「有界」并不冲突：
# 号源改成全局单调计数器后，摘除跟踪仍然不会复用号——两条口径现在都要能复算。
cargo test -p tauron-host --lib runtime_generation_advances_and_old_handle_becomes_stale --locked
cargo test -p tauron-host --lib generation_tracking_stays_bounded_under_plugin_churn --locked
cargo test -p tauron-host --lib tracked_resources_stay_bounded_under_churn_of_distinct_ids --locked
cargo test -p tauron-host --lib generation_tracked_resources_do_not_accumulate_across_plugin_churn --locked
cargo test -p tauron-adapter --lib resource_stats_report_live_usage_and_are_main_window_only --locked
# 有界读数的四处同源（改一处必须四处同改，wire-gate 逐段核对链路）：
rg -n 'pub fn forget|high_water' crates/tauron-host/src/generation.rs
rg -n 'generations\.forget' crates/tauron-host/src/runtime.rs            # 回收连带摘除跟踪
rg -n '"generations": generations' crates/tauron-adapter/src/lib.rs      # 命令面读数
rg -n 'generations: GenerationStats' packages/tauron-host/src/shell-client.ts
#   变异自证（轮 25 同轮实录，改完记得还原）：
#   删掉 remove_plugin 里的 `self.generations.forget(plugin_id);`
#     ⇒ registry + runtime 两条 churn 测试红（407 passed / 2 failed）+ adapter 的活值读数测试红
#        （回收后 `trackedResources` 回不到 0）+ wire-gate leak gate 红；
#   把 `activate` 改回每资源序号（轮 23 及之前的写法）
#     ⇒ 6 条红，其中 `runtime_generation_advances_and_old_handle_becomes_stale`、
#        `versioned_runtime_lookup_rejects_old_generation` 是**既有**代际测试：
#        摘除跟踪与每资源序号不可共存，这正是号源必须全局的证据；
#   删掉命令面那行 `"generations": generations`
#     ⇒ adapter `resource_stats…` 测试红 + wire-gate「host_resource_stats 未带上代际读数」红。
# 轮 23：Windows reference 腿无读超时（Batch 5' 的 overlapped 改造收口前不得升格为可用能力）
rg -n 'ConnectNamedPipe\(listener\.handle\.raw\(\), null_mut\(\)\)' crates/tauron-host/src/local_host_reference.rs
rg -n 'REFERENCE_EXCHANGE_TIMEOUT' crates/tauron-host/src/local_host_reference.rs   # 仅 Unix 侧有上界
# 轮 23：发布集合必须切开 `publish = false` 的夹具，且**点名**排除（不许静默少一个）
node scripts/publish-crates.mjs --check | rg -A 2 '已排除'      # 期望：列出 tauron-test-sidecar
rg -n 'publish === false|publish = false' scripts/publish-crates.mjs scripts/check-registry-ownership.mjs
# 轮 24：域所有权台账——两类形状都要登记，且门禁必须自己证明有牙齿
pnpm gate:domain-ownership
node scripts/check-adapter-domain-ownership.mjs --self-test     # 期望：6 个变异形状各按规则判定
rg -n "kind === 'fn'|fileBudgets" scripts/check-adapter-domain-ownership.mjs   # 自由 fn 与单文件封顶
#   变异自证（同轮实录，改完记得还原）：
#   往 crates/tauron-adapter/src/lib.rs 追加一行 `pub fn zzz_probe() {}`
#     ⇒ 必须同时报出 R5（条目 264 > 263）与 R1（`fn zzz` 未登记域归属）；
#   把台账里 lib.rs 的 maxItems 调低 1 ⇒ 必须红；
#   从台账删掉全部 `kind: "fn"` 条目 ⇒ wire-gate 的「台账缺少顶层 fn 类条目」必须红。
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
诚实边界（当时无真 sidecar 运行期证据——轮 22 已由 `tauron-test-sidecar` 收口、`Partial` 非 `Hard`、`to_capability` 无生产消费者）
同时写进文档与 A97 行的未验证清单）。
