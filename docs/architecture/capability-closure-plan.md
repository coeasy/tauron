# tauron 0.4 能力收口方案：从「有接口无入口」到「链路真通」

> **状态**：方案已定，分轮实施中（轮 19 起）。不得据此宣称可发布。
> **目标版本**：tauron 0.4
> **前置**：底座重构 R1–R8（轮 7–12 已完成收口）；0.3 方案 S/M/X 部分落地
> （轮 13–18，见 [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md)）。
> **关联**：[overview.md](./overview.md)、[app-layer-wire.md](./app-layer-wire.md)、
> [canonical-owners.md](./canonical-owners.md)、[incremental-adoption.md](../integration/incremental-adoption.md)

**本文与 0.3 方案的关系**：0.3 解决的是「**析取之后的兑现问题**」（底座可装配、插件可隔离），
本文解决的是「**兑现之后的通路问题**」——2026-09-26 全仓重测发现，0.3 的多项已判定为
「完成/部分完成」，但**主体链路上仍有 4 跳是断的**，其中最核心的一跳（调用投递）从未存在过。
本文不再重复 0.3 的条目，只处理实测出来的新断链与未收口项。

---

## 0. 这次梳理的方法与口径

1. **一切以代码为准，文档与代码冲突处记文档漂移**。所有结论附 `路径:行号`。
2. **判据是「入口可达性」，不是「函数存在」**。本仓反复出现的缺陷类型是
   「有类型、有接口、有测试、没有入口」——编译过、测试过、文档写着，线上永远走不到。
   轮 9–12 抓出过 8 处，本轮又抓出 4 处（见 §6 的 P0-1 / P0-2 / P0-3 / P1-3）。
3. **两条产品线分别评估**，不混在一张表里打分。
4. 数字为 **2026-09-26 本机实跑**结果，不是源码声明数推断。

### 0.1 实测基线

| 指标 | 实测值 | 命令 |
|---|---|:--|
| Rust 测试（默认特性） | **1260 passed / 0 failed**（15 suite） | `cargo test --workspace --locked --lib --tests` |
| Rust 测试（`+plugin-install`） | **1267 passed / 0 failed** | 同上加 `--features tauron-adapter/plugin-install` |
| TS 测试 | **1645 tests（20 包），1 红** | `pnpm -r test` |
| 唯一红灯 | `wire-gate.test.ts:1280` 能力协商同源门禁 | 见 §6-P0-4 |

> TS 全量 `pnpm verify` 在本机被沙箱的批量删除保护挡住（`packages/tauron-host` 的
> `prebuild: rimraf dist` 触发 `SAFE_DELETE_BULK_CONFIRM_REQUIRED`），故改为逐包
> `tsc -p tsconfig.build.json` + `vitest run`。**这是环境限制，不是项目缺陷**，
> CI 上不受影响。

---

## 1. 项目是什么：功能全景

**一句话**：Tauri 2 之上的**插件化桌面客户端基础设施**——不是一个应用，是一套给接入方
装配「带插件生态的桌面客户端」的地基。15 个 Rust crate + 20 个 npm 包。

### 1.1 它对外承诺两件事

| 承诺 | 落地形态 | 实测结论 |
|---|---|:--|
| **任意宿主底座** | 一套与 UI 无关的宿主能力面：窗口 / 剪贴板 / 深链 / 对话框 / i18n / 通知 / 设置 / 崩溃恢复 / 事件总线 / 流式帧 | ⚠️ 实际是「任意 **Tauri** 宿主底座」；底座能力面**缺 5 个域**且**调用投递不存在** |
| **多插件框架** | 插件可安装、可启停、可调用、可订阅事件、可贡献 UI、可回收 | ⚠️ 实际是「**预置插件**宿主」：装不进来（默认构建）、调不出去（无投递）、UI 不驱动 |

### 1.2 功能域清单（按「真通 / 降级 / 未通」三档）

| 域 | 关键命令 | 状态 | 证据 |
|---|---|:--:|---|
| 窗口管理 | `host_window_*`（8 条） | ✅ 真通 | `lib.rs:4529` 真铸窗，label 恒 `plugin-<id>` |
| 生命周期 | `host_lifecycle_report` / `host_registry_admin` | ✅ 真通 | 10 状态 × 18 事件状态机，宿主单一写入 |
| 事件总线 | `host_events_publish/subscribe/unsubscribe/drain` | ✅ 真通 | 三通道（event/state/request）+ 每订阅者队列 |
| 流式帧 | `host_stream_open/write/close` | ✅ 真通 | seq 由宿主铸，终帧后句柄失效 |
| 崩溃恢复 | `host_recover_boot/report/trial_enable` | ✅ 真通 | 持久化 + 安全模式 + 试验预算 |
| i18n | `host_i18n_*`（6 条） | ✅ 真通 | 落到 `tauron-i18n` 引擎，回退链 + 缺键计数 |
| 通知 | `host_notify` / `host_notifications_list/read` | ✅ 真通（系统推降级） | 缓冲 + dispatchLog，系统气泡不可达 |
| 设置 | `host_settings_*`（含 adopt/migrate） | ✅ 真通 | `SettingsStore` 四层合并 + 版本迁移 |
| 资源配额 | `host_resource_stats` | ✅ 真通 | 4 类 per-plugin 配额已接线 |
| **插件调用** | `host_plugin_call` / `host_call_end` / `host_cancel` | ❌ **只登记，不投递** | `lib.rs:2386-2397`；见 §5-L8 |
| **插件安装** | `host_registry_install(_preview)` | ❌ 默认不可达 | `#[cfg(feature="plugin-install")]`，`Cargo.toml:12 default=[]` |
| **进程插件通信** | `host_runtime_spawn/health` | ❌ 能起不能聊 | `tauron-proc/spawner.rs:126-127` `Stdio::null()` |
| **扩展点 UI** | `host_contributes_register/list` | ❌ 不驱动 UI | 无 reconcile、无 drift 码、`oc-command-select` 孤儿 |
| 对话框 | `host_dialog_*`（4 条） | ⚠️ 诚实降级 | `UnsupportedBody`，非「假装成功」 |
| 剪贴板 | `host_clipboard_write/read` | ⚠️ 进程内回退 | `fallback: in-process-buffer` |
| 品牌 / 商城 / 更新 | `host_brand_info` / `host_market_*` | ⚠️ 诚实桩 | `UnsupportedBody` / `simulated: true` |
| menu / tray / fs / http / updater | — | ❌ **全仓 0 命中** | `grep host_menu_\|host_tray_\|host_fs_\|host_http_\|host_updater` 空 |
| 跨插件调用 | — | ❌ 不存在 | `host_call_plugin` 不存在 |

---

## 2. 架构：真实结构

### 2.1 三层，不是两层

文档里一直写「框架层 + 应用层」两层，但实测依赖关系是**三层**，且**中间那层是死的**：

```
┌─ ① 框架层（对外协议门面）────────────────────────────┐
│  @tauron/types · @tauron/core · @tauron/plugin-sdk     │
│  @tauron/adapter-react|vue|svelte · dual-world         │
│  shell-matrix · @tauron/cli · @tauron/market           │
│  Rust: tauron-shell（plugin_invoke/cancel/emit 三命令） │
│  状态：🔴 链路死——PluginDispatcher 零生产实现           │
└────────────────────────────────────────────────────────┘
                      ↓ 仅 TS 侧被反向依赖
┌─ ② 应用层（真正活着的那条线）────────────────────────┐
│  @tauron/host · framework · ui · ui-primitives        │
│  app-cli · app-plugin-sdk · app-contract-kit           │
│  Rust: tauron-host（引擎）+ tauron-adapter（host_* 58）│
└────────────────────────────────────────────────────────┘
                      ↓
┌─ ③ 平台层 ────────────────────────────────────────────┐
│  Tauri v2（唯一真实传输实现）                          │
└────────────────────────────────────────────────────────┘
```

**隐藏耦合（本次新发现，危险）**：活的应用层在 TS 侧**直接 import 死的框架层**——
`packages/tauron-host/src/bootstrap.ts:32` 引用 `@tauron/core` 的
`PluginRegistry` / `ConfigManager` / `EventBus`。也就是说「删掉框架层」不是一个
低风险动作：活线拖着死线的 TS 部分。这条必须在 A5 里显式处理。

### 2.2 Rust crate 依赖实测（2026-09-26）

```
tauron-adapter ──→ tauron-host, tauron-i18n, tauron-notify,
                   tauron-recovery, tauron-settings, tauron-proc   [默认]
                 ─→ tauron-acl, tauron-market                      [仅 plugin-install]
tauron-settings ─→ tauron-schema
tauron-acl ──────→ tauron-host
零消费者（孤儿）：tauron-brand / tauron-theme / tauron-wasm /
                  tauron-distribute / tauron-shell
```

| crate | 状态 | 判据 |
|---|:--:|---|
| `tauron-acl` | 迁移中 | 仅 `plugin-install` 下被调（`lib.rs:1810`） |
| `tauron-market` | 迁移中 | 仅 `plugin-install` 下被调（`lib.rs:2032/2040`，只验签） |
| `tauron-brand` | 孤儿 | `tauron_brand::` 全仓 0 命中 |
| `tauron-theme` | 孤儿 | 0 命中 |
| `tauron-wasm` | 孤儿 | 0 命中；1629 行、50 测试，但**无 wasm 引擎依赖** |
| `tauron-distribute` | 孤儿（CI 侧，可接受） | 0 命中 |
| `tauron-shell` | 孤儿 + legacy 冻结 | 无 crate 依赖；示例 `main.rs:272-277` 调用点是注释 |

### 2.3 命令面

| 集合 | 条数 | 定义 |
|---|---:|---|
| 底座 `tauron_substrate_handler!` | **39** | `tauri.rs:2253-2305` |
| 插件运行时 `tauron_plugin_handler!` | **56**（39 + 17） | `tauri.rs:2313-2393` |
| 安装（feature-gated） | **+2** | `tauri.rs:2389-2392`，`#[cfg(feature="plugin-install")]` |

`crates/tauron-adapter/Cargo.toml:12` → `default = []`（连 `tauri` feature 都不默认）。
唯一真实装配入口：`examples/minimal-app/src-tauri/src/main.rs:140`。

---

## 3. 实现细节：真正值得继承的机制

这些是 R1–R8 + 0.3 沉淀下来的**硬设计**，本方案所有改动都必须保持：

1. **跨语言线格式强制 camelCase**：跨 IPC 的 Rust 结构体一律
   `#[serde(rename_all = "camelCase", deny_unknown_fields)]`，由 wire-gate 正则扫源码锁定。
2. **两套错误码、零交集、只追加**：框架层 `SC-xxxx`，应用层 `E_*`；
   `ErrorCode` 新码只能追加到枚举末尾（TS `HOST_ERROR_CODES` 按声明顺序比对）。
3. **身份从 webview label 解析，不信任入参**：self 档命令的 pluginId 只从
   `plugin-<id>` 解析（`lib.rs:6202`），调用方传入一律忽略。
4. **底座类型上触达不到注册表**：`SubstrateState` 无 registry 字段，编译期隔离。
5. **`cmd_*` 零直接 Tauri 调用**：平台调用一律经 `trait *Sink`。
6. **事件总线锁序**：`stats → topics → subs → topic_subscribers → approvals → queues`。
7. **诚实降级形状**：`UnsupportedBody{supported,reason,fallback}` /
   `DegradedValue` / `simulated` 字段——**成功形状不得用于表达「没做」**。
   这一条本项目贯彻得很好（dialog / clipboard / brand / market / deep_link 全部合规），
   是它的核心竞争力之一，不得回退。
8. **pending 表只在满时 GC**：稳态不驱逐，保证过期条目返回 `E_CALL_TIMEOUT`（可重试）
   而非 `E_CALL_NOT_FOUND`（不可重试）——两个语义不能塌缩。
9. **per-plugin 配额已接线 4 类**：pending 100/插件（`registry.rs:33`）、
   stream 32/插件 + 256 全局（`stream.rs:113/122`）、订阅 256/插件 + 4096 全局
   （`eventbus.rs:49/52`）、通知 512 全局 + 64/插件（`tauron-notify/src/lib.rs:129/161`）。

---

## 4. 记分卡（两条产品线分开打）

口径：**0 = 不存在 / 30 = 有接口无实现或有实现无入口 / 60 = 主链路可用但有硬缺口 /
85 = 生产可用 / 95 = 生产可用且有对账与可观测**。

### 4.1 应用层（活的那条线）

| 维度 | 0.3 自评 | **本次实测** | 判据 |
|---|:--:|:--:|---|
| 生命周期闭合 | 75 | **85** | 启停/卸载/崩溃恢复全通，卸载回收 6 类旁路状态 |
| 事件与流式 | 60 | **85** | 三通道 + 帧由宿主铸 seq + 终帧后失效 |
| 资源隔离 | 25 | **75** | 4 类 per-plugin 配额真接线；通知环仍全局单实例 |
| 能力协商 | — | **70** | `host_capabilities` 存在且推导入口，但 `families`/`unsupported` 硬编码（`lib.rs:396-415`） |
| 诚实降级 | 50 | **85** | 全部桩命令返回有类型 `UnsupportedBody`，无裸成功 |
| **插件调用** | 60 | **30** ⬇️ | **只登记不投递**（§5-L8）——0.3 把「有命令」误判为「有链路」 |
| **插件安装** | 20 → 部分完成 | **30** | 已实现但 feature-gated，`default=[]`；TS `available()` 误报 |
| 扩展点 contributes | 40 | **40** | 无任何进展：无 reconcile、不驱动 UI |
| 市场 / 更新 | 10 | **25** | 诚实桩（`simulated`），UI 不冒充可用 |
| 部署可配 | 40 | **40** | `crates/tauron-adapter/permissions/` 仍不存在 |
| 底座命令面 | 55 | **55** | 5 个域仍 0 命中 |

### 4.2 框架层（死的那条线）

| 维度 | 本次实测 | 判据 |
|---|:--:|---|
| 主链路贯通 | **20** | 五跳中前四跳真实，第五跳 `PluginDispatcher` **零生产实现** |
| 命令注册 | **0** | `plugin_invoke/cancel/emit` **从未被任何宿主注册**（示例里注释掉） |
| 引擎实现 | 60 | shell 侧 1142 行（eventbus 328 + registry 297 + dispatch 517），全绿测试，但无人用 |
| UI adapter | **30** | 3 个 adapter 真实现，但**零消费者** + 测试是 `expect(typeof x).toBe('function')` 同义反复 |
| dual-world | **20** | `sandbox.ts:121-142` 显式 fail-closed「本包不执行代码」，无 QuickJS-WASM |
| shell-matrix | **20** | 4 形态全是 `setTimeout(10)`，自带 `simulated: true` |
| CLI | **55** | `doctor`/`create`/`plugin new` 真实；`dev`/`test`/`pack`/`publish` 返回 `success:false`；`plugin sign` 是 SHA-256 摘要（诚实标注，非冒充） |

---

## 5. 主体链路贯通判定（本文的核心结论）

**「主体链路」定义**：一个用户从打开客户端到用一个插件完成一次交互所必须经过的跳。

```
 L1  宿主装配    tauron_adapter::tauri::init()          ✅  main.rs:140
 L2  主窗启动    ShellClient + host_capabilities        ✅  但有 1 红灯（P0-4）
 L3  插件安装    host_registry_install                  ❌  feature-gated，默认不可达
 L4  插件启用    host_registry_admin(enable)            ✅
 L5  插件开窗    host_window_create                     ✅  label 恒 plugin-<id>
 L6  插件自举    app-plugin-sdk PluginContext           ❌  示例用 legacy iframe SDK
 L7  生命周期    host_lifecycle_report(ATTACH)          ✅
 L8  调用投递    host_plugin_call                       ❌  ★只登记，不投递
 L9  流式帧      host_stream_open/write/close           ✅
 L10 事件        publish/subscribe/drain                ✅  app-sdk 有泵；legacy 无泵
 L11 卸载回收    host_registry_admin(uninstall/purge)   ✅
 L12 崩溃恢复    host_recover_*                         ✅
```

**结论：12 跳中 8 跳真通、4 跳断。** 断的 4 跳里，**L8 是最严重的**——
它意味着「多插件框架」这个名字里的「调用」这件事，**从来没有实现过**。

### 5.1 ★ L8 的证据链（这是本次最重要的发现）

```rust
// crates/tauron-adapter/src/lib.rs:2385-2397
/// `host_plugin_call`：插件调用自己的 C/D 后端（self 档）。
pub fn cmd_plugin_call(
    state: &PluginRuntimeState, webview_label: &str,
    claimed_id: Option<&str>, cmd: &str, args: serde_json::Value,
) -> HostResult<PendingCall> {
    guard("plugin_call", || {
        let id = tauron_host::authz::resolve_self_identity(webview_label, claimed_id)?;
        state.registry.call_begin(&id, cmd, args)      // ← 只有这一句
    })?
}
```

- `Registry::call_begin`（`registry.rs:513-558`）做的事是：**校验状态 → 判配额 → 满时 GC →
  铸 callId → 插入 pending 表 → 返回**。全函数没有任何一行把请求送出去。
- 命令面里**没有** `host_call_status` / `host_call_poll` 之类的取件命令，所以
  pending 条目也没有出口（除了 `call_end` 关掉它）。
- `packages/tauron-shell-events/src/index.ts:54` 自陈：命令面板的执行
  「属**接入方域**，跨窗口命令调用走 `host_plugin_call` 的 C/D 后端路径」——
  即「投递」被显式划给了接入方自己实现。
- `tauri.rs:251` 注释：`channel` 为前端流式载荷，应用层核心**按 pending 簿记**。

**判定**：`host_plugin_call` 是一个**簿记原语**，不是一条调用通路。
「宿主 → 插件」「插件 → 自己的 sidecar」两个方向的投递都不存在。

### 5.2 其余三跳

| 跳 | 断在哪 | 证据 |
|---|---|---|
| L3 安装 | `#[cfg(feature="plugin-install")]` + `default=[]`；TS 侧 `tauri-backend.ts:76-77`、`capabilities.ts:135-143` **无条件**列出 → `available()` 返回 true，invoke 才失败 | `Cargo.toml:12`、`tauri.rs:2389` |
| L6 自举 | 示例 `examples/minimal-app/src/plugin/first.ts:9` 用的是 legacy `@tauron/plugin-sdk` 的 `registerPlugin`（iframe 握手）；主推的 `@tauron/app-plugin-sdk` 在仓内**零生产调用点**（只出现在 `app-cli/plugin.ts:317` 的脚手架模板字符串里） | 见 §6-P1-3 |
| 进程插件通信 | `tauron-proc/src/spawner.rs:126-127` 把 stdin/stdout 都设成 `Stdio::null()`，文件头 `:82-83` 明文「JSON-RPC 帧回路未接线」；`spawn.rs:322` 是回显模拟 | Process 型「能起不能聊」 |

---

## 6. 不合理清单

### P0（阻塞「核心功能已实现」这个判断）

| # | 问题 | 证据 | 为什么严重 |
|---|---|---|---|
| **P0-1** | **调用链最后一跳不存在**：`host_plugin_call` 只登记 pending，无任何投递通路 | `lib.rs:2386-2397`、`registry.rs:513-558`、`shell-events/index.ts:54` | 「多插件框架」的核心动词（调用）从未实现；命令面板、跨插件协作、插件→sidecar 全部悬空 |
| **P0-2** | **框架层整条产品线不可用却按可用文档发布**：`PluginDispatcher` 零生产实现 → 生产环境 `plugin_invoke` 恒返回 SC-9001 | `tauron-shell/src/dispatch.rs:41-49/187-195`（trait + 恒错返回）；`impl PluginDispatcher` 只出现在 `dispatch.rs:278` 的测试 `EchoDispatcher` | 第三方按 README 集成必然拿到 SC-9001。这是对外承诺与实现的直接冲突 |
| **P0-3** | **安装链路与 TS 能力表不对齐**：Rust 侧 feature-gated 默认关闭，TS `available()` 无条件报 true | `Cargo.toml:12`、`tauri.rs:2389`、`tauri-backend.ts:76-77`、`capabilities.ts:135-143` | 前端按能力表判断「能不能装」，得到错误答案；失败发生在 invoke 而非探测 |
| **P0-4** | **当前默认构建 TS 门禁 1 红** | `wire-gate.test.ts:1280` → `AssertionError: missing PLUGIN_INSTALL_COMMANDS: expected -1 to be greater than -1` | 仓库现在是红的。根因是**门禁解析 bug** 而非 feature：`parse()` 要求 `pub const PLUGIN_INSTALL_COMMANDS: &[&str] = &[` 同行，而 `lib.rs:489-490` 的源码在 `=` 后换行 |
| **P0-5** | **进程插件能起不能聊**：stdin/stdout 全 `Stdio::null()` | `tauron-proc/src/spawner.rs:82-83/126-127` | Process 是四形态里唯一「有执行器」的，但通信不通等于不可用 |

### P1（架构层面不合理）

| # | 问题 | 证据 |
|---|---|---|
| **P1-1** | **活线依赖死线**：`@tauron/host` 的 `bootstrap.ts:32` 直接 import `@tauron/core` 的 `PluginRegistry`/`ConfigManager`/`EventBus` | 框架层不可删除，A5 的迁移成本被这行代码放大 |
| **P1-2** | **插件形态：只有 1.5 个真能用**。Js 无执行器（靠开窗+自举）；Rust/Wasm 只有 `E_PLUGIN_TYPE_NO_RUNTIME`；`tauron-wasm` 1629 行 + 50 测试但**无 wasm 引擎依赖**（`Cargo.toml` 里没有 wasmtime/extism） | 对外文档写「4+1 形态」，代码里 `PluginType` 四变体中三个无执行器 |
| **P1-3** | **示例 app 走错了 SDK**：主推 `app-plugin-sdk` 零生产调用点，示例却用 legacy iframe `registerPlugin` | `examples/minimal-app/src/plugin/first.ts:9`；主推 SDK 唯一出现处是 `app-cli/plugin.ts:317` 的模板字符串。**没有可跑的端到端证据** |
| **P1-4** | **门禁本身有漏洞**：`oc-command-select` / `oc-tray-item` / `oc-shortcut-change` 是孤儿事件（组件派发、无人监听），而门禁 `wire-gate:1523` 允许「接线**或**注释声明接入方域」→ 注释声明即放行 | 门禁绿 ≠ 运行时有人接。这是「门禁形同虚设」的典型 |
| **P1-5** | **contributes 不闭环**：无 reconcile、无 `E_CONTRIBUTES_DRIFT`（全仓 0 命中）、不驱动任何 UI | `lib.rs:86-133` 只是 Vec 增删 |
| **P1-6** | **4 个孤儿 crate 仍零调用点**：brand / theme / wasm / distribute | `tauron_brand::` 等全仓 0 命中 |
| **P1-7** | **MemoryTransport 不完整**：无流式内核（`host_stream_*` 落 `command not found`）、无取件泵、`channel()` 的 `onmessage` 为 null | `memory-transport.ts:59`；0.3 的 S1 验收条件（跑通档 1 全部能力域）**实际未达成** |
| **P1-8** | **文档自身漂移**：0.3 方案 §2 残差表（2026-09-24 口径）与 §0 快照（2026-09-26）矛盾——§2 说 M-1「无 install 入口」、M-2「无任何 per-plugin 配额」，实际两者都已实现 | 文档维护跟不上代码，接入方按 §2 选型会选错 |
| **P1-9** | **底座能力面缺 5 域**：menu / tray / fs / http / updater 全仓 0 命中 | 插件读写文件、发网络请求的能力完全没有 |
| **P1-10** | **两份注册表无桥接**：`@tauron/core` 的 `PluginRegistry`（`bootstrap.ts:152` 硬编码 32）与 Rust `Registry`（`registry.rs:38` `max_plugins: 8`）各自硬编码；前端默认 `Infinity` 与 32 自相矛盾 | 32 ≥ 8 纯属巧合，改任一侧都可能让前端拒绝宿主允许的插件 |

### P2（体验与一致性）

| # | 问题 | 证据 |
|---|---|---|
| P2-1 | `createContractContext` **缺取件泵 = 订阅即死** | `plugin-sdk/contract-context.ts:210-214`，文件头 `:21-23` 自陈「取件泵不在此实现」 |
| P2-2 | 通知环仍是**全局单实例**，逐插件只是裁剪而非独立环 | `lib.rs:1260/1299` |
| P2-3 | 3 个 UI adapter 零消费者 + 同义反复测试 | `use-invoke.test.ts:5-7` 仅 `expect(typeof x).toBe('function')` |
| P2-4 | `host_capabilities` 的 `families` 与 `unsupported` 域名是**硬编码**，不是推导 | `lib.rs:396-415` |
| P2-5 | `dual-world` / `shell-matrix` 是模拟实现（`simulated: true`） | `sandbox.ts:121-142`、`manager.ts:71-93` |
| P2-6 | `plugin sign` 产出 `algorithm: 'sha256-digest'`（诚实标注但功能未实现） | `cli/plugin-lifecycle.ts:157-209` |
| P2-7 | 两套 IPC 后端 + 两套命令名表并存 | `core/src/tauri-backend.ts`（208 行，`plugin_*`）vs `host/src/tauri-backend.ts`（198 行，`host_*`） |

---

## 7. 重构方案：A1–A8

编号规则：**A = Ability closure（能力收口）**。与 0.3 的 S/M/X 不冲突（那条线已收口或并入本文）。
每条给：**问题 / 设计 / 不变量 / 门禁 / 验证 / 量级**。

---

### A1. 调用投递闭环（解 P0-1，本次最高优先级）

**问题**：`host_plugin_call` 只登记 pending，没有把请求送到任何地方。
两个方向都缺：**宿主 → 插件**（命令面板/宿主主动调用）与**插件 → 自己的 C/D 后端**（sidecar）。

**设计**：引入 `trait CallDelivery`，把「登记」与「投递」分成两件事，按插件形态选实现：

```rust
/// 调用投递：把已登记的 pending call 送到真正能执行它的一端。
pub trait CallDelivery: Send + Sync {
    /// 目标形态；宿主按插件类型选实现。
    fn target_kind(&self) -> DeliveryKind;   // Js | Process | Wasm | Native | Unwired
    /// 投递。返回 false = 无可用通路，宿主据此返回有类型的 Unsupported，不静默成功。
    fn deliver(&self, call: &PendingCall) -> HostResult<DeliveryReceipt>;
    /// 结果回执（与 pending 表对账，超时/取消共用既有 TTL 机制）。
    fn settle(&self, call_id: &str, outcome: CallOutcome) -> HostResult<PendingCall>;
}

pub struct DeliveryReceipt {
    pub delivered: bool,
    pub reason: Option<String>,   // 未投递时必须非空
}
```

- **Js 型**：走**已有的 request 通道**投递（复用 `host_events_publish` 的可靠语义，
  不新增命令）——宿主 publish 到保留 topic `plugin:<id>:__call`，插件侧取件泵
  （`app-plugin-sdk` 已有）取件、执行、回发结果。零新增命令、零新增传输。
  **轮 20 已落地**：入站写队列走 `EventBus::deliver_inbound`（宿主内部直达
  `(target, Request)` 队列，绕过 topic 声明/授权——`__call` 是宿主保留 topic，
  要求「先声明才能投」会把激活顺序变成时序依赖）；SDK 泵注册命令即开泵，
  识别 `:__call` 帧自动执行命令并经 `host_call_result` 回填。
- **Process 型**：`tauron-proc` 的 JSON-RPC 帧回路（A3），走 stdin/stdout。
  **轮 20 已提前落地（A3 帧回路部分）**：`CommandSpawner` 改 `Stdio::piped()` +
  每进程一个 stdout 读线程持续排空（化解「接管道没人读 → sidecar 写满缓冲被
  阻塞死」），`write_frame` / `register_frame_sink` 进 `ProcSpawner` trait
  （缺省实现诚实返回不支持）；`ProcessCallDelivery` 把调用写成 JSON-RPC 行帧
  （`callId` 内嵌关联，`seq` 作 `id`），回帧经 `ProcessFrameSinkImpl` 调
  `settle_call`。**诚实边界**：本仓没有可执行 sidecar，端到端无运行期证据，
  Rust 层闭环用 fake 启动面验证（测试不起真进程）。
- **Wasm / Native**：A4 打通后各接一路。
- **未装配任何 delivery** → 返回 `UnsupportedBody { reason }`，**不得**返回 `PendingCall`
  假装成功（沿用 §3-7 的诚实约定）。

**不变量**：
- 「登记」与「投递」必须可分：pending 表继续只做簿记与 TTL，投递失败**不改** pending 语义。
- 投递失败 = 明确的错误码（复用 `E_*`，不新增），**绝不**静默丢弃。
- `host_plugin_call` 的返回值必须能区分「已投递待应答」与「无通路」。

**门禁**：
- 「`CallDelivery` 的每个实现必须被 `PluginRuntimeState` 注册」（防只有 trait 无实现）
- 「未装配 delivery 时 `host_plugin_call` 必须返回 `Unsupported` 而非 `PendingCall`」（负向测试）
- 「Js 型投递必须使用既有 request 通道，不得新增命令」（防通路分叉）

**验证**：Js 型跑通「宿主 → 插件方法 → 返回值 → call_end」；
不装配 delivery → 前端读到 `reason` 而不是拿到一个永不结束的 pending。

**量级：大（1–2 轮）**。这是把「多插件框架」从名词变成动词的一跳。

---

### A2. 安装链路默认可达 + 自举归位（解 P0-3 / P1-3）

**问题**：安装已实现但默认关；TS 能力表无条件列出 → `available()` 误报；
示例 app 用 legacy SDK，主推 SDK 无端到端证据。

**设计**：
1. **feature 与能力表对齐**：`host_registry_install*` 是否可用，必须由**同一份真相**推导。
   在 Rust 侧导出 `PLUGIN_INSTALL_AVAILABLE: bool`（`cfg!` 推导），TS 侧
   `available()` / `capabilities.ts` 读 `host_capabilities` 的返回，不再硬编码。
2. **示例 app 换成主推 SDK**：`examples/minimal-app/src/plugin/first.ts` 改用
   `@tauron/app-plugin-sdk` 的 `createPlugin`，作为**可跑的端到端证据**；
   legacy iframe 版保留为 `examples/minimal-app/src/plugin/legacy-first.ts` 并标注 deprecated。
3. 安装失败/验签失败的路径保持「无残留」（`lib.rs:5187-5285` 已有负向测试，保持）。

**不变量**：能力表**只能**由运行时真相推导，不得硬编码（P2-4 同源）。

**门禁**：「TS 的 install 可用性标注 == Rust `PLUGIN_INSTALL_AVAILABLE`」
+「`app-plugin-sdk` 必须有非测试的消费方（示例 app 或 e2e）」。

**验证**：默认构建下 `available('host_registry_install') === false`；
开启 feature 后 `=== true`；示例 app 用主推 SDK 跑通一次完整调用。

**量级：中（1 轮）**。

> **轮 22 落地注记**：示例已切换（`first.ts` → app-plugin-sdk；`legacy-first.ts` 保留
> 并 deprecated；`plugin-window.html` 新构建入口承载插件面板窗口）。链路设计：
> 主窗 `windowCreate` 开插件 webview → 页面内 `createPlugin` + `createPluginContext`
> 注册命令即开执行泵 → 主窗 `callPlugin` 投递 → 泵自动执行并回填 → `callTakeResult` 取走。
> 门禁「app-plugin-sdk 必须有非测试的消费方」已落（变异验证红）。
> 真实运行时端到端需真 Tauri 环境与签名安装包（诚实边界，构建级证据齐全）。

---

### A3. 进程插件 JSON-RPC 帧回路（解 P0-5）

**问题**：`Stdio::null()`，能起进程不能通信；`spawn.rs:322` 是回显模拟。

**设计**：
1. `spawner.rs` 把 stdin/stdout 换成 `Stdio::piped()`，并起一个**读线程**持续 drain stdout
   （文件头 `:82-83` 担心的「没人读把子进程阻塞死」正是靠这条读线程解决，不是靠丢弃）。
2. 定义帧词表（换行分隔 JSON，`{ id, method, params }` / `{ id, result | error }`），
   与 `host_plugin_call` 的 pending 表对接：一帧一 pending。
3. 背压与超时：写帧带 `expires_at`，复用既有 TTL GC；子进程写满管道时读线程必须已排空。
4. 崩溃回收：`CrashTracker` 已有窗口计数，接入 stdout 关闭/EOF 作为死亡信号
   （补上 A3 后「轮询式崩溃检测」可升级为事件驱动，但**不要求**本轮做）。

**不变量**：
- 读线程**必须**存在且随进程生命周期；不接管道（回退 `Stdio::null()`）时
  返回 `Unsupported` 而非静默降级成「能起不能聊」。
- 帧词表与 TS 侧 `SIDECAR_ABI_CONTRACT` 同构，由 wire-gate 锁定。

**门禁**：「`Stdio::piped()` 必须配对一个 stdout 读线程」（正则 + 结构断言）
+「帧词表两侧字段逐字一致」。

**验证**：sidecar 真收一帧、真回一帧；拔掉读线程 → 门禁红。

**量级：中（1 轮）**。

> **轮 21 落地注记（发布审计深化）**：读线程存在性门禁已落（wire-gate「sidecar stdout 必须须有读线程在排水」，
> 变异验证红）。轮 21 曾把 `ProcRunner` 的心跳改为按插件分账，**发布审计随后把该修复深化为删除**：
> `ProcRunner` 模拟执行器（含全局心跳单例、RPC 回显模拟）全仓零生产调用方，连同专属配置类型与
> 九个零产生点的 `ProcError` 变体一并移除——真实链路（`CommandSpawner` + `RuntimeTable`）从未共享心跳
> 状态。**心跳监控在真实链路上尚未实现**，登记为本节的诚实边界（不是"已实现但在别处"）。
> 「sidecar 真收真回」同为诚实边界：需要真实 sidecar 二进制与集成环境，
> Rust 层以 fake-spawner 闭环测试为证（`process_call_delivers_frame_to_stdin_and_settles_via_sink`）。

---

### A4. 插件形态全打通（解 P1-2）

> **约束变更声明**：0.3 的 §7 明确「不实现进程内 WASM 运行时」。本次按接入方决策改为
> **四种形态全打通**，因此允许在 feature-gated 的可选组件档引入 wasm 引擎依赖。
> 核心逻辑层（`tauron-host` 的 engine、`tauron-adapter` 的 `cmd_*`）**仍然禁止**新增依赖。

| 形态 | 现状 | 目标 | 动作 |
|---|---|---|---|
| **Js** | 无执行器，靠开窗自举 | **契约化** | 承认「宿主开窗 + 插件自举」为正式模型，隔离来自 webview 边界 + 逐命令身份判定；A1 补上投递；示例换主推 SDK（A2） |
| **Process** | 能起不能聊 | **真通** | A3 |
| **Native（Rust）** | 仅出现在 manifest 校验 | **编译期静态注册** | 宿主构建期 `register_native(id, impl)`，清单校验 entry/abi，运行期直接调用，无 IPC/进程/webview；未注册 → 复用 `E_PLUGIN_TYPE_NO_RUNTIME` |
| **Wasm** | 无引擎 | **真通（feature-gated）** | `tauron-wasm` 补 wasm 引擎依赖（`wasmtime` 优先，Extism 次之），**默认关闭**；未启用时维持 `E_PLUGIN_TYPE_NO_RUNTIME` |

**不变量**：每个 `PluginType` 变体必须有 {执行器} 或 {诚实失败码 + 文档标注}，
**不允许**「清单合法但调即死」的第三态。

**门禁**：「遍历 `PluginType` 四变体，每个必须有执行器分支或显式 `NO_RUNTIME` 返回」
+「对外文档的形态表必须与枚举变体集合一致（删除 README 的「B+ 混合模式」这类代码里不存在的宣称）」
+「wasm 引擎依赖不得进入默认构建」（`cargo tree --no-default-features` 断言）。

**量级：大（2 轮：Native 一轮，Wasm 一轮）**。

---

### A5. 框架层复活：阶段 2 委托 + 阶段 3 删除（解 P0-2 / P1-1）

> 执行 `canonical-owners.md` 已规划但未做的阶段 2/3。本次实测给出一个**降低风险的新论据**：
> 正因为 `PluginDispatcher` 零生产实现，把它改成委托 `tauron-host` **不存在行为回归**
> ——只是把「恒返回 SC-9001」换成「真执行」。

**阶段 2（委托）**：
- `tauron-shell` 的 `PluginDispatcher` 改为委托 `tauron-host` 的引擎；
  `tauron-shell` 的 `HostState::handle_invoke` 只做**信封 ↔ 宿主调用**的翻译。
- 两套错误码继续不合并，穿越点走既有 `translate_at_boundary`。
- shell 的 71 个测试改写为「委托后的行为断言」（不再断言内部实现）。

**阶段 3（删除）**：
- 删除 shell 的 `EventBus` / `PluginRegistry` / `HostState` 三套 legacy 实现与旧测试。
- **先解 P1-1**：`@tauron/host` 的 `bootstrap.ts:32` 不再 import `@tauron/core` 的
  `PluginRegistry`/`ConfigManager`/`EventBus`——要么把这三个类迁到 `@tauron/host`
  （或一个新的零依赖包），要么改为只依赖类型。
- `@tauron/core` 的调用点合并到 `@tauron/host` 的 `HostRpc`。

**不变量**：框架层命令名与线格式**不得改变**（`plugin_invoke` 三命令的线形已被
`@tauron/contract-tests` 与接入方依赖）。

**门禁**：「`plugin_invoke` 必须有非测试的可达注册（示例 app 或 harness 宿主）」
+「`@tauron/host` 不得 import `@tauron/core` 的运行时类」（P1-1 的解必须被钉住）。

**验证**：示例 app 同时注册两套 handler，走 `plugin_invoke` 真能调到插件，
不再返回 SC-9001。

**量级：大（2 轮）**。

---

### A6. contributes 对账与驱动 UI + 跨插件调用（解 P1-4 / P1-5）

1. **补 `host_contributes_reconcile`**（self 档）：比对 manifest 声明与 activate 期注册，
   分叉返回 `E_CONTRIBUTES_DRIFT`（**追加到 `ErrorCode` 枚举末尾**，遵守既有约定）。
   触发点：安装完成、启用、`registry_list`（只读比对，不改状态）。
2. **驱动 UI**：`ShellController` 接上 `oc-command-select` → 查 contributes → 派发到
   `host_plugin_call`（经 A1 的投递通路）。`oc-plugin-manager` 从哑组件升级为容器。
3. **修门禁漏洞**：`wire-gate:1523` 的「接线 **或** 注释声明接入方域」改为
   「必须有 `addEventListener` 的**真实消费方**，或在 `UNWIRED_EVENTS` 常量里显式登记
   且该常量被测试断言长度」（当前「写句注释就放行」等于门禁失效）。
4. **跨插件调用**：新增 `host_call_plugin(target, method, args)`，要求双向声明
   （调用方 manifest `dependencies` + 被调方 `public` contributes），否则 `E_AUTH_DENIED`。

**量级：中（1 轮）**。

---

### A7. 底座五域 Provider + 部署面（解 P1-9 / 部署缺口）

复用 0.3 的 S2/S5 设计（不重复造）：引入 `trait MenuProvider` / `TrayProvider` /
`FsProvider` / `HttpProvider` / `UpdaterProvider`，缺省 = `native_supported() == false` →
返回 `UnsupportedBody`。优先级 **fs > http > updater > menu > tray**。
同时补 `crates/tauron-adapter/permissions/`（**由 `authz::COMMANDS` 生成，不手写**）
与三档 capability 模板。

**量级：大（2 轮，按域拆）**。

---

### A8. 横切：门禁加固与文档收口

- **A8-1 门禁解析健壮性**：wire-gate 的 `parse()` 类辅助函数一律用
  「容忍换行与注释」的正则，并加**解析到空即失败**的断言（防 P0-4 那类假红/假绿）。
- **A8-2 孤儿事件门禁**（见 A6-3）。
- **A8-3 文档漂移门禁**：对外文档的形态表 / 能力清单必须由代码符号推导或显式登记，
  与 `X1` 同一机制。
- **A8-4 同步 0.3 文档**：按本次实测重写 `multi-plugin-substrate-roadmap.md` 的 §2 残差表
  （M-1/M-2 已实现，需标注为「已实现但 feature-gated / 已接线」）。
- **A8-5 孤儿 crate 收口**：brand / theme / distribute 三选一决策入
  `canonical-owners.md` 的机器可读表（wasm 因 A4 转为「可选组件（已接线时）」）。

**量级：中（分散在各轮收尾）**。

---

## 8. 轮次编排（轮 19 起，承接 0.3 的轮 13–18）

依赖顺序原则：**先补动词（A1 调用投递），再补名词（A2 安装 / A4 形态），最后收结构（A5）**。
理由是 A6 的命令面板、A4 的 Native/Wasm、A2 的安装后启用，全部要经 A1 的投递通路。

| 轮 | 内容 | 完成判据 |
|---|---|---|
| **轮 19** | **修 P0-4 红灯** + A2 的 feature↔能力表对齐（A8-1 同源） + A8-4 文档同步 | `pnpm -r test` **0 红**；`available('host_registry_install')` 与 Rust `cfg!` 一致；0.3 §2 残差表按实测更新 |
| **轮 20** ✅ | **A1 调用投递闭环**（经用户决策合并：Js 与 Process 一轮打通，宿主→插件与插件→插件双向） | **已完成（2026-09-26）**：`CallDelivery` 抽象 + Js（request 通道）/ Process（stdin/stdout 帧回路）/ Unwired 三实现；`host_call_plugin` / `host_call_result` / `host_call_take` 三命令全链；Rust 闭环测试 `process_call_delivers_frame_to_stdin_and_settles_via_sink` 绿；SDK 执行泵自动接单回填；wire-gate 137/137、tauron-host 361/361、adapter 209（install 214）、host 281、proc 58 全绿。未装配 delivery → `Unsupported`（UnwiredDelivery 兜底） |
| **轮 21** ✅ | **A3 进程插件 JSON-RPC 收尾**（帧回路已于轮 20 提前接线） | **已完成（2026-09-26，发布审计深化）**：① stdout 读线程存在性**门禁**（piped×2 + thread::spawn + BufReader + on_frame + EOF 回收 `remove(&pid)`，剥注释后断言防诚实边界注释假红）——变异验证拔掉读线程门禁红；② 原"心跳按插件分账"修复在发布审计中**深化为删除**：`ProcRunner` 模拟器全仓零生产调用方，连同全局心跳单例与专属类型整体移除（真实链路 `CommandSpawner` 无共享心跳状态；心跳监控未实现属诚实边界）。sidecar **真实**收发端到端仍需真 sidecar 集成环境（诚实边界保留，见 A3 §更新）。proc 17、wire-gate 140 全绿 |
| **轮 22** ✅ | **A2 后半**（示例换主推 SDK，端到端证据） | **已完成（2026-09-26）**：`examples/minimal-app/src/plugin/first.ts` 改用 `@tauron/app-plugin-sdk`（`createPlugin` + `createPluginContext` 接宿主 RPC 面，声明 `format` 命令即开执行泵）；旧 iframe 实现保留为 `legacy-first.ts` 并标注 deprecated；新增 `plugin-window.html` 构建入口（插件面板窗口页，由 `host_window_create` 依 manifest `entry.ui` 加载进 `plugin-<id>` webview）；主窗新增 §5 演示（`windowCreate` → `callPlugin` → 轮询 `callTakeResult`）；wire-gate 新增「app-plugin-sdk 必须有非测试的消费方」门禁（变异验证红）。wire-gate 140、`tsc` + `vite build`（3 入口）全绿。**诚实边界**：真实运行时端到端（安装签名包 → 开窗 → 泵回帧）需真 Tauri 环境，本环境以构建 + 门禁 + 单测为证 |
| **轮 23** | **A4-a** Native（Rust）静态注册 | `PluginType::Rust` 有执行器 |
| **轮 24** | **A4-b** Wasm 引擎接入（feature-gated） | Wasm 插件真执行；默认构建不含 wasm 依赖 |
| **轮 25** | **A5 阶段 2**（委托） | `plugin_invoke` 不再恒返回 SC-9001，示例 app 双注册可达 |
| **轮 26** | **A5 阶段 3**（删除 + 解 P1-1） | shell 三套 legacy 实现删除；`@tauron/host` 不 import `@tauron/core` 运行时类 |
| **轮 27** | **A6** contributes 对账 + 驱动 UI + 跨插件调用 | drift 可检出；命令面板真调到插件；孤儿事件门禁生效 |
| **轮 28** | **A7** fs → http → updater → menu → tray + permissions 生成 | fs/http 可用或明确 `Unsupported`；permissions 自 authz 生成 |
| **轮 29** | **A8 收口** + 全量诚实性对账 | 本文 §6 清单全部有处置结论（已修 / 已登记 / 明确不做） |

> **每轮门槛**（沿用 R1–R8）：`cargo check --workspace --all-targets` 0 警告；
> `cargo test --workspace --locked --lib --tests` 全绿 **且**
> `--features tauron-adapter/plugin-install` 全绿（feature 门控必须与主测并列）；
> `pnpm -r test` **0 红**；wire-gate 全绿。**每轮结束都是可发布态**。

---

## 9. 验收标准（0.4 发布条件）

1. **调用真通**：`host_plugin_call` 在至少 Js 与 Process 两种形态下真投递、真应答；
   未装配通路时返回有类型的 `Unsupported`，不返回永不结束的 pending。
2. **安装默认可达或明确标注**：`host_registry_install` 的可用性与 TS `available()` 同源；
   默认构建不允许出现「能力表说有、invoke 说没有」。
3. **四种形态各有落点**：`PluginType` 四变体每个都有执行器或 `E_PLUGIN_TYPE_NO_RUNTIME`
   + 文档标注；对外文档形态表与枚举一致（无「4+1」这类代码里不存在的宣称）。
4. **进程插件能聊**：sidecar 真收真回，stdout 读线程存在性有门禁。
5. **框架层复活**：`plugin_invoke` 有非测试的可达注册，不再恒返回 SC-9001；
   shell 的 legacy 三套实现已删除；`@tauron/host` 不 import `@tauron/core` 运行时类。
6. **扩展点闭环**：contributes 可对账（drift 可检出），命令面板由 contributes 驱动，
   孤儿事件门禁生效（不再「写句注释就放行」）。
7. **门禁 0 红**：`pnpm -r test` 与 wire-gate 在默认构建下全绿，且解析类辅助函数
   有「解析到空即失败」保护。
8. **诚实披露**：本文 §6 的每一条都有处置结论；无「有类型、有接口、无入口」的第三态
   被描述为可用。
9. **底座能力面**：fs 与 http 两域可用或返回有类型的 `Unsupported`；
   `host_capabilities` 的 `families`/`unsupported` 由注册集合推导而非硬编码。
10. **全量 battery 绿**（同 §8 门槛）。

---

## 10. 风险、不变量与边界

### 10.1 硬不变量（不得回退）

沿用 R1–R8 与 0.3 的六条（底座不触达注册表、单向依赖、ui-primitives 不依赖 host、
`cmd_*` 零 Tauri 调用、身份判定在副作用之前、两套错误码不合并），并新增：

7. **成功形状不得用于表达「没做」**（`UnsupportedBody` / `DegradedValue` / `simulated`）。
8. **能力表只能由运行时真相推导**，不得硬编码。
9. **调用登记的 pending 表与投递通路职责分离**，投递失败不改 pending 的 TTL 语义。

### 10.2 风险

| 风险 | 缓解 |
|---|---|
| A1 引入投递抽象后，pending 语义与投递语义耦合回退 | trait `CallDelivery` 只接收 `&PendingCall`，不得写 pending 表；门禁断言其签名不含 `&mut Registry` |
| A4-b 引入 wasm 引擎破坏「零依赖」承诺 | 严格 feature-gated 且默认关闭；`cargo tree --no-default-features` 门禁断言 |
| A5 阶段 3 删除触发 P1-1 的隐藏耦合 | 先解 P1-1（把三个类迁出 `@tauron/core`）再删；门禁钉死「host 不得 import core 运行时类」 |
| A3 接管道后子进程被阻塞死 | 读线程随进程生命周期，门禁断言 `Stdio::piped()` 必配对读线程 |
| 轮次长，中途需发布 | 每轮结束都是可发布态（battery 绿 + 文档同步） |
| 本方案自己制造新的「有接口无入口」 | 每条 A 项都配**入口可达性门禁**；轮末做负向突变验证 |

### 10.3 明确不做

- **不合并两套错误码**（`SC-xxxx` / `E_*`）。分层是有意设计。
- **不做进程内 JS 沙箱运行时**。Js 型隔离来自 webview 边界 + 身份判定。
- **不做进程内内存/CPU 配额**。改为记录与暴露指标并在文档写明。
- **不删除 `tauron-shell` 的协议门面**。A5 删的是它的三套 legacy **引擎实现**，
  `plugin_invoke` 三命令的线格式与协议保持不变。
- **不改写已收口轮次的结论**。R1–R8 的六条硬不变量只增不改。

---

## 11. 关联文档（本方案落地后需同步）

| 文档 | 需同步内容 |
|---|---|
| [overview.md](./overview.md) | 三层结构（补充「活线依赖死线」）；接线状态表按 A8-5 归置表更新；命令面 59 条 |
| [app-layer-wire.md](./app-layer-wire.md) | 新增 install 的 feature 可达性说明、调用投递的 `DeliveryReceipt` 线形、contributes reconcile |
| [canonical-owners.md](./canonical-owners.md) | crate 归置表按 A4/A5/A8-5 更新；wasm 从「可选组件」改为「可选组件（已接线）」 |
| [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md) | §2 残差表按本次实测重写（M-1/M-2 已实现） |
| `docs/api/plugin-development-guide.md` | 形态表按 A4 重写（补 Rust 行、删 B+ 行、改 Js 描述）；示例换主推 SDK |
| README.md | 形态宣称、能力清单按 §6 处置结论更新 |
