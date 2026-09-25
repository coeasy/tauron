# Changelog

本文件记录 tauron 的显著变更。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> **关于「已知债务」**：本仓库刻意区分「真实实现」与「接口占位」。下面每一节
> 的「已知债务」都是**实测**结果（附复现命令），不是推测。请勿在发布公告里
> 把债务当已完成项宣传。

---

## [0.1.0] - 未发布

首个版本。框架层（`@tauron/types|core|plugin-sdk|adapter-*|cli|market|shell-matrix|dual-world|contract-tests` + `tauron-shell`）
与应用层（`@tauron/host|framework|ui|ui-primitives|app-*` + `tauron-host`/`tauron-adapter`）双栈成型。

### Added

**框架层**

- 单命令信封协议：`plugin_invoke` / `plugin_cancel` / `plugin_emit`
- 双世界隔离架构（`@tauron/dual-world`）；QuickJS-WASM 引擎接入仍为路线图项
- 4+1 插件形态：Rust / JS（真实实现）/ WASM / Process（配置 + 模拟原型）+ B+ 混合
- 双层 ACL：外层 Tauri 静态 ACL + 内层框架动态 ACL
- 每插件隔离的事件总线队列，`Event` 溢出丢最旧 / `Request` 硬失败 / `State` 只留最新；
  `MAX_QUEUE = 1000`，连续溢出 3 次熔断
- 配置 4 层合并（session > plugin > user > default）
- Shell 矩阵 4 形态（local / local-server / remote-url / sub-webview）
- 插件市场：HMAC-SHA256 验签、注册表搜索 / 发布
- CLI 工具链：`create` / `plugin new|dev|test|pack|sign|publish` / `doctor`
- React / Vue / Svelte 适配层

**应用层**

- `host_*` 命令族共 54 条（底座 38 + 插件运行时 16）
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

- `@tauron/contract-tests` 的 wire-gate：直接正则解析 Rust 源码文本做断言，**110 条**
  （`vitest run src/wire-gate.test.ts` 实跑计数）
- 全部跨 IPC 边界的 Rust 结构体强制 `#[serde(rename_all = "camelCase", deny_unknown_fields)]`
- 双套错误码且零交集：框架层 `SC-xxxx`（14 个）/ 应用层 `E_*`（19 个）

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

### Changed

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

以下问题**已知且未解决**，发布时不应被描述为已完成。

**1. Rust 格式化与 lint 从未跑过 → CI 中为 advisory**

```
cargo fmt --all -- --check     # 691 hunk / 59 文件（61 个 .rs 中）
cargo clippy --workspace --all-targets -- -D warnings   # 从未运行
```

代码库是按人工风格写的（对齐注释块、长解释性注释），从未做过 rustfmt 规范化。
`rustfmt.toml` 取的是实测差异最小的一档（`max_width = 100` + `use_small_heuristics = "Max"`）。
**转正条件**：一次性 `cargo fmt --all` + clippy 清理，并验证编译与测试后，
删掉 `ci.yml` 里对应 job 的 `continue-on-error`。

**2. Prettier 从未跑过**

`pnpm format:check` 当前会失败。CI 里没有 format 门禁，避免永久红。

**3. ESLint 有 81 条 warning（0 error）**

`pnpm lint` 退出码为 0，因此已作为**硬门禁**接入 CI。81 条 warning 绝大多数是
测试文件里的未用导入。**不要**加 `--max-warnings 0`，那会让仓库直接变红。

**4. Rust 测试未在有网络的环境执行过**

本仓库的 `cargo test --workspace` 结果以 CI 为准。README 徽章上的 Rust 数字
是源码 `#[test]` 声明数（静态计数），**不是**执行结果 —— feature 门控
（`tauron-adapter` / `tauron-shell` 的 `tauri` feature）另计，两者不同口径。

**5. `crates.io` 发布未就绪**

内部互引用写的是 `{ path = "crates/xxx" }`，cargo 要求 path 依赖同时给出
`version` 才能打包。在补齐 `version` 之前不要执行 `cargo publish`。

**6. npm 包全部 `private: true`**

20 个 `packages/*/package.json` 均为 `private`，无法 `npm publish`。

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

**8. 命令面缺口**

host 命令面缺 menu / tray / fs / http 四域。

**9. 孤儿 crate**

未被任何其他 crate 依赖：`tauron-acl` / `tauron-brand` / `tauron-theme` /
`tauron-market` / `tauron-distribute` / `tauron-wasm` / `tauron-shell`（`tauron-shell`
是框架层门面，属有意为之；其余为待激活）。

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

**11. sidecar ABI 校验的「实际值」无可信来源**

`host_runtime_spawn` 现在会比对宿主 ABI 契约与调用方声明的 `profile.abi`（不符 →
`E_ABI_MISMATCH`），但 `profile.abi` 是**调用方自报**的——这道校验挡的是「配置错配 /
前端用了旧模板」，**不是**「恶意调用方伪造 ABI」。真正的可信校验需要 sidecar 在
RPC 握手时自报指纹（**尚未实现**：`tauron-proc` 的 RPC 帧循环未接线）。同一性质：
`validate_spawn_config` 的签名 / 哈希检查也只做**格式**校验（非空 / 长度 64），
不做真实验签——`ProcError::SignatureInvalid` / `HashMismatch` 目前同样无生产产生点
（只有映射表与测试引用它们）。这两条**不构成安全边界**，发布公告里不要写成
「已实现进程插件验签 / ABI 校验」。

---

## 版本流程

1. 三处版本号一起升：
   - `Cargo.toml` 的 `[workspace.package] version`（15 个 crate 全部继承它）
   - 根 `package.json` 的 `version`
   - 20 个 `packages/*/package.json` 的 `version`
2. 在本文件顶部新增一节，把「未发布」改为实际日期
3. `git tag vX.Y.Z && git push origin vX.Y.Z`
4. `release.yml` 会先校验三处版本号与 tag 一致，再矩阵构建，最后落成**草稿** Release
   （人工过一眼再点发布）

[0.1.0]: https://github.com/coeasy/tauron/releases/tag/v0.1.0
