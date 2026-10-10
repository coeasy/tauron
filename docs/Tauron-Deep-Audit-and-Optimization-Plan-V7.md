# Tauron Deep Audit and Optimization Plan V7

> 基于 `coeasy/tauron` 最新 `main` 的三轮完整复核。本文同时记录验证证据、V6 遗留项收敛状态、新增问题、核心链路断点、修复顺序和发布验收标准。

## 1. 审计结论

本次审计基线为：

| 项目 | 结果 |
| --- | --- |
| 仓库 | [coeasy/tauron](https://github.com/coeasy/tauron) |
| 分支 | `main` |
| commit | `74bf2ccd05d2e3252aa34f7b535af72faee7d88e` |
| 提交时间 | `2026-10-03 18:56:20 +08:00` |
| 提交说明 | `feat: close audit rounds 19-21 (release chain truth, leg-level gates)` |
| V6 基线 | 同一 commit；本次未发现 `main` 已前进到新的 commit |
| 审计轮次 | 3 轮；每轮均复核上一轮问题并把未闭环项提升为阻断条件或后续验收项 |

总体判断：

1. **工程质量和契约门禁明显变好**。本地 TypeScript verify、Rust 全工作区测试、Clippy、格式、85 条命令面、文档引用、无跨 await 持锁等均通过。
2. **不能把当前 main 判为“所有核心链路已贯通”**。Process、WASM、商城更新、分发升级、Shell 运行时仍有明确的模拟或未接入边界。
3. **最严重的新问题是 WASM supervisor 的成功语义仍可能脱离模块加载事实**：`execute()` 不要求模块已加载，且实例池以 `instance_id` 存储，却按 `plugin_id` 查找，导致复用和统计链路失真。
4. **V6 的重复 Runtime 装配问题没有完全消失**。当前已有 `OnceLock` 和错误日志，但冲突时仍继续构造并返回第二个 Runtime，恢复写回口仍可能指向旧 Registry，split-brain 风险仍在。
5. **升级执行器仍不是生产升级器**：下载、验签、备份、解压、替换、回滚、重启都存在模拟或未闭环步骤；测试通过的是“拒绝伪成功”的契约，不是真实文件升级。
6. **Shell 和懒加载层存在异步代际问题**：停止后旧的 `start()` 可把实例重新写回 `ready`；懒加载 `reset()`/`clear()` 丢弃在途记录但不取消旧任务，旧任务仍可能写状态或通知。

本报告只做审计和优化方案生成，没有向 GitHub 仓库写入源码、创建 PR 或修改远端分支。文中“修正”表示 V7 要求的设计闭环和门禁，不把计划误写成已经落地的代码修复。

## 2. 验证证据

### 2.1 本地通过项

| 验证项 | 结果 |
| --- | --- |
| `pnpm verify` | 通过：workspace build、TypeScript typecheck、全部 TS tests |
| `pnpm lint` | 通过，ESLint 0 warning/error |
| `pnpm format:check` | 通过 |
| `pnpm fmt:rust:check` | 通过 |
| `pnpm version:check` | 通过，版本统一为 `1.1.0` |
| `pnpm docs:check` | 通过，6 个文档、235 条行号引用有效 |
| `pnpm command-surface:check` | 通过，85 条命令，孤儿命令 0，未归类 0 |
| `node scripts/check-no-lock-across-await.mjs` | 通过，42 个 Rust 源文件 |
| `node scripts/check-target-matrix.mjs --check` | 通过，Linux x64、Windows x64、macOS ARM64、macOS x64 |
| `node scripts/generate-public-surface-ledger.mjs --check` | 通过，85 条公共命令无孤儿元数据 |
| `node scripts/generate-release-evidence.mjs --check` | 通过，版本、Rust/Node、四目标矩阵、47 个 pinned actions 输入一致 |
| `pnpm lint:rust` | 通过，workspace/all-targets Clippy，`-D warnings` |
| `cargo test --workspace --locked` | 通过；Adapter 默认 272 tests，WASM 53，Distribute 59，Process 30，Host conformance 7，其余 crate/doc tests 亦通过 |
| `cargo test -p tauron-adapter --features tauri --locked` | 通过，301 tests；Tauri doc test 1 passed、4 ignored |
| `cargo test -p tauron-adapter --features plugin-install --locked` | 通过，293 tests |
| `cargo test -p tauron-adapter --features tauri,plugin-install --locked` | 通过，327 tests；Tauri doc test 1 passed、4 ignored |

### 2.2 本地无法代替 CI 的项目

| 项目 | 状态 | 解释 |
| --- | --- | --- |
| `cargo deny check --workspace` | 未执行成功 | 本机未安装 `cargo-deny`；不能据此推断 CI 结果，必须保留 GitHub hard gate |
| `scripts/verify-source-ci.mjs` | 未执行成功 | 该脚本要求 GitHub Actions 注入 `GITHUB_REPOSITORY/GITHUB_SHA/GITHUB_TOKEN`，本地缺少 CI 证据上下文 |
| performance/size gate | 未执行成功 | `check-performance-size.mjs` 需要 Linux runner 生成的 probe、FFI 和 Local Host artifact |
| 真实 registry 发布 | 未验证 | crates.io/npm 实际发布态仍需由 publish workflow 的 clean consumer 验证 |
| 四平台桌面打包 | 未在本机全量复现 | 审计原句写作「MSI 以 CI 为准」，**这句不对**：`release.yml` 的 windows 任务显式传 `args: '--bundles nsis'`，MSI 是被**主动摘掉**的（WiX 下载不稳，已作为已知债登记在 CHANGELOG），CI 现在根本不产 MSI。linux 与两个 macos 任务传空 `args`，落到 `tauri.conf.json` 的 `bundle.targets`。本机只复现过 NSIS + MSI 分两次 `tauri build` |
| 真 sidecar 全链路 | **已验证**（原句已被推翻） | 审计原句是「当前仓库没有可执行的正式 sidecar fixture；Rust 侧主要使用 fake spawner」。现在有 `crates/tauron-test-sidecar`：`[[bin]] tauron_test_sidecar` 是真二进制，`tests/sidecar_e2e.rs` 用 `env!("CARGO_BIN_EXE_tauron_test_sidecar")` + 生产 `CommandSpawner` / `ProcessFrameSinkImpl` / `Registry` 起**真 OS 进程**跑完整帧回路（`real_sidecar_round_trip_settles_and_disable_kills_the_process` 等，无 `#[ignore]`），且它是 workspace 成员、随 `cargo test --workspace --locked` 在 CI 里跑。**边界**：`publish = false` 并被 `scripts/publish-crates.mjs` 排除——它是测试夹具，不是发布产物；"真实第三方 sidecar 的兼容性"仍以集成方自己的进程为准。**轮 30 补记**：这条链路真跑通了，但它**报错**的方式有过一段假陈述——适配层把 sidecar 写侧的四种失败（写队列满 / 通路没了 / 帧超上界 / 该启动器不支持写）一律塌成 `E_STATE_INVALID_TRANSITION`，只在并行门禁抢 CPU 时才露馅（洪峰用例「只该看到成功与受理满两种结局」成片红）。轮 30 按事实分码并加负载无关的注入用例；本行原句"端到端已验证"当时为真，只是验证的覆盖面里没含错误码分流这一维。 |

## 3. 三轮审计方法

### 第 1 轮：拓扑、调用链和前后端贯通性

检查内容：

- 从 TS SDK、Host client、Shell client 到 Tauri invoke、Adapter command、Registry/Recovery/Runtime 的调用方向。
- 85 条命令是否有 TS 类型、Rust handler、权限表、文档和测试对应物。
- 插件安装、启用、调用、事件、窗口、配置、恢复、卸载是否能从入口走到真实副作用或明确失败。
- Process、WASM、Local Host、Remote Host、market/update 的真实度标记是否贯穿到前端。
- ServiceGraph 是否真的负责启动/关闭，还是只做 DAG 校验。

第 1 轮结论：命令面和 wire contract 基本贯通；真正断点集中在“被标记为 simulated 的功能是否被下游错误当作成功”、WASM supervisor 的加载事实、升级器的文件副作用以及真实 sidecar/Remote transport。

### 第 2 轮：并发、锁顺序、生命周期和资源

检查内容：

- `await` 前后锁是否跨越异步边界。
- 调用、租约、进程、事件订阅、设置 watcher、懒加载、Shell、WASM 实例的回收路径。
- reset/clear/stop/destroy 与旧异步任务之间的代际关系。
- LRU、队列、重试、超时、回滚循环是否有明确终止条件。
- 旧进程回复、旧 generation、重复 settle、并发 install/uninstall 是否会穿透当前状态。

第 2 轮结论：Rust 锁纪律和旧 process generation 保护较完整；新发现集中在 TS 侧代际取消缺失、Shell 状态 resurrection、WASM 实例 key 错配，以及升级临时目录/错误吞掉等资源一致性问题。

### 第 3 轮：恢复、升级、发布和真实运行证据

检查内容：

- 安装包验签、安装目录、ACL、回滚镜像、恢复 marker、crash/safe-mode。
- market check/download/install 与 `AutoUpdateClient` 的 simulated 传播。
- 分发升级状态机的每一步是否真的改变文件、是否可重试、是否可回滚。
- 发布版本、registry consumer、target matrix、release artifact、MSI/NSIS、performance/size、cargo-deny。
- Local Host UDS/named pipe、Remote TLS reference、真实 sidecar E2E 的证据边界。

第 3 轮结论：安全和“不要伪报成功”的契约较强，但发布链仍把“源码和契约就绪”与“真实 registry/真实桌面包/真实运行时已验证”分开；V7 必须继续保持这种区分。

## 4. V6 遗留项逐项复核

| V6 项目 | 当前判断 | 证据和 V7 要求 |
| --- | --- | --- |
| 重复 `PluginRuntimeState` 装配 | **部分修复，仍未闭环** | `OnceLock` 能防止替换写回口并打印冲突，但 `with_substrate_and_spawner()` 仍返回第二个 Runtime。必须改成显式 `Result`/单例装配 token，冲突时不得返回可用 Runtime。 |
| Process spawn/stdin/stdout/settle/kill | **Rust 侧真实，端到端未证实** | `tauron-proc` 有真实 `Command`、frame sink、process group/Job Object；但正式 sidecar fixture 和宿主到 sidecar 的运行期证据缺失。 |
| Process Hard Sandbox | **仍开放** | 默认 Unix process group / Windows Job Object 只能给 partial containment，filesystem/network/syscall 隔离未内建；生产 readiness 已 fail-closed，但生产能力尚未完成。 |
| WASM `execute()` 模拟 | **仍开放且新增逻辑缺陷** | `crates/tauron-wasm/src/execute.rs:214` 仍以模拟结果返回成功；同时未强制 load/cache 命中，实例复用 key 错配。 |
| market search/update | **仍是诚实模拟**（审计当时的结论；**轮 29 改了一半**） | Adapter 返回 `simulated: true`，TS `AutoUpdateClient` 会阻止进入下载/安装；安全，但功能链尚未接通真实网络、包、验签和安装器。**轮 29 的事实补充**：本行把"检查"与"下载/安装"混为一谈了——宿主另有**真实现**的 `host_updater_check`（轮 13 就接好），而 SDK 的检查当时读的偏偏是那张桩表，于是真能力被诚实的桩挡住（宿主注入端点也永远报"无更新"）。今天检查腿改读真通道（**轮 31 把壳层那条也改了**：`ShellController` 的「检查更新」此前同样读桩且丢弃返回值，现在与 SDK 共用同一份判定），**下载/安装仍是桩**：真实的"接不通"的部分只剩装配腿（`UpgradeRunner` 零外部引用）。 |
| `tauron-distribute` rollback/restart | **仍开放** | 当前回滚明确报“恢复尚未接入”，restart 只返回 `Ok(())`，`run()` 也不调用 restart。 |
| ServiceGraph | **仍是 DAG 校验器** | `startup_order()`/`shutdown_order()` 给出顺序，但没有 service provider、启动句柄、失败补偿和真正的逆序 close。 |
| Adapter 巨型文件 | **仍开放并继续是维护热点** | `crates/tauron-adapter/src/lib.rs` 约 17,187 行，另有 Tauri/领域模块；应按 assembly、commands、install、runtime、settings、recovery 拆分。 |
| TS 双轨 API | **仍需收敛** | `@tauron/host`、`@tauron/core`、plugin SDK 与 app SDK 仍有边界重复；V7 要建立 canonical host contract 和兼容层退出时间表。 |
| plugin-install canonical path | **当前特性测试已通过** | `tauri,plugin-install` 327 tests 通过，包含 `install_config_root_is_canonical_so_window_asset_prefix_matches`；仍需依赖同 SHA GitHub CI 证据确认发布链。 |
| Settings/Recovery 锁顺序 | **当前门禁通过** | Rust 全 workspace tests、no-lock-across-await、settings 并发/迁移/rollback 测试均通过；继续把门禁保持为硬失败。 |
| 版本/命令面/文档台账 | **当前通过** | version、command surface、public ledger、doc line refs 均通过；发布前不得绕过。 |
| registry/npm/crates 实际发布 | **仍未闭环** | 发布脚本和版本检查可通过，不等于 registry 已存在 1.1.0 或 clean consumer 可安装。 |
| Windows MSI | **仍开放** | release workflow 当前显式 `--bundles nsis`，WiX/MSI 仍未恢复。 |

## 5. 新增问题清单

### V7-P0-01：WASM execute 未验证模块已加载

位置：`crates/tauron-wasm/src/execute.rs:214-326`。

当前路径：

1. `execute()` 验证配置、host function 白名单、崩溃预算。
2. 尝试 `acquire_active_lease()`，但没有把“没有 active module generation”当作失败。
3. 以 `plugin_id` 调用实例池。
4. 即使没有先调用 `load_module()`，也可以创建实例并返回 `success: true`。
5. 未注册的 host function 还会生成 `mock_<fn>` 成功结果。

风险：调用方看到成功，但没有任何 WASM 字节、ABI、导出函数或模块 generation 被执行。这是“成功语义脱离真实副作用”的核心断链，后续一旦把 WASM broker 暴露给生产命令，会变成错误执行或安全边界绕过。

修正：

- `execute()` 必须先按 `plugin_id + generation/hash` 查找 active cache entry；不存在时返回明确的 `E_PLUGIN_TYPE_NO_RUNTIME` 或 `E_WASM_MODULE_NOT_LOADED`。
- `load_module()` 必须完成字节读取、hash、ABI、导出函数、host function 映射和 generation 注册。
- 未注册 host function 必须失败，不得返回 mock success。
- 将 `module_path`、`interface_hash`、`function_name`、`args` 全部接入真实 engine invocation；没有 engine 时直接 fail closed。
- 增加“未 load 不能 execute”“已删除 generation 不能 execute”“未知 host function 不能成功”的 wire 和 Rust tests。

验收：任何 `success: true` 都必须能关联到可查询的 `module_hash/generation/instance_id` 和真实执行计数。

### V7-P0-02：WASM 实例池按错误 key 查找，复用和统计失真

位置：`crates/tauron-wasm/src/lib.rs:318,397`、`crates/tauron-wasm/src/execute.rs:254,349,376-385`。

`InstancePool` 用 `instance_id` 作为 map key；`execute()` 却用 `plugin_id` 调用 `get_instance()`。结果是：

- 空闲实例复用分支基本不会命中。
- 每次执行倾向于创建新实例，达到上限后产生不必要的 LRU churn。
- `execute_with_recovery()` 出错时无法按 plugin 找到实际 instance 做回收。
- `plugin_stats().instance_count` 只会查询一个不存在的 plugin key，容易长期报告 0。
- `host_fn_calls` 通过 `fn_name.contains(plugin_id)` 过滤，但记录只保存函数名，没有 plugin_id 维度，统计也会长期失真。

修正方案二选一但必须统一：

- 方案 A：增加 `plugin_instances: plugin_id -> active/idle instance ids` 二级索引，实例本体仍按 instance id 存储。
- 方案 B：`get_idle_instance(plugin_id)` 直接遍历/索引 plugin owner，不再把 plugin id 当 instance id。

同时在 `HostFnCallRecord` 加入 `plugin_id`、`module_generation`，统计按结构化字段过滤；禁止字符串 contains 作为归属判断。

验收：连续 100 次执行在池容量足够时复用实例；统计 instance_count、host_fn_calls、generation 与实际记录一致；clear/reclaim 后无悬挂实例。

### V7-P0-03：升级执行器仍可能报告“升级完成”但没有升级文件

位置：`crates/tauron-distribute/src/upgrade.rs:286-469`。

实际问题：

- `download_timeout_secs` 只有配置字段，没有被 downloader 约束。
- 没有 Downloader 时创建 `mock-update-content` 空文件。
- 没有 Verifier 时只要 signature 非空就通过，未做密码学验签。
- `backup_current()` 只写 `backup-info.txt`，没有复制安装目录。
- `extract_update()` 只写 `version.txt` 和 `release-notes.txt`，没有解压 zip，也没有 zip-slip 防护。
- `replace_files()` 只删除临时目录，未替换安装目录；还忽略了 `remove_dir_all` 错误。
- `run()` 返回 `backed_up: true`，即使安装目录不存在或没有复制内容。
- `run()` 不调用 `restart()`，永远返回 `restarted: false`。
- `restart()` 在 auto_restart=true 时只返回 `Ok(())`。
- `rollback()` 检查到 metadata 后仍明确报“备份文件恢复尚未接入”。

风险：若此模块被任何非 `simulated` 路径直接消费，会出现状态机完成、UI 显示成功、文件实际未替换的断链。

修正：

1. 设计 `UpgradeState` 的持久化状态机：Downloaded → Verified → BackedUp → Extracted → Swapped → HealthChecked → Restarted/Committed。
2. 所有状态必须记录 `operation_id`、旧版本、新版本、包 hash、备份路径、临时路径和 commit marker。
3. 备份使用同文件系统临时目录 + fsync + atomic rename；备份成功的定义是可读取的完整目录或可验证 archive。
4. zip 解压做 canonical path 校验，拒绝 `..`、绝对路径、符号链接逃逸和大小超限。
5. 替换采用 staging/current/previous 三目录或等价 atomic swap；任何一步失败必须能恢复 current。
6. 验签器缺失时硬失败；不能因为签名字段非空而通过。
7. timeout 必须覆盖下载、解压、替换和健康检查，并且取消后清理临时文件。
8. restart 必须注入 `RestartProvider`，返回真实 spawn/hand-off 结果；不能用 `Ok(())` 代表已重启。

验收：用真实临时安装目录和真实 zip fixture，验证升级前后文件 hash；在下载超时、坏签名、zip-slip、磁盘满、替换中断、重启失败、恢复后重启等场景下，状态、文件和返回值三者一致。

### V7-P1-01：重复 Runtime 装配仍可能产生恢复 split-brain

位置：`crates/tauron-adapter/src/lib.rs:3310-3400`、`crates/tauron-adapter/src/tauri.rs:3392-3425`。

当前已有改进：底座用 `Arc` 共享，`plugin_flags` 和 `process_sandbox` 用 `OnceLock`，重复注入会打印冲突日志，V6 的静默覆盖已被消除。

仍存风险：第一次 Runtime 的 `RegistryFlagSink` 留在底座内，但第二次调用继续创建并返回新的 Registry/Runtime。新 Runtime 的调用方看到的是新 Registry，恢复写回却仍写旧 Registry。日志不能替代状态不变量。

修正：

- 将 `with_substrate*` 改成 `Result<Self, AssemblyError>`；任何 `OnceLock::set` 冲突都返回 `AlreadyAssembled`。
- 或在 `SubstrateState` 保存不可伪造的 `assembly_id`/`runtime_handle`，重复装配只能返回既有 Runtime 的显式引用。
- Tauri `init()`/`state_init()` 同时使用时必须在 setup 阶段返回结构化错误，不让 Tauri 运行到 managed state panic。
- 增加“同一 substrate 第二次装配失败且旧 sink/registry 不变”的并发测试。

### V7-P1-02：懒加载 reset/clear 没有异步代际取消

位置：`packages/tauron-host/src/lazy-plugin-loader.ts:66-181,261-320`。

当前 `reset()`/`clear()` 会删除 `_inflight`，但不会取消已经开始或已排队的 promise；`_doLoad()` 仍持有旧 descriptor/旧 cache entry，并在完成时写入旧 entry、发送旧通知。文件注释已经承认“不做代际取消”。现有测试只验证旧 promise 能 settle，并没有验证旧任务在新注册后不能污染新一代状态。

风险：

- `clear()` 后重新注册同 id，旧加载完成时可能通知 `loaded/failed`，让调用方误认为新 descriptor 已完成。
- `reset()` 后新加载会排在旧 `_loadQueue` 后，旧 descriptor 卡住时，新代际也被拖延。
- 旧 entry 的成功实例可能继续持有资源，但对外已经不可见，形成逻辑孤儿。

修正：为每次 register/reset/clear 分配 generation；inflight 记录 `{generation, promise, abort}`；完成时只有 generation 与当前 cache 一致才提交状态/通知；支持 AbortSignal 的 entry 取消，不能取消的 entry 至少要丢弃结果并记录 abandoned load。

验收：旧任务晚于新任务完成时，旧结果不能改变新状态、不能触发新一代 loaded 通知、不能覆盖新实例；clear 后进程可退出且没有未清理 timer/worker。

### V7-P1-03：Shell start/stop 存在 resurrection 和错误吞掉

位置：`packages/tauron-shell-matrix/src/manager.ts:16-56,66-104`。

当前四种 Shell 的 `start*` 都是 10ms 延迟模拟；`start()` 捕获异常后把 status 设为 error，但 resolve 成功，不向调用方 reject。更重要的是：

- `stop()` 在 start 尚未结束时设置 `unloaded`；旧 start 完成后又把实例写成 `ready`。
- `start()` 没有 generation 或 AbortSignal，重复 start/stop 可能乱序覆盖状态。
- `destroy()` 删除 map 前只等待 stop，无法证明所有异步启动任务已经失效。

修正：引入 `ShellGeneration` 和状态迁移表；start 返回/持有 generation token，完成时校验 token；stop/destroy 使 token 失效并等待可取消任务；启动失败必须 reject 或返回结构化失败，不允许 Promise resolve 但状态为 error。

真实功能接入前保持 `simulated: true`，并要求所有上游继续阻止 simulated 进入可用安装/更新路径。

### V7-P1-04：Dual-world destroy 后仍可调用和写状态

位置：`packages/tauron-dual-world/src/sandbox.ts:49-188`。

`destroy()` 清理 pendingCalls、handlers、state，但没有 `destroyed` 标志；destroy 后 `call()` 仍可执行 host function，`setState()` 仍可写入新状态，`emit()` 仍可进入空或后来重新注册的 handler。`pendingCalls` 当前没有实际加入调用流程，属于孤儿清理状态；id 使用 `Math.random()`，不适合安全归属或诊断关联。

修正：

- 添加一次性 `destroyed` 状态，destroy 后 execute/call/setState/getState/emit/on 全部按统一错误码处理。
- 所有 host function invocation 检查 destroyed 和 generation。
- 若当前没有真正 pending call，就删除该 map；接入异步 transport 后再由真实请求登记。
- 使用 monotonic counter + cryptographic/random UUID 组合生成实例 ID，禁止把 Math.random 当作安全身份。

### V7-P1-05：真实 Process/Remote 运行证据仍缺一层

当前 `tauron-proc` 已真实 spawn、写 JSON-RPC frame、读 stdout、处理 EOF、绑定 lease，并有 process group/Job Object 设计；但仓库没有正式 sidecar executable fixture，测试不能证明“sidecar 真收到请求、回包、异常退出、重启后旧回复被拒绝”的运行期闭环。

Remote Host 有 TLS1.3 reference 和 contract chaos tests，但 reference 不等于产品 transport adapter。Local Host 的 UDS/named pipe reference 也不代表 Tauri/真实宿主进程已消费同一通道。

修正：新增受控的 `tauron-test-sidecar` fixture，仅用于 CI：

- 读取 manifest/ABI handshake，回显 request id 和 generation。
- 能按命令延迟、乱序、断 stdout、退出、返回超大 frame。
- CI 在 Linux/Windows/macOS 各运行一次真实 spawn → frame → settle → kill/restart。
- Remote 增加真正 socket transport adapter 测试；reference 只保留为 contract fixture。

## 6. 核心流程贯通图与断点

```text
TS SDK / UI
   │  typed request + caller identity
   ▼
@tauron/host / shell-client / auto-update-client
   │  Tauri invoke / plugin command
   ▼
tauron-adapter command boundary
   │  authz → validation → registry/recovery/event bus
   ├───────────────┬────────────────┬──────────────────┐
   ▼               ▼                ▼                  ▼
Install/ACL      Process runtime   WASM runtime       Market/update
   │               │                │                  │
   ▼               ▼                ▼                  ▼
package files   sidecar/lease     module/cache       simulated result
   │               │                │                  │
   ▼               ▼                ▼                  ▼
enabled/call    frame/settle      current stub       AutoUpdate blocks
```

当前真正贯通的部分：

- TS ↔ Rust wire shape、camelCase、错误码、身份绑定和权限表。
- JS package install/preview/verify/ACL/enable/call/uninstall 的契约链。
- Process runtime 的 Rust 内部 lease、generation、settle、crash/recovery 逻辑。
- settings transaction、migration、rollback image、recovery marker 的测试链。
- Local/Remote reference 的协议/安全约束测试。

当前仍断开的部分：

- WASM bytes → real engine → host function → result。
- market manifest → real download → real signature verification → staging → install。
- UpgradeRunner → real backup/extract/swap/health check/restart。
- Shell config → actual local file/server/webview/remote transport。
- Process adapter → formal sidecar fixture → real cross-platform E2E。
- Remote Host contract → production socket transport consumer。

“断开”不等于“代码不应存在”。对未实现能力，必须维持 `simulated` 或 `E_PLUGIN_TYPE_NO_RUNTIME` 的显式失败；真正的问题是任何路径都不能把它转换为产品成功。

## 7. 死循环、死锁、资源泄漏专项结论

### 已验证的安全点

- `check-no-lock-across-await` 通过；settings、recovery、registry 的关键锁顺序有测试。
- Settings migration 有 self-loop 检测，不会无限迁移。
- Registry LRU 淘汰每轮至少移除一项，避免递归淘汰不终止。
- Process sink 对旧 generation、EOF、未知 pid 有保护；frame 有最大字节上限。
- EventBus、settings watcher、adapter ring log、contributes 注册都有容量/清理测试。
- `AutoUpdateClient` 已在 `simulated` 时拒绝 download/install，避免把模拟数据直接推进为更新。
  **轮 29 另修一条**：它的**检查**过去读宿主桩 `host_market_check`、今天读真通道
  `host_updater_check`（`currentVersion` 必填入参；`Unsupported`/`degraded` 落 `'error'`
  而非 `'idle'`），所以"诚实的桩"不再顺带把真更新能力挡在外面。
  **轮 31 修掉同一条链的壳层腿**：`<oc-updater-dialog>` 的「检查更新」过去打桩且**把返回值
  丢掉**——同一次点击在 SDK 侧报得出 `UpdateAvailable`、在对话框侧永远显示"没有更新"。
  现在它也走 `host_updater_check`，判定与 SDK 共用导出的 `toUpdateInfo`（一处定义），
  结果经 `onUpdaterCheck` 交给接入方驱动 `version` / `message`。
- **轮 32 收口这条链的状态词表**：更新"到哪一步了"曾经有三套平行字面量——契约事件名、
  `AutoUpdateClient.UpdateStatus`（安装成功终态 `ready`）、`<oc-updater-dialog>` 的渲染分支
  （终态 `done`）。没有任何生产者产出 `done`，于是「立即重启」在真装配里**点不出来**
  （`oc-restart` → `host_window_relaunch` 整条腿死着），而 `ready` 落进兜底分支显示
  「开始更新」，点一下会把下载+安装重跑一遍。今天词表与「状态 → 主按钮动作」映射都在
  `@tauron/shell-events`（`UPDATER_STATUSES` / `UPDATER_PRIMARY_ACTION`），写侧
  `UpdateStatus` 是它的类型别名、读侧组件的 `status` 是它的类型，`ready` 是唯一能点亮
  重启按钮的状态。第三套状态机 `UpdaterStore` 无生产消费者，登记进孤儿账而非静默删除。
- **轮 33 收口这条链的读侧**：`host_updater_status` 是灰度批次、崩溃门禁、宿主进程内更新
  账本这三件事实的唯一出口，此前在 TS 侧**零消费者**——于是"不在灰度批次""被崩溃门禁停发"
  "宿主没装端点"与"确实已是最新"在 UI 上塌成同一句"没有更新"。现在 SDK 的 `checkUpdate()`
  与对话框的检查按钮共用同一个 `enrichUpdaterInfoWithChannel`（两条入口各拼一句就是第三面
  镜像），示例 `describeUpdate()` 把并进来的依据真打印出来——"没有更新"不再是光秃秃的结论。
  另一半：账本 `state` 现有写入方全是桩（`host_market_download` / `host_market_install`），
  `installed:<v>` 这个字符串分不出出处，所以线形加 `stateSimulated` 并与 `state` **成对读**——
  成对做到装配形状上：`UpdaterSink::status` 的账本对是必传参数，provenance 由 sink 推导而非
  自述（首版让 sink 写 `state_simulated: false`，被 `check-simulated-never-commits.mjs` 判红，
  放宽 needle 等于绕过门禁，故改成"这种断言写不出来"）。桩阶段恒 `true`，决定它的是**账本
  写入方**的 provenance 标记（`UpgradeRunner` 装配腿落地后把 `update_state_simulated` 写
  `false`），本轮没有伪造真实推进。诊断不改判定（命令缺席/报错/答空都原样返回结论），且加的是返回字段不是新命令——
  85 条命令面冻结未破。
- **轮 34 收口"施工单与门禁相反"这一类**：三份文档把已经删除、且被并行门禁**反向钉住不许回来**
  的形状写成待办——A75 要求删掉 `service_startup_order`/`service_shutdown_order`（轮 11 已删、
  `wire-gate` 有反向 needle），A67/A68 要求新增 `host_protocol_info`（与 85 条冻结断言直接冲突），
  `multi-plugin-substrate-roadmap.md` 的 M-9 仍挂 🟡 却引用已删除的 `bootstrap.ts` 与被禁的
  `maxPlugins: 32` 第二出处。这类问题的代价不是错别字：照着施工单做就会撞红门禁，或把"已消除"
  重新引入。现在三条待办各改成带依据的改判（做/不做/为何不做），`wire-gate` 新增一条按 bullet
  抽取文档正文的 needle 把它们钉住，两条零消费者的历史 SDK 面（`PluginRegistry`、`ConfigManager`）
  登记进孤儿账而非静默删除——登记即机器复核"零消费者"这句话，探针在接线面找到任何用户即红。
- **轮 35 收口"跨 iframe 桥的两半各丢一次码"**：写侧 `PluginBridge` 把 handler 抛出的码硬换成
  `SC-9001`、`retryable` 写死 `false`（与已发布码表相反：`SC-9001` 本就属可重试），读侧
  `PluginContext.invoke` 在局部类型里**声明了** `error.code` 却只取 `message` 造裸 `Error`——
  于是轮 30 在宿主写侧按事实分好的四个码，到前后端最后一跳塌成一句 "Internal error"。
  这条缺口原本就登记在 `@tauron/host/src/errors.ts` 的 `UNWIRED_BOUNDARIES` 注释里，理由
  "插件侧 SDK 不能依赖 `@tauron/host`（依赖方向）"读起来像无解，但它同时给出了解：
  **认码不需要词表**。形态判据（`E_*` 前缀 / `^SC-\d{4}$`）搬进两套词表都能触到的最低层
  `@tauron/types`，宿主侧改为再导出（公开面不变），于是全仓只有一份正则；写侧
  `codeFromThrown` 逐级取码（错误自带 → 消息文本 → 才兜底），读侧 `PluginBridgeError`
  带 `code`/`retryable`/`fallbackApplied`，超时/取消/销毁也第一次带上各自的框架层码。
  `wire-gate` 新增八段门禁（含两条反向 needle、依赖方向、"重试语义不得复制第三份表"、
  两半各有真实测试、面向插件作者的文档已写）。同轮按"用词与文档相反也算断链"抓到两处并改判：
  `SC-####` 在 TS 注释里被称作"应用层词表"（按 `overview.md` 的层表它属**框架层**，
  `tauron-shell` 用的正是这份码），`plugin-development-guide.md` 一处仍写 `HOST_ERROR_CODES`
  按**声明顺序**比对（门禁实为按**码名集合**比对，V4 A69）。
- **轮 36 收口"推方向：`emit` 一路 `Ok` 而零监听方"**：轮 35 之后把同一把尺掉转方向量**事件面**。
  事实是通知链路两头都断——`TauriDispatchSink::send` 每次都把通知 `emit` 到
  `NOTIFICATION_TOPIC` 并返回 `Ok`，而全仓**没有**任何代码监听那个 topic；读端
  `ShellClient.notificationsList()` 只有自己的单测在调。发出端绿灯、接收方不存在，
  "通知已投递"为真而"用户看得到"为假，且不报错——轮 31 那条静默断链在事件面的同型体。
  修法不给新命令（85 条冻结）：`ShellController` 的 `start()` 订阅**信号**后拉
  `host_notifications_list`（按身份过滤，正文的唯一出口），经新增的 `onNotification` 上屏；
  载荷仍 `{ id, pluginId, kind, ts }` **不含正文**，保留轮 11 的跨插件泄露封口；
  在途单拉取 + 收尾补拉（5 条信号 = 2 次命令）、代际令牌管住晚到的回帧与快照、
  订阅/拉取/渲染三个失败出口各归各。线值的唯一事实源是 Rust `pub const`，TS 镜像
  `host-topics.ts` 由门禁比对且全仓不许有第二处字面量。同轮按同一判据清掉三处"发出=送达"
  叙事与 `incremental-adoption.md` 里**同一事实的两处镜像**（§2.2 还写「仓库内没有任何
  `DispatchSink` 实现、也无人注入」，与 §5 和 `tauri.rs` 的 `notify_sink.set(...)` 相反），
  Rust 侧改成**逐 topic 监听方清单**：菜单/通知=有监听方，
  `tauron://dialog-degraded`、`tauron://deep-link-registration`=**零监听方**。
  `wire-gate` 新增八段门禁（正向：线值/订阅/拉取/载荷封闭集/合并/消费者/文档镜像/命令面；
  反向：回调不许吃载荷、示例不许自己监听、docs 不许再写"未接线"、不许为此新增命令）。
- **轮 37 收口"菜单/托盘点击：文档指着一条不通的路"**：轮 36 那句"菜单/通知=有监听方"
  的**菜单半边当时就是假的**（清单里没有任何 `listen`），本轮从这条假行回查实现，挖出三层：
  ①四处叙述（`lib.rs` 字段注释、`tauri.rs` 命令注释、`shell-client.ts`、由它们**生成**的
  `command-surface.md`）都写着点击"经 `host_events_*` 总线、`host_events_drain` 取件"，
  而实现一直是 `AppHandle::emit`——照文档接线的人守着一个永远为空的队列且不报错；
  ②`TauriTraySink` 从未登记过路由，托盘右键点击必然被静默丢弃；③六条菜单/托盘方法
  零消费者零测试。修法仍然**不加一条命令**（85 条冻结，roadmap 那个 `host_menu_on_select`
  提案名继续只存在于提案里）：新增平台无关的 `crates/tauron-adapter/src/menu_routes.rs`
  ——`MenuRouteTable` 按 `MenuLane`（`AppMenu` / `TrayMenu`）分道存 `id → topic`，整条 lane
  随来源一起替换；`TauriMenuSink::new` 只注册**一个**全局监听（Tauri 只有一张
  `global_event_listeners`，托盘点击也进它，第二个"专用托盘监听"会把同一次点击 emit 两遍），
  命中即 `emit`  `{ id, source, native: true }`；登记时机=生效时机（create/set_menu 真的
  挂上才登记、remove 真的移除才清空、`menu_reset` 只失效 `AppMenu`）。TS 侧 `MenuClickFrame`
  / `MenuClickSource` / `MENU_CLICK_TOPIC`（Rust `pub const` 的镜像，字面量全仓单点），
  真实消费者是 `examples/minimal-app` 第 9 节：先 `listen` 再建菜单、按 `source` 上屏、
  五个按钮走完六条命令。`wire-gate` 新增八段（线值与 lane 词表同源、监听与查询点唯一、
  两条 lane 的登记/清空形状、回传帧字段两侧同集合且封闭、消费者集合逐文件冻结、
  Rust+TS 双侧测试同时在册、四处文档镜像同口径、不许新增命令），16 次定向变异逐段变红。
  **同轮被自己的门禁判红三轮**：①`MENU_CLICK_TOPIC` 四处被说成"宿主会替你补的那个值"，而实现只对
  显式填了 `event` 的项发帧、宿主从不回填——措辞已在唯一事实源改正并重新生成命令面；
  ②第一轮变异里有 2 次**没有**变红（`replace(...TrayMenu,` 被 `set_menu` 顶包、README 只查
  类型名时删掉解码行仍绿），补强成盯各自那行后才 15/15；③收尾复跑时形状禁令把**本文档自己**
  判红了两次——一次因为逐字列出被禁形状，一次因为复述那句假话。形状禁令无法区分"引用"与
  "宣称"，所以规则比轮 36 更硬：**被禁字符串在文档里连作为反例都不许出现，只能指向门禁文件**。
  ④第 16 次变异补的是**门禁当时没判红、但按本轮自己的口径站不住**的一处：示例的接收腿当初写成裸
  `void backend.listen(...)`，而 `TauriBackend.listen()` 原样返回 Tauri 的 `listen()` Promise，
  事件系统不可用时会拒绝——届时只剩一条 unhandled rejection，"前端已经接上回传"又变成核对不了
  的承诺。补上失败出口（上屏到 `menu-out`）+ 补 needle 后，"摘掉 catch"确实把⑤打红；同一件事
  顺带暴露**变异锚点也会腐坏**：prettier 把这条链拆成三行后两条旧⑤变异命中数归零，脚本按
  "锚点须恰好命中 1 次"判 SKIP-BAD-ANCHOR，才不会误报成"门禁没咬"。
  **规则**：没被变异证明过的 needle 不算门禁；被门禁判红的文档要先改文档，而不是先给门禁开洞；
  一句"已经接上了"要有失败出口，变异脚本自己的锚点也要复核。
- **轮 38 topic 词表两侧同源**：轮 36/37 的"线值只许写在一个文件"都是出事之后逐条补的；
  把 Rust 全部 `pub const *TOPIC*` 与 TS 侧摊开对照后，同一形状仍在两处——深链接投递腿在
  TS 侧没有常量（裸字面量监听，宿主改名即静默零命中而发出端照旧成功）、两条诊断帧连 TS
  镜像都没有（接入方只能手打线值）。修法 = 逐条规矩升级为**全量表**：`host-topics.ts`
  成为全部五条宿主 `emit` topic 的唯一 TS 镜像（`DEEP_LINK_TOPIC` / `DIALOG_DEGRADED_TOPIC` /
  `DEEP_LINK_NATIVE_TOPIC` 三个新导出，均从 `@tauron/host` 入口导出）、深链接腿改按常量
  监听、B 层（`HOST_SETTINGS_CHANGED_TOPIC` → `@tauron/app-plugin-sdk`）与 C 层三个片段
  （含 `://` 即红）纳入同一张表。`wire-gate` 新增六段（Rust 常量全集 ↔ 三层表逐字相等；
  A 层线值两侧逐字相等；线值在 TS 非测试源**恰好出现一次**且落在声明文件；投递腿禁裸
  字面量并按常量监听；监听方文件集合逐条冻结——两条诊断帧为零集；两侧文档块写明零监听
  事实与权威落点），14 次定向变异逐段变红。同轮三次判红自己 / 被工具腐坏：③"恰好一次"
  先红了新镜像的**文档注释**（注释引用线值也算第二份，改为不复述）；第一次变异 13 次
  只红 11 次（文件集合太松、整文件 needle 被邻居顶包）；prettier 与 rustfmt 各腐坏一处
  手写锚点（`SKIP-BAD-ANCHOR` 守卫与基线校验分别兜住）。**规则**：needle、变异锚点、
  门禁钉的具体代码形状，三者都会被格式化工具腐坏，复跑前都要重查。
- **轮 39 能力可见性**：`isAvailable()` 声称插件侧看不到主窗特权命令，实际只查注册位；
  `host_capabilities`（Rust）返回**构建级**全集、不带调用方参数（设计如此），而"插件视图"
  契约测试用按 `consumer` 预筛的夹具循环取材——宣称只在注释里成立。修法 = 过滤落在 SDK
  `isAvailable()`：双判据 = 注册位 ∧ 白名单主体（`cap.consumer !== 'main-window' ||
  principal === 'main-window'`，畸形主体不按主窗放行）；`capabilityMatrix` 逐条委托，
  `.kind === 'main-window'` 非测试源恰好一次；测试改全量 30 条夹具 + 插件/畸形/主窗三主体；
  `supports()` 与 README 划清"只答注册与否"。`wire-gate` 新增四段，9 次定向变异逐段变红。
  **规则**：注释里的安全宣称要有实现解剖 + 反证测试；循环夹具的绿不算绿。

### 仍需修复或补证的风险

| 区域 | 风险 | 处理 |
| --- | --- | --- |
| Lazy loader | 旧 promise 不取消，可能跨代写状态；队列可能被旧任务拖住 | generation + AbortSignal + abandoned task metric |
| Shell | start 完成晚于 stop 后复活为 ready | generation token + cancel/await |
| WASM | 实例 key 错配导致重复创建、统计失真；模块未 load 仍可成功 | 二级索引 + loaded-generation gate |
| Upgrade | 临时目录清理错误被忽略；真实备份/替换尚未存在 | atomic staging + fsync + cleanup error is fatal |
| Process | 审计当时的事实：`terminate_tree` 失败时保留跟踪句柄但**无重试方**，可能留下存活进程。**轮 26 已改**：失败入队（按 pid 去重、队列上界 64、满则固化并计 `overflow`），既有主窗命令 `host_resource_stats` 每次读数前驱动一轮重试（单条至多 3 次），到顶转成可查询的终态记录 `reap.terminalRecords`（环形上界 16）。**轮 27 补上安全封口**：重试前先核对 pid 归属，被在册租约占用即**让位**并计 `reap.skippedLivePid`——否则"清理孤儿"会变成"杀掉刚起来的活 sidecar"。**轮 41 收口跨重启扫描**：待重试队列随每次脏变更落进恢复数据目录的 `reap-ledger.json`（durable 校验和；撕裂/篡改拒绝启动），下次启动逐条定性——`Gone` 销账（`sweepResolved`），`Alive` / 不可判定只留证据**不盲杀**（`sweepSurvivors` / `sweepUnknown`；跨重启验明不了进程身份，同号可能已被复用）。**仍缺**：宿主自身退出路径的重试方（退出途中无驱动腿——但那次失败已必进台账，下次启动可查、可销账） | 已落：kill retry + evidence + 防误杀（轮 26/27）+ startup orphan sweep（轮 41，门禁＝`wire-gate` 的「V7 §7/§9：kill 失败必须有 retry 或终态证据，两者都有上界，且重试不得误杀同号活进程」与「轮 41：跨重启孤儿扫描只探测不杀，台账可校验且读不开不洗白」两段）；边界：存活/不可判定只留证不盲杀（残骸交操作者按证据处置） |
| Dual-world | destroy 后 API 仍可使用；pendingCalls 是未消费字段 | destroyed guard；删除或接通 pending map |
| Runtime assembly | 第二 Runtime 继续返回，旧 sink 与新 registry 分离 | 装配冲突 hard error |
| ServiceGraph | 只有顺序，没有 provider close/rollback | 实现 lifecycle executor 或明确降级为纯 validator |

> **本表各行是 V7 审计当时的事实陈述。** 轮 28 对全部 7 行 + §2.2 逐行复核过代码，
> 结论如下（判据是本仓口径「已落地 = 真代码 + 真生产消费者 + 门禁」；证据一律给**符号**，
> 不给行号，以免下次编辑就漂）。落地过程的逐轮实录在
> `docs/architecture/v4-industrial-gap-closure-plan.md`。

| 行 | 轮 28 复核结论 | 证据（符号）与仍成立的边界 |
| --- | --- | --- |
| Lazy loader | 已闭合 | `LazyPluginLoader` 的 `_generations` / `_invalidate()` / `_isStale()` 构成逐 id 代际令牌，`_doLoad` 的每一次写（状态、实例、错误、`_notify`）都在闸门之后；`load(id, { signal })` 与超时/`load.release` 竞速且必清 `setTimeout`。有界读数 `stats()` 的 `abandonedLoads` / `discardedResults` / `activeTimers`。**边界**：不协作的 entry 是「丢弃结果」而非「真取消」——这条口径写在注释与测试名里，没被写成"已取消" |
| Shell | 已闭合 | `packages/tauron-shell-matrix` 的 `createShell()` 持 `generation` + `inFlight`，`commit()` / `writeStatus()` 同时校验令牌与 `SHELL_STATUS_TRANSITIONS`，`stop()` 在第一个 await 之前就升代并 await 在途；`ShellStartAbandonedError` / `SHELL_START_ABANDONED` / `ShellManager.stats().abandonedStarts`。**边界**：四个 `start*` provider 本身仍是 `simulated: true` 的 10 ms 模拟——代际纪律是真的，被代际保护的"真启动"不是 |
| WASM | 已闭合 | `InstancePool` 的 `plugin_index` + `InstanceBinding{ generation, module_hash }` 是二级索引，`find_idle_instance` / `discard_stale_instances` 按代际复用；`WasmExecuteEngine::execute` 对未加载模块返回 `WasmError::ModuleNotLoaded`，另有 `StaleInstanceGeneration`。门禁 `scripts/check-wasm-loaded-generation.mjs`。**边界**：没有新增 `E_WASM_MODULE_NOT_LOADED` 这类**线码**（审计原本建议的那个），失败停留在类型化 `WasmError` 层 |
| Upgrade | **部分**（清理腿已闭合；轮 40 装配腿落地到适配层 seam，网络实现仍由装配方注入） | 已闭合：临时目录清理错误不再被吞——`with_cleanup_note()` / `remove_tree_if_exists()`，且 `run_phases` 的清理步会以 `FileOperationFailed` 失败上抛。真备份/真替换在 `UpgradeRunner::run_phases` 里（`copy_tree_fsynced` + `tree_hashes` 复核 → `fs::rename` 旧树入 `PREVIOUS_DIR` → staging 转正 → `rollback_after_swap` / `restore_from_backup` + 日志 `mark_committed`），测试 `run_success_performs_real_file_effects_and_journal_sequence`、`rollback_after_success_really_restores_old_tree`。**轮 40 复核（结论更新）**：仓内消费者落位——适配层 `DistributeUpgradeInstaller`（`UpgradeInstaller` seam，runner 构造点全仓仅此一处）把 `host_market_download` / `host_market_install` 接成真下载/真安装（staged 槽位 + `download_bounded` + `verify_package` + 完整 runner 的备份/交换/健康检查/提交/自动回滚；账本 provenance 如实翻转），命令按 `native_supported()` 分派、缺省装配仍是如实模拟。**仍成立的边界**：生产 HTTP 下载器与 ed25519 验签器仍由装配方注入（仓内无联网/证书实现），`host_market_check` 仍是有意的桩，端到端联网升级无仓内实证。SDK 侧不会替它圆场：`downloadUpdate()` / `installUpdate()` 见 `simulated` 一律抛错 |
| Dual-world | 已闭合 | `sandbox.ts` 的一次性 `destroyed` + `assertAlive()` 覆盖 `invoke` / `emit` / `on` / `getState` / `setState`，`execute` / `call` 走 `destroyedFailure()`（`SANDBOX_DESTROYED`），`destroy()` 幂等。审计点名的两处都按「不留第三态」处理：`pendingCalls` **删除**（没有异步传输就没有可接的对象），id 改为 `generateId()` = 模块内单调计数 + `crypto.randomUUID()`，并有测试盯着 `Math.random` 不被使用 |
| Runtime assembly | 已闭合 | `PluginRuntimeState::with_substrate` / `with_substrate_and_spawner` 返回 `Result<Self, AssemblyError>`，第二个 Registry **根本不会被构造**（`AssemblyError::AlreadyAssembled{ existing, attempted }`，凭据存 `SubstrateState.plugin_runtime_assembly` 的 `OnceLock`）；测试 `second_assembly_on_same_substrate_returns_no_runtime` |
| ServiceGraph | **仍成立**（按处方的第二分支处置，而不是伪造闭合） | 结构仍是 `ServiceNode{ id, requires }`——没有 provider/handle/close 字段；`ServiceGraph` 对外只有 `startup_order()` / `shutdown_order()`，而 `shutdown_order()` **零生产调用者**。适配器 `canonical_substrate_service_graph()` 只消费校验结果（失败即 `panic!`）。审计给的二选一里取的是"明确降级为纯 validator"，且 `wire-gate` 有一条测试反向钉住"没有假造 lifecycle 执行器"。真执行器（provider close/rollback）仍是 V7 的开放项 |

所有新增 async 测试都要使用 deterministic deferred promise，不使用固定 sleep 作为唯一竞态证明。

## 8. V7 实施批次

### Batch 0：发布阻断和语义硬化

目标：先避免任何“没有副作用却返回成功”的新路径。

- 为 `simulated`、`E_PLUGIN_TYPE_NO_RUNTIME`、`Unsupported` 建立统一 success policy。
- UpgradeRunner 默认没有真实 downloader/verifier/installer 时必须返回 typed unsupported，不创建 mock update file。
- WASM 未 load、未注册函数、无 engine 时必须失败。
- 重复 Runtime assembly 改为 hard error。
- Shell start error 必须 reject/typed failure；destroyed sandbox 禁止继续调用。
- CI 增加 grep/AST gate：`simulated: true` 不得进入 download/install/commit；`UpgradeResult.success=true` 必须伴随真实 commit marker。

验收：构造任何 simulated adapter，UI 只能显示“未接入”，不能显示“已下载/已安装/已更新”。

### Batch 1：WASM 真执行或彻底隔离

优先级：P0。

1. 定义 `WasmRuntimeProvider` trait：load、validate ABI、instantiate、invoke、interrupt、destroy。
2. `WasmModuleCache` 以 plugin/generation/hash 为权威键。
3. `InstancePool` 增加 plugin owner index 和 generation index。
4. host function 调用携带 plugin identity、capability、deadline、cancellation token。
5. 内存页、执行时间、host call 次数、crash budget 全部由 engine 真实计量。
6. 没有 provider 时只暴露诊断和 fail-closed error，不暴露 success-shaped result。

最低测试集：

- load→execute→result。
- 未 load execute 拒绝。
- generation replace 时旧实例不能执行。
- timeout interrupt 后实例不再复用。
- host function deny/unknown/over quota。
- 100 次 execute 复用和 clear。
- provider panic/engine trap 后 crash budget、recovery、metrics 一致。

### Batch 2：统一 RuntimeDriver 与 Process production profile

- 用 `RuntimeDriver` 统一 JS/Process/WASM 的 load/start/call/health/stop/force_kill/dispose。
- `ProcSpawner` 的 ABI 值必须来自 sidecar handshake、签名 manifest 或可信构建产物，不能由调用方自报。
- 签名/hash 做真实 verification，不只检查 64 字符格式。
- Hard sandbox capability 必须按平台声明并在 production readiness 中验证。
- 引入真实 sidecar fixture，覆盖 frame、crash、backpressure、超大帧、旧 generation。
- 失败状态必须区分 `NotStarted`、`Exited`、`KillFailed`、`HandshakeRejected`，不能只映射成一个通用 error。

### Batch 3：真实 PackageManager / UpdateStateMachine

- 抽出 `PackageManager`：download、verify、unpack、stage、backup、swap、health_check、rollback、restart。
- 所有依赖通过 trait 注入，生产实现和 deterministic test fixture 分离。
- manifest 除版本外还必须包含 package hash、signature algorithm、public key id、minimum host version、ABI、rollback policy。
- update operation 持久化并可在重启后 resume/recover。
- 安装与更新共享 canonical path、zip-slip、symlink、大小、文件数、权限策略。
- update commit 前保持旧版本可启动；health check 失败自动回滚。

### Batch 4：TS 生命周期和 Host/Remote

- Lazy loader generation/abort。
- Shell manager generation state machine 和真实四类 provider。
- Dual-world destroyed guard；QuickJS-WASM provider 接入前保持显式 unavailable。
- Local Host client 与 Remote Host client 统一 Universal Wire envelope，但 transport security facts 不可互相冒充。
- Remote production adapter 提供 TLS、audience、origin、nonce、sequence、rate-limit、resume token 的真实 socket 路径。

### Batch 5：Adapter 拆分和 API 收敛

建议拆分为：

```text
tauron-adapter/
  assembly.rs       # Substrate/Runtime 唯一装配与冲突策略
  commands/base.rs  # 61 条底座命令
  commands/runtime.rs
  commands/install.rs
  commands/market.rs
  runtime_driver.rs
  package_manager.rs
  recovery_bridge.rs
  settings_bridge.rs
  tauri.rs
```

每次拆分保持 wire contract 不变，先移动代码和测试，再改变语义；每个 domain 只能有一个 mirror site，避免 TS/Rust 双份状态转换。

SDK 收敛规则：

- `@tauron/host` 作为唯一前端 host contract。
- `@tauron/core` 只保留纯领域类型、runtime flags 和错误/版本常量。
- `plugin-sdk` / `app-plugin-sdk` 通过明确的 caller capability 层区分，不重复定义 command envelope。
- 生成 TypeScript 类型、Rust serde 类型和文档台账，移除手写重复接口。

### Batch 6：真实发布闭环

- 恢复并稳定 Windows MSI，或在发布说明中明确 NSIS-only policy 和验收范围。
- 在隔离 registry 中真实执行 npm/crates 发布顺序和 clean consumer install。
- 以同 SHA source-ci-proof 作为 tag 发布前必要条件。
- performance/size report 必须来自目标 runner artifact，不接受本地空报告。
- 发布证据中区分“源码 CI 绿”“artifact 构建成功”“registry consumer 安装成功”“运行时 E2E 通过”。

## 9. 推荐新增 CI 门禁

### Core semantic gates

- `success_requires_effect`：所有 success-shaped result 必须有 effect token、commit marker 或真实 runtime observation。
- `simulated_never_commits`：simulated 结果不得进入 install/update/restart。
- `runtime_assembly_singleton`：同一 substrate 只能有一个 plugin runtime。
- `wasm_loaded_generation`：execute 必须绑定 active module generation。
- `upgrade_file_hash`：升级前后以文件 hash、manifest 和 state journal 对账。

### Concurrency gates

- lazy loader reset/clear 与旧任务乱序完成。
- shell start/stop/destroy 交错。
- WASM execute/reclaim/clear/replace 并发。
- update cancel/timeout/rollback/restart 并发。
- install/uninstall/update 与 recovery boot 交错。

### Leak gates

- 每次 test 结束 tracked process、frame sink、timer、watcher、pending call、temporary directory 均为 0。
- kill failure 必须有 retry/terminal evidence，不能只留日志。
- abandoned generation 必须有 bounded metric，不允许无限增长。
- `clear()` 后所有异步任务必须可观测地完成、取消或被丢弃。

### Release gates

- `cargo deny`、format、Clippy、workspace tests、Tauri/plugin-install matrix。
- Linux/Windows/macOS target matrix。
- Local UDS/named pipe、Remote TLS socket、sidecar fixture E2E。
- npm/crates clean consumer。
- NSIS/MSI/Linux/macOS artifact install smoke test。
- performance/size report 与预算对账。

## 10. 终版发布判定

在以下条件全部满足前，不建议把 Tauron 宣布为“核心链路全部打通”或“生产级多形态 Runtime 已完成”：

- [ ] 重复 Runtime 装配变成 hard error，恢复写回口不存在 split-brain。
- [ ] WASM 未 load、未知 host function、无真实 engine 时全部 fail closed；真实 provider 有 load→execute→result 证据。
- [ ] WASM 实例复用、统计、generation、clear、crash recovery 与真实记录一致。
- [ ] Process 有真实 sidecar fixture 和四平台关键 E2E；Hard sandbox readiness 有平台证据。
- [ ] UpgradeRunner 完成真实下载、验签、备份、解压净化、atomic swap、健康检查、回滚、重启。
- [ ] market/update 的真实路径和 simulated 路径在类型和 UI 上不可混淆。
- [ ] lazy loader、Shell、Dual-world 的代际/销毁语义有 deterministic race tests。
- [ ] Local/Remote Host 的 reference、contract、production transport 三层边界明确且有对应测试。
- [x] Adapter 拆分或至少冻结 domain ownership，避免继续向单文件追加核心逻辑。（round 69 T-7 已把命令族二十二片逐字节纯 move 拆出 `lib.rs`、`pub fn cmd_*` 清零、六 feature profile 全绿；domain ownership 台账冻结并逐片 `--record` 重分预算。同族私助手 / 跨域助手 / 更广基础设施 `struct`·`const` 刻意暂留 `lib.rs`——搬它们需放宽可见性、属语义改动，留待与 T-9 灭并行态合并处理。）
- [ ] npm/crates 真实 registry clean consumer、四平台 artifact、MSI policy、performance/size、cargo-deny 全部有同 SHA 证据。

## 11. 建议执行顺序

```text
Batch 0 语义硬化与装配 hard error
   ↓
Batch 1 WASM loaded-generation gate + 实例池修正
   ↓
Batch 2 RuntimeDriver + sidecar fixture + Hard sandbox
   ↓
Batch 3 PackageManager / UpgradeStateMachine
   ↓
Batch 4 TS lifecycle + Local/Remote production transport
   ↓
Batch 5 Adapter 拆分 + SDK canonical API
   ↓
Batch 6 registry/artifact/release E2E
```

最终建议：先做 Batch 0 和 Batch 1，恢复“所有成功都对应真实效果”的基本语义；再做 Process 和升级链；最后才扩展 Remote、Shell 和发布形态。当前 main 已具备良好的契约审计基础，但仍应把“诚实模拟”与“真实生产能力”继续严格分层，避免在前端、Host、Adapter、Runtime 任一层把模拟结果重新包装成成功。

