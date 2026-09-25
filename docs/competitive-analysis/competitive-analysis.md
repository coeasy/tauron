# Tauron 竞品全景图与架构设计梳理

> 生成日期：2026-09-23
> 项目：tauron / tauron
> 定位：Tauri 2 插件化客户端基础框架 —— 安全沙箱、双世界隔离、多形态插件加载的开源基础设施

---

## 〇、兑现度标记的含义与复核方式

本文档的**头条特性/卖点**（第一章「差异化优势」、第 2.2 节对比矩阵的 Tauron 列、
第六章「已知设计亮点」）逐条带三档**兑现度标记**。判定口径固定为
「有没有实现 + 有没有测试 + 在不在生产路径上真的被调用」，**不以设计文档的描述为准**：

| 标记 | 含义 | 判定依据 |
|---|---|---|
| ✅ 已接线 | 有实现 + 有测试 + 在真实调用路径上（命令体 / 导出 API 真的调它，或被门禁锁定） | 源码实现体 + 测试文件 + 调用点 |
| 🟡 参考实现 | 自带测试，但**未接线**（库独立存在、命令体是桩、调用点缺失，或只跑通了部分链路） | 见各条括注的文件/行 |
| ⚪ 规划 | 无实现，或仅保留诚实失败码 | 见各条括注的文件/行 |

**复核方式**（任何人可复现）：

1. **Rust 侧**：`cargo test -p <crate>` 看「有没有测试」；看
   `crates/tauron-adapter/Cargo.toml` 的依赖表判断「有没有接线」——**没进该依赖表的
   crate，其对应 `host_*` 命令体一定是桩**；再打开
   `crates/tauron-adapter/src/lib.rs` 的 `cmd_*` 实现体确认是否真的调了它。
2. **TS 侧**：`pnpm --filter <包名> test` 看测试；接线与否看是否存在调用点
   （例如 `@tauron/dual-world` 是否被插件运行时引用）。
3. **交叉核对**：`docs/architecture/overview.md` 的「接线状态（诚实披露）」一节是
   仓库自述的同一份事实；`docs/architecture/app-layer-wire.md` 记录线格式与门禁。

> 标记核对日期 = **2026-09-24**（即本次改动的日期）。标记只对当日代码成立；
> 代码先动、标记后动，或两者不一致时，以代码与 `cargo test` / `pnpm test` 实测为准。
>
> ⚠️ **并发重构披露（已收口）**：核对窗口内 `crates/tauron-adapter` 曾被另一条工作流
> 并发修改（R7：settings 换 `SettingsStore`、通知接 `DispatchSink`）。该工作流**已落地
> 并通过四条 battery**，原先标为「接线进行中」的两条（配置四层合并、系统通知派发）
> 已按实测改判；本文行号引用一律以符号名（函数/类型/宏）为准。
>
> ⚠️ **同窗口的第二、三批改动（R8 + 身份收口，已落地）**：`WindowSink`/`DialogSink`/
> `DeepLinkSink` 三组 Sink 抽象 + `tauron_plugin_as_host_command!` 装配器 + 两条新命令
> （`host_window_relaunch` 先对账后重启、`host_window_create` 只认 manifest 的 UI 入口）、
> 商城三命令的 `simulated` 成为**线字段**；以及**代码层身份判定**（仅主窗 / 绑定自身
> 命名空间 / 绑定自身身份 / 按身份过滤四类，见 `docs/architecture/app-layer-wire.md` §5）。
> 同一轮还修掉三处"仿真冒充成功"与一处"命令连错目标"（`oc-restart` 曾连 `host_window_quit`，
> 只退不重启且跳过恢复对账）。该轮收口时的实测计数：Rust **1221** / TS **1571** /
> wire-gate 当轮计数；命令面 **54 = 底座 38 + 插件运行时 16**。
>
> **后续更新（2026-09-25，三轮全链路审计之后）**：TS **1626**（`pnpm -r test` 实跑）；
> Rust 源码 `#[test]` 声明数 **1284**（**不是**执行结果，feature 门控另计，真实执行以
> CI 为准）；ESLint 0 error。本文其余兑现度标记仍以文首的核对日期为准。

---

## 一、项目定位分析

| 维度 | 特点 |
|------|------|
| **核心价值主张** | 为 Tauri 2 桌面应用提供完整的插件化基础设施：沙箱隔离、权限管控、插件市场、白标品牌化、多形态插件加载 |
| **目标用户** | ① 基于 Tauri 2 开发桌面应用的团队/个人 ② 需要可扩展插件系统的客户端产品 ③ 需要白标/OEM 多品牌的企业 |
| **核心功能** | 4+1 插件形态（Rust/JS/WASM/Process/B+）🟡、双世界沙箱 🟡、双层 ACL ✅、插件信封协议 ✅、事件总线 ✅、CLI 工具链 🟡、插件市场 🟡、品牌化 ✅（构建期）/🟡（运行期） |
| **技术架构** | Rust 核心（15 crates 平台无关）+ TypeScript 层（17 packages）+ Tauri 2 适配器 + Lit/React/Vue/Svelte UI |
| **差异化优势** | ① 平台无关核心（不依赖 tauri crate）✅ ② 三档授权模型 ✅ ③ 四种插件形态+混合模式 🟡 ④ 完整的商城-签名-灰度分发链路 🟡 ⑤ 双世界隔离（QuickJS-WASM）🟡 —— 标记含义见 §〇，逐条依据见第六章 |

---

## 二、竞品全景图

### 2.1 竞品分类表

| # | 竞品名称 | 简介 | 平台 | 核心亮点 | 开源 | 竞品类型 |
|---|---------|------|------|----------|------|----------|
| 1 | **Tauri 2 官方插件系统** | Tauri 内置 Plugin trait + ACL | 跨平台 | Plugin trait、capabilities/permissions/scopes、IPC v2 | ✅ | 直接竞品（底层） |
| 2 | **Taurify (CrabNebula)** | Tauri 官方合作公司的可扩展 Shell | 跨平台 | Extensible Shell、默认壳内置插件、OTA、四端覆盖 | ❌ | 直接竞品 |
| 3 | **VSCode Extension Host** | VSCode 的进程级插件隔离架构 | 桌面 | 独立 Extension Host 进程、延迟激活、LSP/DAP 协议 | ✅ | 间接竞品 |
| 4 | **DSH Desktop** | Electron + Cordis 组合式插件系统 | 桌面 | "万物皆插件"、桌面壳即插件、Profile 隔离 | ❌ | 间接竞品 |
| 5 | **OpenAkita** | Tauri + AI 多 Agent 插件平台 | 跨平台 | Plugin SDK + 6 层沙箱、30+ LLM、89+ 工具 | ✅ | 间接竞品 |
| 6 | **Headlamp** | Kubernetes 插件化 Web UI | Web/桌面 | 插件注册表、白标主题、headlamp-plugin CLI | ✅ | 间接竞品 |
| 7 | **Extism** | 跨语言 WASM 插件框架 | 跨平台 | 可选字节 ABI、10+ 宿主 SDK、Manifest 能力白名单 | ✅ | 开源替代 |
| 8 | **Wasmtime** | Bytecode Alliance WASM 运行时 | 跨平台 | Cranelift 优化、fuel 资源治理、WASI Preview 2 + Component Model | ✅ | 开源替代 |
| 9 | **ZeroClaw** | Rust 原生 AI Agent WASM 运行时 | 跨平台 | WIT 契约、Landlock 内核沙箱、deny-by-default 工具白名单 | ✅ | 潜在竞品 |
| 10 | **Dioxus** | 类 React 纯 Rust GUI 框架 | 跨平台 | RSX + 信号状态管理、WebView 渲染、Subsecond 热更新 | ✅ | 跨界竞品 |

### 2.2 核心功能对比矩阵

| 功能维度 | **Tauron** | Tauri 2 原生 | Taurify | VSCode Ext | DSH Desktop | Extism | ZeroClaw | **Tauron 兑现度**（2026-09-24） |
|----------|:----------:|:------------:|:-------:|:----------:|:-----------:|:------:|:--------:|:---:|
| 插件生命周期管理 | ✅ 10 态状态机 | ⚠️ 基础 initialize | ✅ | ✅ 完整 | ✅ Cordis | ❌ | ✅ | ✅ 已接线（`crates/tauron-host/src/lifecycle.rs`；`host_lifecycle_report` / `host_registry_admin` 实调） |
| 多形态插件加载 | ✅ 4+1 形态 | ❌ 仅 Rust | ⚠️ | ⚠️ 仅 JS | ⚠️ 仅 JS | ✅ WASM | ✅ WASM | 🟡 参考实现——js 命令面已接线；process 仅「真起进程 / 真探测 / 真终止」，**stdin/stdout JSON-RPC 帧回路未接线**（`crates/tauron-proc/src/spawner.rs:82`，stdout 走 `Stdio::null()`）；wasm **无运行时**，只回诚实失败码 `E_PLUGIN_TYPE_NO_RUNTIME`（`crates/tauron-adapter/src/lib.rs` 的 `cmd_runtime_spawn`）；B+ 双世界未接线 |
| 安全沙箱 | ✅ 双世界+WASM | ⚠️ ACL-only | ⚠️ | ✅ 进程隔离 | ❌ | ✅ 沙箱 | ✅ 内核级 | 🟡 参考实现（`packages/tauron-dual-world/src/sandbox.ts:134` 仍是 `simulate execution`；wasm 无运行时） |
| 双层 ACL 权限 | ✅ 静态+动态 | ✅ capabilities | ✅ | ✅ contributes | ❌ | ✅ Manifest | ✅ deny-default | ✅ 已接线（动态三档授权 `tauron_host::authz::resolve_principal` + origin 门，见 `crates/tauron-adapter/src/tauri.rs:684,716,1248`）；授予/审批链 `tauron-acl` 🟡 未接线 |
| 插件市场/商城 | ✅ 完整链路 | ❌ | ✅ | ✅ Marketplace | ✅ dshmarket | ❌ | ❌ | 🟡 参考实现（`host_market_check` 恒 `{available:false}`、`host_market_download/install` 恒 `{simulated:true}`，`crates/tauron-adapter/src/lib.rs` 的 `cmd_market_check` / `cmd_market_download` / `cmd_market_install`；`tauron-market`/`tauron-distribute` crate 自带测试但未进适配层依赖表） |
| 白标品牌化 | ✅ CI 矩阵 | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ 已接线（**构建期**：`packages/tauron-app-cli/src/brand.ts` 的 `generateCiMatrix` / `generateBrandFiles` 有测试；**运行期** `host_brand_info` 仍是桩，返回 `{}`——`crates/tauron-adapter/src/lib.rs` 的 `cmd_brand_info`） |
| 灰度发布 | ✅ 4 阶段 | ❌ | ✅ OTA | ❌ | ✅ Beta 通道 | ❌ | ❌ | 🟡 参考实现（`crates/tauron-distribute` 51 个测试，未进适配层依赖表，无命令面） |
| 事件总线 | ✅ 三通道 | ⚠️ 基础 | ✅ | ✅ | ✅ Cordis | ❌ | ❌ | ✅ 已接线（`host_events_publish/subscribe/unsubscribe/drain`，`kind` = event/request/state） |
| 多 UI 框架适配 | ✅ R/V/S/Lit | ❌ | ⚠️ | ❌ Webview | ⚠️ React | ❌ | ❌ | ✅ 已接线（`@tauron/adapter-react` / `adapter-vue` / `adapter-svelte` / `ui-primitives`，各自 5–14 个测试文件） |
| CLI 工具链 | ✅ 完整 | ⚠️ basic | ✅ | ✅ yo 生成器 | ❌ | ✅ PDK | ❌ | 🟡 部分接线：`@tauron/app-cli` 多数命令有实现与测试（`pluginSign` 已接 `@tauron/market`，2026-09-24）；但 `@tauron/cli` 的 `plugin sign` 仍是 `hash*31` 假签名并谎报 `algorithm: 'ed25519'`（`packages/tauron-cli/src/plugin-lifecycle.ts:247`），`plugin publish` 只生成端点不落网络 |
| 跨语言契约测试 | ✅ TS↔Rust | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ 已接线（`@tauron/contract-tests` 的 `contract.test.ts` + `wire-gate.test.ts` 解析两侧源码逐调用点比对） |
| i18n 国际化 | ✅ 插件命名空间 | ❌ | ⚠️ | ✅ | ❌ | ❌ | ❌ | ✅ 已接线（`host_i18n_*` 6 条命令实调 `tauron_i18n`；`plugin:<id>.oc.<key>` 前缀在 `cmd_i18n_load` 落地） |
| 崩溃恢复 | ✅ 三级降级 | ❌ | ❌ | ✅ 自动重启 | ❌ | ❌ | ❌ | ✅ 已接线（`tauron-recovery` 进依赖表，`host_recover_boot/report/trial_enable` + `reconcile_recovery_phase`）；⚠️ **进程崩溃检测是轮询式的**（无后台监控线程，`crates/tauron-adapter/src/lib.rs` 的 `cmd_runtime_health` 文档注释） |
| 配置四层合并 | ✅ builtin→plugin→user→session | ❌ | ❌ | ✅ contributes | ❌ | ❌ | ❌ | ✅ **已接线**（R7 收口：`tauron-settings` 已进 `crates/tauron-adapter/Cargo.toml`，`SubstrateState.settings` 就是 `SettingsStore`，`host_settings_get/set` 走 `Store`；数据版本迁移 v1→v2 有**显式线上入口** `host_settings_adopt_legacy` / `host_settings_migrate`，两条都已注册进 handler 宏）。⚠️ **诚实边界：Store 目前是内存态，没有磁盘持久化**——`doc` 的来源是宿主自己读盘；四层实为 `builtin < brand < plugin < user` |

### 2.3 竞争力总结

**Tauron 的核心优势：**
1. ✅ **唯一提供完整插件化基础设施的 Tauri 框架** — 从沙箱到商城到品牌化的一站式方案（注：其中"沙箱 / 商城"两环当前是 🟡 参考实现，见 §〇 与第六章）
2. 🟡 **4+1 插件形态** — 竞品最多支持 2 种形态，Tauron 支持 Rust/JS/WASM/Process/B+（形态在清单与状态机层面齐备；**只有 js 的命令面完整接线**，process 的 JSON-RPC 帧回路未接线，wasm 无运行时，B+ 双世界未接线）
3. ✅ **平台无关核心** — Rust crates 不依赖 tauri，可被其他框架复用
4. 🟡 **生产级安全设计** — 双层 ACL ✅、三档授权 ✅、HMAC 签名 🟡（`tauron-acl` 未接线）、安全解压 🟡（仅校验）、路径清洗 🟡

**Tauron 的主要劣势：**
1. **生态起步期** — 插件数量和社区规模远不及 VSCode/Taurify
2. **两代包命名并存** — `@tauron/*` 和 `@tauron/*` 双轨增加集成成本
3. **架构文档缺失** — docs/ 目录几乎为空，设计决策全靠代码注释中的"计划 §X.X"（2026-09-24 复核：**已过时**——`docs/architecture/` 现有多份设计文档与线格式规范，另见 §〇 的复核入口）
4. **部分模块实现深度不一** — proc/wasm 的运行时逻辑依赖 stub

**最大威胁：** Taurify（CrabNebula / Tauri 官方合作）— 直接在 Tauri 生态内竞争，拥有官方背书和社区信任

**市场空白机会：** 
- WASM 插件系统（Extism/Wasmtime 级别的沙箱）与桌面客户端的深度集成
- 白标/OEM 品牌化是所有竞品均缺乏的差异化能力
- 跨语言契约测试在插件框架领域几乎无人做

---

## 三、项目架构详细设计

### 3.1 整体架构图

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        应用层 (Application)                               │
│    React App / Vue App / Svelte App / 原生 TS App                        │
│    (使用 HostClient + UI Components + Framework Bindings)                │
├──────────────────────────────────────────────────────────────────────────┤
│                    UI 框架适配层 (@tauron/adapter-*)                       │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────────┐│
│  │ adapter-react│ │ adapter-vue  │ │adapter-svelte│ │  @tauron/ui ││
│  │ useInvoke    │ │ useInvoke    │ │ createStore  │ │  (Lit WebComp)   ││
│  │ useEvent     │ │ useEvent     │ │ events       │ │  <oc-toast>      ││
│  │ useCaps      │ │ useCaps      │ │ capabilities │ │  <oc-plugin-mgr> ││
│  └──────────────┘ └──────────────┘ └──────────────┘ └──────────────────┘│
├──────────────────────────────────────────────────────────────────────────┤
│                    能力编排层 (@tauron/core)                          │
│    HostClient / TauriBackend / invoke / events / channels / gates        │
├──────────────────────────────────────────────────────────────────────────┤
│                    框架核心层 (@tauron/core)                               │
│    invokePlugin / EventBus / ACL / Registry / Config / Runtime           │
├──────────────────────────────────────────────────────────────────────────┤
│                    类型定义层 (@tauron/types)                              │
│    Envelope / ErrorCode / PluginState / Manifest / Events / ACL          │
├──────────────────────────────────────────────────────────────────────────┤
│                  ┌───────────────────────────────────────┐               │
│                  │  Tauri 命令适配层 (tauron-adapter) │               │
│                  │  #[tauri::command] 薄包装              │               │
│                  │  generate_handler!() 宏 → 17 commands  │               │
│                  │  init() Tauri Plugin 入口              │               │
│                  └───────────────┬───────────────────────┘               │
├──────────────────────────────────┼───────────────────────────────────────┤
│                  Rust 壳层       │    TS 插件 SDK 层                      │
│  ┌──────────────────────────┐   │   ┌──────────────────────────────────┐ │
│  │    tauron-shell           │   │   │  @tauron/plugin-sdk              │ │
│  │  envelope / acl / registry│   │   │  PluginBridge (postMessage)      │ │
│  │  eventbus / config        │   │   │  插件宿主侧代理                  │ │
│  └──────────────────────────┘   │   └──────────────────────────────────┘ │
│  ┌──────────────────────────┐   │   ┌──────────────────────────────────┐ │
│  │   tauron-host        │   │   │  @tauron/dual-world              │ │
│  │  manifest / lifecycle     │   │   │  QuickJS-WASM 沙箱               │ │
│  │  registry / authz         │   │   │  JS↔WASM 桥接                   │ │
│  │  eventbus / config        │   │   └──────────────────────────────────┘ │
│  └──────────────────────────┘   │                                        │
├──────────────────────────────────┴───────────────────────────────────────┤
│                      插件运行时 (Plugin Runtimes)                         │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────────┐│
│  │  tauron │ │  tauron │ │  Rust Native │ │  B+ 混合模式     ││
│  │  -proc       │ │  -wasm       │ │  Plugin      │ │  JS + Host 双世  ││
│  │  进程插件    │ │  WASM 插件   │ │  原生集成    │ │  界隔离          ││
│  │  sidecar     │ │  Extism      │ │              │ │                  ││
│  │  JSON-RPC    │ │  QuickJS-WASM│ │              │ │                  ││
│  └──────────────┘ └──────────────┘ └──────────────┘ └──────────────────┘│
├──────────────────────────────────────────────────────────────────────────┤
│                    横切关注点 (Cross-Cutting Concerns)                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────────────┐│
│  │  brand   │ │  theme   │ │  i18n    │ │  notify  │ │  settings      ││
│  │  白标    │ │  主题    │ │  国际化  │ │  通知    │ │  设置中心      ││
│  ├──────────┤ ├──────────┤ ├──────────┤ ├──────────┤ ├────────────────┤│
│  │  acl     │ │  market  │ │distribute│ │  schema  │ │  recovery      ││
│  │  权限    │ │  商城    │ │  灰度    │ │  Schema  │ │  崩溃恢复      ││
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └────────────────┘│
└──────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Rust Crate 架构详解

#### 3.2.1 tauron-host — 宿主核心

**职责：** 平台无关的宿主核心，管理插件的完整生命周期。

| 子模块 | 文件 | 核心设计 |
|--------|------|----------|
| **manifest** | `manifest.rs` | 插件清单校验：反域名 ID（`com.example.plugin`）、权限词表验证、framework range 声明 |
| **lifecycle** | `lifecycle.rs` | **10 态状态机**：DISCOVERED→INSTALLING→INSTALLED→ENABLED→RUNNING→DISABLED→ERRORED_RETRYABLE→ERRORED_USER_CONFIRM→INSTALL_FAILED→UNINSTALLED。通过 `transition()` 单点收口，每属性唯一写入者 |
| **registry** | `registry.rs` | 插件注册表：max 8 插件、LRU 活跃身份、pending call TTL GC |
| **authz** | `authz.rs` | 三档授权模型：`self`（从 webview label 解析身份，忽略前端入参）/ `scoped-read`（结果过滤）/ `privileged`（显式授予） |
| **eventbus** | `eventbus.rs` | 三类通道语义分离：Event（可丢，丢最旧）/ Request（可靠，硬失败）/ State（快照替换）。每插件独立有界队列，背压可配置 |
| **config** | `config.rs` | 客户端配置聚合 |

**关键安全设计：**
- `self` 档命令从 `WebviewWindow.label()` 自动解析身份，**不信任前端入参** — 这是防伪造的核心机制
- 状态机 `transition()` 函数是唯一的状态变更入口，杜绝并发写入
- 禁止授予清单（如 `shell:allow-execute`）在授予路径上硬拒

#### 3.2.2 tauron-shell — Rust 壳层

**职责：** 更通用的 tauron 框架版本壳层，提供信封协议、ACL、注册表、事件总线。

| 子模块 | 核心设计 |
|--------|----------|
| **envelope** | 信封协议：`PluginInvokeRequest` → `PluginInvokeResponse` + `ProgressEvent`，统一调用/取消/进度 |
| **acl** | 双层 ACL：外层 Tauri 静态 + 内层框架动态 |
| **registry** | 插件注册表（独立于 tauron-host） |
| **eventbus** | 事件总线实现 |
| **config** | 配置管理 |
| **error** | 错误映射（14 个错误码，按可重试性分类） |

> **设计说明：** `tauron-shell` 和 `tauron-host` 是两个独立的宿主核心实现。tauron-shell 面向通用 tauron 框架，tauron-host 面向 tauron 定制。两者都不依赖 tauri crate。

#### 3.2.3 tauron-adapter — Tauri 命令适配层

**职责：** 平台无关 cmd_* 函数 + Tauri 命令薄包装。

```rust
// 核心模式：平台无关函数 + feature-gated Tauri 包装
fn cmd_plugin_invoke(...) -> Result<InvokeResponse> { /* 平台无关逻辑 */ }

#[cfg(feature = "tauri")]
#[tauri::command]
pub async fn plugin_invoke(...) -> Result<InvokeResponse, String> {
    cmd_plugin_invoke(...).map_err(|e| e.to_string())
}

// generate_handler!() 宏一次性注册 17 个命令
generate_handler! {
    plugin_invoke, plugin_cancel, plugin_list, plugin_info, ...
}

// init() 提供标准 Tauri Plugin 入口
pub fn init() -> TauriPlugin<Wry> { ... }
pub fn init_with_config(config: ScuteConfig) -> TauriPlugin<Wry> { ... }
```

**第三方集成只需一行：**
```rust
tauri::Builder::default().plugin(tauron_adapter::tauri::init())
```

#### 3.2.4 tauron-proc — 进程插件 Host

| 特性 | 实现 |
|------|------|
| 启动 | sidecar spawn + 签名校验 |
| 通信 | JSON-RPC over stdio/WS |
| 帧限制 | maxFrameBytes 上限，stdout 仅承载协议帧 |
| 健康检查 | 心跳 + 崩溃检测 |
| 并发控制 | 背压机制 |
| 超时 | 空闲超时自动 kill |
| 安全 | ABI 指纹校验 |
| 稳定性 | per-plugin 崩溃重启限制（3 次 / 5 分钟） |

#### 3.2.5 tauron-wasm — WASM Supervisor

| 特性 | 实现 |
|------|------|
| 运行时 | Extism 加载 |
| API 白名单 | host_fn 白名单机制 |
| 内存限制 | memory.max_pages 配额（默认 64MB） |
| 实例管理 | 实例池（LRU） |
| 模块缓存 | 模块缓存（LRU 有界） |
| 安全 | ABI 指纹校验 |
| 稳定性 | per-plugin 崩溃计数 |

#### 3.2.6 tauron-acl — 权限授予与审批

| 特性 | 实现 |
|------|------|
| 安装时审批 | 权限审批流程 → 物化为 Tauri Capability |
| 签名防篡改 | 授予集落盘 + HMAC-SHA256 签名 |
| 差分检测 | 权限/scope 变更自动检测 |
| UX | 高危档逐条展开 + 默认不勾选 |
| 硬拒 | 禁止授予清单（如 `shell:allow-execute`）在授予路径硬拒 |

#### 3.2.7 tauron-market — 插件商城

| 特性 | 实现 |
|------|------|
| CRUD | 索引 / 搜索 / 安装 / 更新 / 卸载 / 吊销 API |
| 签名验证 | RFC 8785 规范化签名 |
| 安全解压 | 路径清洗、条目≤2000、解压≤200MB、压缩比≤100× |
| 版本管理 | 版本单调递增检查 |
| 审计 | 链式 hash 审计日志 |

#### 3.2.8 tauron-distribute — CI 分发运维

| 特性 | 实现 |
|------|------|
| 灰度发布 | 1%→5%→25%→100%，每批最小停留 1h |
| 崩溃门禁 | 5% 崩溃率阈值自动停发 |
| 签名校验 | 发布产物签名验证 |
| 可靠性 | 双 endpoint 发布 |

#### 3.2.9 tauron-brand — 白标品牌化

| 特性 | 实现 |
|------|------|
| 品牌配置 | `BrandConfig`(identifier/protocol/autostart/data_dir/shortcuts/icons) |
| 唯一性 | `validate_uniqueness()` 校验多品牌间标识唯一性 |
| 配置合并 | `merge_config()` 深度合并 tauri.conf（数组替换、null 删键、递归合并） |
| CI 矩阵 | `expand_matrix()` 生成 品牌×平台×构建类型 矩阵 |
| dev 模式 | 自动加 `.dev` 后缀 |

#### 3.2.10 横切关注点 Crates

| Crate | 职责 | 关键设计 |
|-------|------|----------|
| **tauron-theme** | 主题/皮肤 | CSS 自定义属性 + `data-theme` 属性切换，插件可贡献主题(`contributes.themes`)，内置亮/暗主题 |
| **tauron-i18n** | 国际化 | 回退链 `zh-CN→zh→en-US`，插件文案命名空间 `plugin:<id>.oc.<key>`，缺失键回落计数，RTL 检测 |
| **tauron-notify** | 通知中心 | 环形缓冲(默认 500 条)，未读/分组/清空，系统通知静默降级(`DispatchSink` trait)，插件卸载清理 |
| **tauron-settings** | 设置中心 | SchemaRegistry + 四层合并(`builtin < brand < plugin < user`)，等值写回落 unset，schema 校验 |
| **tauron-schema** | Schema 管线 | `$ref` 本地展开、enum 内部标签化、`x-tauron→uiSchema` 转换、门禁(禁止嵌套 oneOf/anyOf) |
| **tauron-recovery** | 崩溃恢复 | 三级降级(Normal→Safemode→Repairmode)，启动失败计数(2次→安全模式)，幂等恢复动作 |

### 3.3 TypeScript Package 架构详解

#### 3.3.1 包命名体系

```
@tauron/*           ← tauron 框架层（平台无关核心）
@tauron/*      ← tauron 应用层（Tauri 绑定 + UI + CLI）
```

#### 3.3.2 框架层 (@tauron/*)

| 包 | 职责 | 关键导出 |
|---|------|---------|
| **@tauron/types** | 共享类型 | `Envelope`, `ErrorCode`, `PluginState`, `Manifest`, `Events`, `ACL` |
| **@tauron/core** | 平台无关核心 | `invokePlugin()`, `EventBus`, `ACL`, `Registry`, `Config`, `Runtime` |
| **@tauron/plugin-sdk** | 插件 SDK | `PluginBridge` (postMessage 沙箱通信)、插件宿主侧代理 |
| **@tauron/dual-world** | 双世界原型 | QuickJS-WASM 沙箱、JS↔WASM 桥接 |
| **@tauron/adapter-react** | React 适配 | `useInvoke`, `useEvent`, `useCapabilities`, `usePluginId` |
| **@tauron/adapter-vue** | Vue 适配 | `useInvoke`, `useEvent`, `useCapabilities` (composables) |
| **@tauron/adapter-svelte** | Svelte 适配 | `invoke`, `event`, `capabilities` (stores) |
| **@tauron/market** | 插件市场 | Ed25519 签名、`index.json` 生成、注册表 |
| **@tauron/shell-matrix** | Shell 矩阵 | 4 种形态：local / local-server / remote-url / sub-webview |
| **@tauron/cli** | CLI 工具 | `create` / `plugin new` / `dev` / `test` / `pack` / `sign` / `publish` / `doctor` |
| **@tauron/contract-tests** | 契约测试 | TS↔Rust 跨语言协议验证 |

#### 3.3.3 应用层 (@tauron/*)

| 包 | 职责 | 关键导出 |
|---|------|---------|
| **@tauron/core** | 能力编排层 | `HostClient`, `TauriBackend`, `invoke`, `events`, `channels`, `gates` |
| **@tauron/framework** | 框架薄封装 | signals 响应式内核 + 框架无关 bindings（useInvoke/useEvent/useCapabilities） |
| **@tauron/ui** | UI 组件库 | Lit Web Components + design tokens + `<oc-plugin-manager>` 等 |
| **@tauron/cli** | CLI | tauron 脚手架 / 插件开发 / 品牌构建 |
| **@tauron/plugin-sdk** | 插件开发 SDK | 类型安全、生命周期、Contributes 注册 |
| **@tauron/contract-kit** | 契约测试套件 | core API 行为 / 事件协议 / 错误码 + mock Backend |

### 3.4 核心设计模式分析

#### 3.4.1 信封协议 (Envelope Protocol)

```
┌─────────────┐     plugin_invoke(pluginId, method, payload)     ┌──────────────┐
│  前端应用    │ ───────────────────────────────────────────────→ │  Rust 壳层    │
│             │                                                  │  tauron-shell │
│  HostClient │ ←── PluginInvokeResponse(result/error) ──────── │              │
│             │ ←── ProgressEvent(progress/total/message) ────── │              │
│             │ ←── CancelledEvent ───────────────────────────── │              │
└─────────────┘                                                  └──────────────┘
```

- **单一入口：** 所有插件调用统一通过 `plugin_invoke` 命令
- **三类响应：** 最终结果(OK/Err) + 进度事件 + 取消通知
- **统一错误码：** 14 个错误码，按 SC-{类别}{编号} 命名，每类区分可重试性

#### 3.4.2 双世界隔离 (Dual-World Isolation)

```
┌─────────────────────────────────────────────────┐
│  World A (Host)                                   │
│  ┌─────────────────────────────────────────────┐│
│  │  宿主 JS 运行时                              ││
│  │  完整 Tauri API 访问                         ││
│  │  @tauron/core → invokePlugin()              ││
│  └─────────────┬───────────────────────────────┘│
│                │ postMessage (受限 API 白名单)    │
│  ┌─────────────▼───────────────────────────────┐│
│  │  World B (Guest) — QuickJS-WASM 沙箱        ││
│  │  插件代码运行于此                            ││
│  │  资源限制 + 超时 + API 白名单               ││
│  │  @tauron/plugin-sdk → PluginBridge          ││
│  └─────────────────────────────────────────────┘│
└─────────────────────────────────────────────────┘
```

- **World A** 是宿主运行时，拥有完整的 Tauri API 访问权限
- **World B** 是 QuickJS-WASM 沙箱，插件代码运行于此，通过 postMessage 桥接与 Host 通信
- 沙箱配置：资源限制、超时、API 白名单

#### 3.4.3 三档授权模型

```
┌────────────────────────────────────────────────────────────┐
│  self (最低信任)                                            │
│  • 身份从 webview.label() 自动解析                         │
│  • 忽略前端传入的身份参数（防伪造）                         │
│  • 适用于：插件自身的数据读写                               │
├────────────────────────────────────────────────────────────┤
│  scoped-read (中等信任)                                     │
│  • 结果过滤：只返回插件有权访问的数据                       │
│  • 适用于：跨插件数据查询                                   │
├────────────────────────────────────────────────────────────┤
│  privileged (最高信任)                                      │
│  • 显式授予，需要用户审批                                   │
│  • 适用于：系统级操作（如文件系统、网络）                   │
├────────────────────────────────────────────────────────────┤
│  禁止授予清单 (Hard Deny)                                   │
│  • 如 shell:allow-execute 在授予路径上硬拒                  │
│  • 即使 privileged 档也无法绕过                             │
└────────────────────────────────────────────────────────────┘
```

#### 3.4.4 事件总线三通道语义分离

| 通道类型 | 语义 | 丢弃策略 | 可靠性 |
|----------|------|---------|--------|
| **Event** | 通知类事件 | 可丢（丢最旧） | 低 |
| **Request** | 请求-响应 | 不可丢（硬失败） | 高 |
| **State** | 状态快照 | 替换（最新覆盖） | 中 |

每插件独立有界队列，背压可配置，命名空间隔离。

#### 3.4.5 崩溃恢复三级降级

```
Normal 模式
  │ 启动失败 2 次
  ▼
Safemode (只加载必需插件)
  │ 再失败
  ▼
Repairmode (零插件最小壳)
```

- 堵住"必需插件崩溃→永久 boot loop"的经典问题
- 试验性启用（`--enable-plugin`）有独立的失败计数
- 所有恢复动作幂等
- **线上接线**：持久化标记在宿主入口自动打开（`app_config_dir`），崩溃判定
  由 `RecoveryStore::load` 一次完成（载入 + 判定 + 写 in-flight 标记，调用方
  无法跳过）；驱动信号 `host_recover_report(outcome, pluginId?)`——应用每轮
  启动成功必须上报一次，否则每次重启都被计为一次崩溃（方向安全：一次
  `success` 即自愈）。判定器（`BootPhase`）与执行器（注册表状态机的
  `disabledBySafemode`）由 `reconcile_recovery_phase` 对账桥接，前端只读后者。

#### 3.4.6 Schema 管线 (RJSF 兼容性)

```
原始 Schema
  │ $ref 本地展开
  ▼
展开后 Schema
  │ enum 内部标签化
  ▼
标签化 Schema
  │ x-tauron → uiSchema 转换
  ▼
RJSF 兼容 Schema + uiSchema
  │ 门禁检查（禁止嵌套 oneOf/anyOf、未展开 $ref）
  ▼
✅ 通过 / ❌ 拒绝
```

#### 3.4.7 Shell 矩阵

| 形态 | 场景 | 说明 |
|------|------|------|
| `local` | 本地文件加载 | 从本地路径加载插件 HTML |
| `local-server` | 本地 HTTP 服务 | 启动本地 HTTP 服务加载插件 |
| `remote-url` | 远程加载 | 从 CDN/远程 URL 加载插件 |
| `sub-webview` | 子窗口隔离 | 独立 webview 加载，带 devtools |

### 3.5 模块间依赖关系

```
┌─── 应用层 ────────────────────────────────────────────────────┐
│  @tauron/core ──────────────→ @tauron/types              │
│  @tauron/framework ─────────→ @tauron/core          │
│  @tauron/ui ────────────────→ @tauron/core, Lit     │
│  @tauron/cli ───────────────→ @tauron/core          │
│  @tauron/plugin-sdk ────────→ @tauron/core          │
└───────────────────────────────────────────────────────────────┘

┌─── 框架层 ────────────────────────────────────────────────────┐
│  @tauron/core ───────────────────→ @tauron/types              │
│  @tauron/adapter-* ──────────────→ @tauron/core, @tauron/types│
│  @tauron/plugin-sdk ─────────────→ @tauron/types              │
│  @tauron/dual-world ─────────────→ @tauron/types              │
└───────────────────────────────────────────────────────────────┘

┌─── Rust 层 ───────────────────────────────────────────────────┐
│  tauron-adapter ────────────→ tauron-host           │
│  tauron-acl ────────────────→ tauron-host           │
│  tauron-settings ───────────→ tauron-schema         │
│  tauron-shell ───────────────────→ (独立，不依赖其他 crate)    │
│  tauron-host ───────────────→ (独立)                     │
└───────────────────────────────────────────────────────────────┘
```

**关键设计决策：**
- `tauron-host` 和 `tauron-shell` 是两个平行的宿主核心，面向不同框架
- `tauron-adapter` 是唯一依赖 `tauron-host` 的 crate
- Tauri 绑定通过 feature-gated 条件编译隔离，核心可独立测试

---

## 四、测试体系

| 层级 | 测试数量（2026-09-25 实测） | 工具 |
|------|---------|------|
| Rust 单元测试 | **1251 tests**（`cargo test --workspace --lib --tests` 聚合，15 个 crate 全部 0 failed；feature 门控另计：adapter+tauri 222+1 doc-test / shell 71。源码 `#[test]` **声明数** 1284 是另一个口径，见 §四脚注） | `cargo test --workspace` |
| TypeScript 单元测试 | **1626 tests**（98 个测试文件，2026-09-25 更新；轮 11 时 1571） | `vitest`（`pnpm -r test`） |
| 契约测试 | TS↔Rust 跨语言，wire-gate **110 条门禁** | `@tauron/contract-tests` + `@tauron/contract-kit` |
| 属性测试 | — | `proptest` |
| 性能基准 | — | `criterion` |

> 上表数字是 `cargo test --workspace --locked --lib --tests` / `pnpm -r test` 的**实测
> 聚合值**（口径：所有 suite 的 `passed` 之和，failed 必须为 0；实测 15 个 suite /
> **1251 passed / 0 failed**）。**另有一个不同的口径**：源码里 `#[test]` 的**声明数**
> 是 **1284**——它包含 feature 门控（`tauri`）下才编译的用例，因此大于执行数；
> README 的「测试」表给的是声明数，两处不要混读。feature 门控的额外套件单列：
> `cargo test -p tauron-adapter --features tauri` = 175、`-p tauron-shell --features tauri` = 71。

---

## 五、集成指南摘要

### 5.1 第三方快速集成

```bash
# 1. 安装依赖
pnpm add @tauron/types @tauron/core

# 2. 初始化运行时
import { createTauriBackend, invokePlugin } from '@tauron/core';
const backend = createTauriBackend();

# 3. Rust 侧一行接入
tauri::Builder::default().plugin(tauron_adapter::tauri::init())
```

### 5.2 集成成熟度评估

| 维度 | 状态 | 说明 |
|------|------|------|
| API 稳定性 | ⚠️ | `@tauron/*` 包尚无 semver 承诺 |
| 文档完整性 | ❌ | docs/ 目录几乎为空（2026-09-24 复核：**已过时**，`docs/architecture/` 已有多份设计文档与规范） |
| 示例应用 | ⚠️ | 仅 minimal-app 一个示例 |
| 包发布 | ❌ | 未发布到 npm / crates.io |
| CI/CD | ⚠️ | 品牌 CI 矩阵已设计，GitHub Actions 未确认 |
| 版本管理 | ✅ | semver 规范、Cargo.lock 和 pnpm-lock.yaml 均存在 |

---

## 六、已知设计亮点（完整清单 · 带兑现度标记）

> 标记含义与复核方式见 §〇；核对日期 2026-09-24。

1. ✅ **平台无关核心** — 所有 Rust crate 的核心逻辑不依赖 `tauri`，可离线测试。Tauri 绑定通过 feature-gated 适配层实现（`crates/tauron-adapter/src/tauri.rs:29` 的 `#![cfg(feature = "tauri")]`；`tauron-shell` / `tauron-host` 的 `tauri` 均为 optional 且不在默认 feature 内）
2. ✅ **三档授权模型** — self(身份从 webview label 解析，忽略入参) / scoped-read(结果过滤) / privileged(显式授予)（`tauron_host::authz::resolve_principal`，`crates/tauron-adapter/src/tauri.rs:684,716,723`）
3. ✅ **状态机单写者原则** — `plugin.state` 只能通过 `lifecycle.rs` 的 `transition()` 修改，杜绝并发写入（`crates/tauron-host/src/lifecycle.rs:528`）
4. ✅ **事件总线三通道语义分离** — Event(可丢) / Request(可靠) / State(快照替换)（`crates/tauron-host/src/eventbus.rs` 按 `(subscriber, ChannelKind)` 分队列；`host_events_drain` 的 `kind` 为线格式字段）
5. 🟡 **Schema 管线解决 RJSF 兼容性** — `$ref` 展开 + enum 标签化，绕过 RJSF 的嵌套 oneOf/anyOf 限制（`crates/tauron-schema` 131 个测试；但未进 `crates/tauron-adapter/Cargo.toml` 依赖表，**无命令面**——仅被同样未接线的 `tauron-settings` 消费）
6. ✅ **设置中心的 null 语义隔离** — 独立 `$unset` 列表表达"继承"，不与 JSON Merge Patch 的 null 删键混淆（R7 收口：`SubstrateState.settings` 就是 `tauron-settings::SettingsStore`，`host_settings_get/set` 走 `Store`，`tauron-settings` 已在依赖表内；crate 自身 84+ 测试通过）。⚠️ 增量代价：v1→v2 的键编码契约切换需要宿主显式承接旧文档（`host_settings_adopt_legacy`）再迁移（`host_settings_migrate`），**不做隐式改写**；且 Store 无磁盘持久化
7. ✅ **崩溃恢复三级降级** — Normal→Safemode→Repairmode，堵住"必需插件崩溃→永久 boot loop"（`tauron-recovery` 已进依赖表；`host_recover_boot/report/trial_enable` + `reconcile_recovery_phase`）。⚠️ 进程插件的**崩溃检测是轮询式**的：没有后台监控线程，不被调用的 `host_runtime_health` 不会发现死亡（`crates/tauron-adapter/src/lib.rs` 的 `cmd_runtime_health` 文档注释）
8. ✅ **环形裁剪通知存储** — 满时丢最旧而非阻塞写入，未读计数精确追踪（`host_notify` → `tauron_notify::NotifyStore`；读端 `host_notifications_list` 回 `unread/total/items/dispatchLog`）。✅ **派发链路已接线**（R7 收口：`cmd_notify` 走 `tauron_notify::dispatch`，**顺序固定 push → send → log**，失败降级 `Degraded` 且通知不丢；`impl tauri_notify::DispatchSink for TauriDispatchSink` 存在且被注入，`dispatchLog` 为线字段）。⚠️ **但真实系统通知气泡仍未实现**：依赖闭包里没有 `tauri-plugin-notification`，Tauri 实现改走 `app.emit("tauron://notification")` + `request_user_attention` 后**如实返回 `Ok(false)`=降级**，因此生产路径上 `DispatchOutcome::System` 不可达（只有 mock sink 覆盖）——这条按"参考实现"计
9. 🟡 **HMAC-SHA256 授予签名** — 权限授予集落盘防篡改（`crates/tauron-acl/src/grant.rs:107` 有实现与 42 个测试；`tauron-acl` **未进适配层依赖表**，授予/审批/物化为 Tauri Capability 的链路未接线）
10. 🟡 **RFC 8785 规范化签名验证** — 商城插件签名验证（`crates/tauron-market/src/lib.rs:53` 的 `canonical_json` + 63 个测试；适配层 `host_market_*` 是桩，见第 11 条）
11. 🟡 **安全解压四重防护** — 路径清洗、条目≤2000、解压≤200MB、压缩比≤100×（`crates/tauron-market` 的 `validate_zip_constants` 有正反测试；**只做校验、不做解压**——crate 自述"zip 解包由适配层提供"，`crates/tauron-market/src/lib.rs:11`，而适配层没有解压实现）
12. 🟡 **链式 hash 审计日志** — 商城操作不可篡改（`crates/tauron-market` 的 `AuditLog::verify_chain` 有篡改检测测试；未接线）
13. ✅ **generate_handler!() 宏** — 一次性注册全部 Tauri 命令，零样板。⚠️ 原文写的 **17 条已过时**：实测为 **50 条**（`tauron_plugin_handler!` = 底座 35 + 插件运行时 15），底座-only 宿主用 `tauron_substrate_handler!` 只注册 **35 条**（`crates/tauron-adapter/src/tauri.rs:1279,1333`，计数见 `docs/integration/incremental-adoption.md`）
14. 🟡 **Shell 矩阵 4 形态** — local / local-server / remote-url / sub-webview 覆盖主流场景（`packages/tauron-shell-matrix/src/manager.ts:70-88` 四条分支均为 `Simulate ...` 注释下的模拟返回，自带测试但未接真实 webview/本地服务）
15. ✅ **双层 ACL** — 外层 Tauri 静态（capability/permission，由接入方在 `capabilities/` 声明）+ 内层框架动态（三档授权 + origin 允许清单，`origin_gate` 是唯一分发咽喉点，`crates/tauron-adapter/src/tauri.rs:1243`）
16. ✅ **per-plugin 崩溃重启限制** — 进程插件 3 次 / 5 分钟，防止雪崩（`tauron-proc::CrashTracker`，在 `cmd_runtime_spawn` 的预算门里真被调用，`crates/tauron-adapter/src/lib.rs`）。⚠️ **无进程组 / 作业对象、无 kill 树**（孙进程不随父进程一起死），空闲超时 kill 未实现（`crates/tauron-proc/src/spawner.rs:93`）
17. 🟡 **ABI 指纹校验** — 进程插件和 WASM 插件均做 ABI 版本兼容检查（进程侧已接线：`validate_spawn_config` 在 spawn 前真调用；WASM 侧只有 `tauron-wasm` 库内的 `validate_abi`，而该 crate **不在依赖表里**、且无 WASM 运行时）
18. ✅ **跨语言契约测试** — TS↔Rust 协议验证，极少有框架做到（`@tauron/contract-tests`：`contract.test.ts` 比对命令名/camelCase 线格式/错误码，`wire-gate.test.ts` 解析两侧源码逐调用点比对参数形状）
19. ✅ **插件 i18n 命名空间** — `plugin:<id>.oc.<key>` 隔离插件文案（`host_i18n_load` 在传 `pluginId` 时自动加前缀，6 条 i18n 命令实调 `tauron_i18n`）
20. 🟡 **品牌唯一性校验** — 多品牌间标识冲突自动检测（`crates/tauron-brand` 55 个测试；未进依赖表，运行期 `host_brand_info` 返回 `{}` 桩，`crates/tauron-adapter/src/lib.rs` 的 `cmd_brand_info`）

---

## 七、已知问题与风险

| # | 问题 | 严重度 | 说明 |
|---|------|--------|------|
| 1 | **两代包命名并存** | 🔴 高 | `@tauron/*` 和 `@tauron/*` 双轨增加集成成本，职责有重叠（两个 core、两个 plugin-sdk、两个 cli） |
| 2 | **架构文档缺失** | 🔴 高 | `docs/architecture/` 和 `docs/competitive-analysis/` 为空，设计决策全靠代码注释（2026-09-24 复核：**已过时**，两个目录现均有文档；本条的其余风险见第 1、3 条） |
| 3 | **tauron-shell vs tauron-host 重复** | 🟡 中 | 两个 Rust crate 都实现注册表/事件总线/ACL，但面向不同框架 |
| 4 | **部分模块实现深度不一** | 🟡 中 | proc/wasm 的运行时逻辑依赖 stub |
| 5 | **Tauri 适配层硬编码** | 🟡 中 | `tauri.rs` 中 `host_events_publish` 通过 `strip_prefix("plugin-")` 解析 publisher，缺乏防御性 |
| 6 | **未发布到公共注册表** | 🟡 中 | npm/crates.io 均未发布，第三方无法直接 `cargo add` / `pnpm add` |
| 7 | **示例应用不足** | 🟢 低 | 仅 minimal-app，缺少展示完整能力的 demo |

---

## 八、下一步建议

### 短期（1 个月内）— 补基础
1. **统一包命名** — 合并 `@tauron/*` 和 `@tauron/*` 两套命名空间
2. **补充架构文档** — 基于本文档生成 docs/architecture/ 目录下的详细设计文档
3. **发布到 npm/crates.io** — 至少发布 `@tauron/types` + `@tauron/core` + `tauron-shell`

### 中期（3 个月内）— 补功能
4. **深化 WASM 运行时** — 参考 Extism/Wasmtime 实现真正的生产级沙箱
5. **补充示例应用** — 至少 3 个：纯 Rust 插件示例 + WASM 插件示例 + 多品牌白标示例
6. **CLI 工具链完善** — `tauron doctor` 诊断、`tauron plugin dev` 热重载

### 长期（6 个月+）— 建生态
7. **插件商城上线** — 公共注册表 + 签名验证 + 社区贡献流程
8. **VSCode 插件迁移工具** — 降低从 VSCode Extension 生态迁移的成本
9. **性能基准测试** — 插件启动时间、内存占用、并发调用吞吐量的量化指标

---

> 本文档由 WPS Comate 竞品分析工具自动生成，基于项目源码和竞品搜索结果。建议定期更新。

---

> **兑现度标记核对日期 = 2026-09-24**（本改动日期）。
> 标记只对当日仓库状态成立；三档口径与复核命令见 §〇。复核入口：
> `crates/tauron-adapter/Cargo.toml`（接线与否）、`crates/tauron-adapter/src/lib.rs`
> 的 `cmd_*` 实现体（桩与否）、`docs/architecture/overview.md`「接线状态（诚实披露）」。
> 本次核对中**修正的一处事实**：第六章第 13 条的命令数不是 17，而是 **50**（底座 35 + 插件运行时 15）。
> 本次核对**未验证**（故未加标记）：第五章的集成成熟度评估——需要逐项端到端演练才能定论。
> 第四章的测试总数**已在轮 11 补测**（Rust 1221 / TS 1571 / wire-gate 125，聚合口径见该章脚注）。
> 另：核对期间 `crates/tauron-adapter` 曾被另一条工作流并发修改（R7 接线），
> 该工作流现已落地并全绿：「配置四层合并」改判 ✅（含"Store 无磁盘持久化"边界）、
> 「系统通知派发」改为"派发链路 ✅ 已接线 / 真实 OS 气泡仍未实现（依赖闭包无
> `tauri-plugin-notification`，`System` 结果生产不可达）"，详见对应条目括注。