# tauron 架构概览

> 品牌统一为 **tauron**：全部 npm 包为 `@tauron/*`，全部 Rust crate 为 `tauron-*`。

## 整体架构图

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        应用层 (Application)                               │
│    React App / Vue App / Svelte App / 原生 TS App                        │
├──────────────────────────────────────────────────────────────────────────┤
│                    UI 框架适配层 (@tauron/adapter-*)                       │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐                      │
│  │ adapter-react│ │ adapter-vue  │ │adapter-svelte│                      │
│  │ useInvoke    │ │ useInvoke    │ │ createStore  │                      │
│  │ useEvent     │ │ useEvent     │ │ events       │                      │
│  │ useCaps      │ │ useCaps      │ │ capabilities │                      │
│  └──────────────┘ └──────────────┘ └──────────────┘                      │
├──────────────────────────────────────────────────────────────────────────┤
│                    框架核心层 (@tauron/core)                               │
│  invokePlugin / createTauriBackend / EventBus / ACL / Registry / Config  │
├──────────────────────────────────────────────────────────────────────────┤
│                    类型定义层 (@tauron/types)                              │
│  Envelope / ErrorCode / PluginState / Manifest / Events / ACL            │
├──────────────────────────────────────────────────────────────────────────┤
│                  Tauri 命令适配层（薄包装，feature = "tauri"）              │
│   tauron-shell: plugin_invoke / plugin_cancel / plugin_emit (3 条信封命令) │
│   tauron-adapter: host_* 命令族（应用层，54 条 = 底座 38 + 插件运行时 16）  │
├──────────────────────────────────────────────────────────────────────────┤
│          Rust 壳层                        TS 插件 SDK 层                   │
│  ┌──────────────────────────┐      ┌──────────────────────────────────┐  │
│  │  tauron-shell            │      │  @tauron/plugin-sdk              │  │
│  │  envelope / dispatch     │      │  PluginBridge (postMessage)      │  │
│  │  acl / registry / eventbus│     │  createPluginContext             │  │
│  │  config                  │      └──────────────────────────────────┘  │
│  └──────────────────────────┘      ┌──────────────────────────────────┐  │
│  ┌──────────────────────────┐      │  @tauron/dual-world              │  │
│  │  tauron-host             │      │  QuickJS-WASM 沙箱               │  │
│  │  manifest / lifecycle    │      │  JS↔WASM 桥接                   │  │
│  │  registry / authz        │      └──────────────────────────────────┘  │
│  │  eventbus / config       │                                            │
│  └──────────────────────────┘                                            │
├──────────────────────────────────────────────────────────────────────────┤
│                      插件运行时 (Plugin Runtimes)                         │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────────┐│
│  │  tauron-proc │ │ tauron-wasm  │ │ Rust Native  │ │  B+ 混合模式     ││
│  │  进程插件    │ │ WASM 插件    │ │ Plugin       │ │  JS + Host 双世界││
│  │  sidecar     │ │ Extism       │ │ 原生集成     │ │  隔离            ││
│  │  JSON-RPC    │ │ QuickJS-WASM │ │              │ │                  ││
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

## 两层架构

tauron 由**框架层**与**应用层**组成。两层共享同一套类型与协议，但职责不同：

| | 框架层 | 应用层 |
|---|---|---|
| **面向** | 第三方客户端集成 | 完整客户端交付 |
| **npm** | `@tauron/types` `core` `plugin-sdk` `dual-world` `adapter-*` `market` `shell-matrix` `cli` `contract-tests` | `@tauron/host` `framework` `ui` `app-cli` `app-plugin-sdk` `app-contract-kit` |
| **Rust** | `tauron-shell` | `tauron-host` `tauron-adapter` |
| **命令族** | `plugin_invoke` / `plugin_cancel` / `plugin_emit` | `host_*`（54 条） |
| **入口** | `tauron_shell::commands::init()` 或零配置 `state_init()` + `tauron_generate_handler![]` | `tauron_adapter::tauri::init()` 或 `state_init()` + `tauron_generate_handler![]` |

> 应用层**复用**框架层的类型与协议，不修改框架层契约。两层之间的命令名与
> 线格式由 `@tauron/contract-tests` 的跨语言门禁锁定；应用层 `host_*` 命令族的
> **参数形状/返回值/生命周期事件/能力档位**规范见
> [应用层线格式协议](./app-layer-wire.md)。

> **注册形态与 ACL**：`init()` 插件形态的命令经 `plugin:<插件名>|<命令>` 路由，
> Tauri v2 对 `plugin:` 命令**强制 capability/ACL**（未授予即被运行时拒绝）；
> `state_init()` + `tauron_generate_handler![]` 的 root 形态以裸命令名注册
> （前端 `TauriBackend` 传 `commandPrefix: ''`，框架层 core `createTauriBackend()`
> 本就是裸名）。两种形态二选一，同时使用会重复 `manage::<State>` 而 panic。
> 两层插件名已区分（`tauron-shell` / `tauron`），可叠加注册。
>
> ⚠️ **root 形态也不是"零配置"**（轮 12 改判，此前文档这么写是错的）：
> Tauri v2 的规则是**不匹配任何 capability 的 webview 完全没有 IPC 访问**
> （原文见 `tauri-build` 生成的 `gen/schemas/desktop-schema.json` 的 `Capability`
> 说明）。所以任何形态都必须至少有一份 capability 文件；示例已随仓提供
> `examples/minimal-app/src-tauri/capabilities/default.json`（授予 `core:default`，
> `windows: ["main", "plugin-*"]`——`plugin-*` 不能省：`host_window_create` 铸出的
> 插件面板窗 label 恒为 `plugin-<id>`，漏掉它插件界面就是一片空白）。
> 插件形态**额外**需要 `tauron` 插件自己的权限条目，而 `crates/tauron-adapter`
> 目前**没有** `permissions/` 定义 → `plugin:tauron|*` 在启用能力检查的宿主上缺权限，
> 这是已登记的部署配置缺口（见 app-layer-wire.md §1 告警）。

## 包命名体系

```
@tauron/*      ← 全部 npm 包（框架层 + 应用层）
tauron-*       ← 全部 Rust crate
```

### 框架层 (@tauron/*)

| 包 | 职责 | 关键导出 |
|---|------|---------|
| **@tauron/types** | 共享类型 | `PluginInvokeRequest/Response`, `PluginErrorCode`, `PluginState`, `PluginManifest`, `PluginEvent`, `TauronConfig`, `PluginPermissionGrant` |
| **@tauron/core** | 平台无关核心 | `invokePlugin()`, `createTauriBackend()`, `TAURON_COMMANDS`, `EventBus`, `PluginRegistry`, `ConfigManager`, ACL |
| **@tauron/plugin-sdk** | 插件 SDK | `PluginBridge`（宿主侧）、`createPluginContext()`（插件侧） |
| **@tauron/dual-world** | 双世界沙箱 | `createBridge()`, `createSandbox()`, QuickJS-WASM 隔离 |
| **@tauron/adapter-react** | React 适配 | `TauronProvider`, `useInvoke`, `useEvent`, `useCapabilities` |
| **@tauron/adapter-vue** | Vue 适配 | `useInvoke`, `useEvent`, `useCapabilities`（composables） |
| **@tauron/adapter-svelte** | Svelte 适配 | `createInvokeStore`, `createEventStore`, stores |
| **@tauron/market** | 插件市场 | HMAC-SHA256 签名/验签、注册表 search/register/remove（独立基础设施库，供市场服务集成方使用；应用内商城入口未接线） |
| **@tauron/shell-matrix** | Shell 矩阵 | `createShellManager()`，4 形态：local / local-server / remote-url / sub-webview |
| **@tauron/cli** | CLI 工具（bin: `tauron`） | `doctor` / `create` / `plugin new\|dev\|test\|pack\|sign\|publish` |
| **@tauron/contract-tests** | 跨语言契约 | TS↔Rust 命令名、camelCase 线格式、错误码门禁 |

### 应用层 (@tauron/*)

| 包 | 职责 | 关键导出 |
|---|------|---------|
| **@tauron/host** | 能力编排层 | `HostClient`, `TauriBackend`, `Backend`, `capabilities`, `channels`, `gates` |
| **@tauron/shell-events** | 壳层事件契约（零依赖） | `SHELL_EVENTS` 事件名常量 + detail 类型（组件与控制器唯一事实源） |
| **@tauron/ui-primitives** | UI 原语（零宿主依赖） | Lit Web Components + design tokens + 动效 + 退出/组件动画 |
| **@tauron/framework** | 框架薄封装 | signals 绑定、WC 包装抽象、三框架统一导出 |
| **@tauron/ui** | UI 便利包 | 转出 `@tauron/ui-primitives` + `PluginManagerStore`（需宿主命令面） |
| **@tauron/app-cli** | 应用脚手架（bin: `tauron-app`） | `init` / `doctor` / `client config` / `theme generate` / `plugin *` / `codegen` |
| **@tauron/app-plugin-sdk** | 应用级插件 SDK | 生命周期、Contributes 注册 |
| **@tauron/app-contract-kit** | 应用契约套件 | core API 行为 / 事件协议 / 错误码 + mock Backend |

## Rust crate 清单

| Crate | 层 | 职责 |
|---|---|---|
| **tauron-shell** | 框架 | 信封、`HostState` 命令核心、ACL、注册表、事件总线、配置；`tauri` feature 提供命令层 |
| **tauron-host** | 应用 | 宿主核心：清单、生命周期、注册表、授权、事件总线、配置 |
| **tauron-adapter** | 应用 | Tauri 命令适配（`host_*`），`tauri` feature |
| **tauron-acl** | 应用 | 权限授予与审批 |
| **tauron-schema** | 应用 | Schema 规范化管线 |
| **tauron-settings** | 应用 | 设置中心（四层合并） |
| **tauron-market** | 应用 | 插件商城 |
| **tauron-brand** | 横切 | 白标品牌化 |
| **tauron-theme** | 横切 | 主题 / 皮肤 |
| **tauron-i18n** | 横切 | 国际化 |
| **tauron-notify** | 横切 | 通知中心 |
| **tauron-recovery** | 横切 | 崩溃恢复 |
| **tauron-distribute** | 横切 | CI 分发运维 |
| **tauron-proc** | 运行时 | 进程插件 Host（JSON-RPC sidecar） |
| **tauron-wasm** | 运行时 | WASM Supervisor（Extism / QuickJS） |

> **接线状态（诚实披露）**：上表是**架构组件全景**，不代表运行时已装配。
> 以 `crates/tauron-adapter/Cargo.toml` 的依赖表为准（**没进该表的 crate，
> 其对应 `host_*` 命令体一定是桩**）：
>
> | 状态 | crate | 证据 |
> |---|---|---|
> | ✅ **已接线** | `tauron-host` / `tauron-i18n` / `tauron-notify` / `tauron-recovery` | 适配层直接依赖并调用其引擎 |
> | ✅ **已接线** | `tauron-settings` | 设置走 `SettingsStore`（R7-2 激活孤儿 crate），不是适配层内建裸 KV；`host_settings_*` 的 schema 校验与版本迁移都在它里面 |
> | ✅ **已接线** | `tauron-proc` | 进程插件用 `CommandSpawner` / `CrashTracker` / `validate_spawn_config`（P0-2 激活孤儿 crate）；崩溃窗口计数**唯一**来源是 `tauron_proc::CrashTracker` |
> | ⚠️ **未接线** | `tauron-market` / `tauron-brand` | `host_market_*` / `host_brand_info` 为内联桩，恒返回可消费的空形状 |
> | ⚠️ **未接线** | `tauron-theme` / `tauron-wasm` / `tauron-distribute` | **独立组件库**——自带全绿测试，集成点已定义但尚未接入适配层运行时 |
>
> 未接线的部分接入时只需替换对应命令体 / 新增命令，**不需要改动线协议**。

## 模块间依赖关系

```
┌─── 框架层 ─────────────────────────────────────────────────────┐
│  @tauron/core ────────────────→ @tauron/types                  │
│  @tauron/adapter-* ───────────→ @tauron/core, @tauron/types    │
│  @tauron/plugin-sdk ──────────→ @tauron/types                  │
│  @tauron/dual-world ──────────→ @tauron/types                  │
│  @tauron/contract-tests ──────→ @tauron/core, @tauron/types    │
└────────────────────────────────────────────────────────────────┘

┌─── 应用层（R3 后：底座可取原语而不拖宿主）────────────────────┐
│  @tauron/shell-events ────────→ (zero-dep)                     │
│  @tauron/ui-primitives ───────→ @tauron/shell-events, Lit      │
│  @tauron/host ────────────────→ @tauron/shell-events           │
│  @tauron/framework ───────────→ @tauron/host                   │
│  @tauron/ui ──────────────────→ @tauron/ui-primitives + host   │
│  @tauron/app-cli ─────────────→ @tauron/host                   │
│  @tauron/app-plugin-sdk ──────→ @tauron/host                   │
│  @tauron/app-contract-kit ────→ @tauron/host, @tauron/framework│
└────────────────────────────────────────────────────────────────┘

依赖方向（R3 强约束，单向无环）：
- `@tauron/ui-primitives` **不得**依赖 `@tauron/host`——哑组件只吃属性、只发
  `oc-*` 事件（事件名取自 `@tauron/shell-events`）。任意桌面宿主可只取原语。
- `@tauron/host` **不得静态**引入任何 UI 包（保持入口 DOM / `lit` 无关，
  node 测试环境可用）；`bootstrap` 只以动态 import 可选获取 UI 能力。
- `@tauron/ui` 是便利包：转出原语 + 提供需要宿主命令面的 `PluginManagerStore`，
  因此它可以依赖两者（方向 ui → host 单向）。

┌─── Rust ───────────────────────────────────────────────────────┐
│  tauron-shell ────────────────→ (独立，默认不依赖 tauri)        │
│  tauron-host ─────────────────→ (独立)                         │
│  tauron-adapter ──────────────→ tauron-host, tauron-i18n,      │
│                                 tauron-notify, tauron-recovery,│
│                                 tauron-settings, tauron-proc   │
│  tauron-acl ──────────────────→ tauron-host                    │
│  tauron-settings ─────────────→ tauron-schema                  │
└────────────────────────────────────────────────────────────────┘

单向约束：`SubstrateState`（底座）**不得**触达注册表；插件运行时 → 底座的
依赖成立，反向不成立。`tauron-adapter` 的依赖表就是这条约束的现场证据——
它列出的全是底座侧 crate。
```

## 关键设计决策

1. **平台无关核心** — 所有 Rust crate 的默认构建不依赖 `tauri`，核心逻辑可离线测试；
   Tauri 绑定通过 feature-gated 薄适配层实现。
2. **单命令信封协议** — 前端只通过 `plugin_invoke` 一个命令发起插件调用，
   由 `tauron_shell::HostState::handle_invoke()` 统一做信封校验 → 注册表状态检查 → 分发。
3. **三档授权模型** — self（身份从 webview label 解析，忽略入参）/ scoped-read（结果过滤）/ privileged（显式授予）。
4. **状态机单写者原则** — `PluginState` 只能通过 `registry.transition()` 修改，非法迁移被拒绝。
5. **事件总线背压** — 每插件独立队列，溢出丢弃最旧（`EventBus` 与 dual-world `Bridge` 同策略）。
   事件到前端有**两条独立通路**，缺一不可：
   - **框架层**：`plugin_emit` → Rust 内部总线 + `EventSink` 出站投递（Tauri 事件系统），
     前端 `listen("plugin:<id>:<event>")` 实时接收；
   - **应用层**：`host_events_publish` → 每订阅者队列（Request 通道），
     前端 `eventsSubscribe()` 后必须调用 `eventsDrain()` 取件——订阅只入队，不取件等于没订阅。
6. **跨语言线格式统一为 camelCase** — Rust 侧所有跨边界结构体均标注
   `#[serde(rename_all = "camelCase", deny_unknown_fields)]`，与 TS 类型逐字段对齐。
7. **跨语言契约门禁** — `@tauron/contract-tests` 读取真实 Rust 源码断言命令名、
   字段命名与错误码同构，任何一侧漂移即 CI 失败。门禁族包括：命令名与注册完整性
   （每个 `#[tauri::command]` 必须在 `generate_handler!` 里——未注册 = 前端
   `command not found`）、参数形状与必填项、返回值字段、错误码线名与可重试集合、
   生命周期状态机、能力档位、进度通道与出站事件载荷（`Event` ↔ `PluginEvent`）。
   **门禁必须剥注释后再匹配**，否则把注册项注释掉会假绿。
8. **进度通道由宿主真实投递** — `plugin_invoke` 接受可选
   `Option<JavaScriptChannelId>`（`Option<Channel<T>>` 无法编译：`Channel` 只实现
   `CommandArg`，未实现 `Deserialize`），在分发前后各投一帧**调用级**进度
   （`started` / `completed|failed`）。`PluginDispatcher::dispatch` 是同步
   请求/响应、不产出细粒度步骤，故不伪造中间进度；不传通道则一帧不发（零开销）。
9. **资源回收由装配方负责** — 订阅/反向索引/队列/pending 均以插件 id 为键：
   卸载/清除由适配器自动回收（`host_registry_admin` → `dispose_*`）；
   **窗口销毁必须由宿主**在 App Builder 的 `on_window_event(Destroyed)` 中调用
   `tauron_adapter::tauri::cleanup_closed_window`（`tauri::plugin::Builder` 没有
   窗口事件钩子，只有 App Builder 有），不接线即泄漏。
10. **事件总线锁序**（**应用层** `crates/tauron-host/src/eventbus.rs`，非框架层
    shell 总线）— `stats → topics → subs → topic_subscribers → approvals →
    queues`，任何路径不得反序持有；由源码级断言锁定（`subscribe` 必须先
    `subs` 后 `topic_subscribers`，否则与 `publish` 的悬挂清理构成死锁环）。
    审批记录随 `dispose_subscriber` 一并回收。
11. **取消与超时语义** — `plugin_cancel` 是真委派（`dispatcher.cancel(call_id)`
    → `bool`）；JS 侧超时到点后**best-effort 补发一次取消**，避免调用方已放弃
    而宿主仍在执行到 TTL GC。取消失败只吞不抛（可能未装载 dispatcher / pending
    已被回收），不放大成调用失败。未装载 dispatcher 时 `plugin_invoke` 返回
    `SC-9001`——这是库的契约，不是缺陷。
12. **错误码分两层且零交集** — 框架层 `SC-xxxx`（`tauron-shell` ↔
    `@tauron/types`）、应用层 `E_*`（`tauron-host` ↔ `@tauron/host`），各自
    **层内**同构，跨层不统一。TS 侧遇到表外码收窄为 `E_UNKNOWN`，但
    **原串保留在 `rawCode`**（版本偏斜时遥测不失明）。
13. **安全模式只有一个权威** — `PluginSummary.disabledBySafemode` 读的是
    **注册表状态机**（`crates/tauron-host/src/lifecycle.rs`：`Event::SafemodeEnter`
    → `Action::SetSafemodeDisabled`，7 条规则可达，`SafemodeExit`/`TrialEnable`
    退出）。`tauron-recovery` crate 的 `BootPhase` 是**判定器**，注册表状态机是
    **执行器**，两者由 `reconcile_recovery_phase` 对账桥接（只补发事件，不改状态，
    可重复调用）：判定为安全模式而注册表未标记时补发 `SafemodeEnter`，反之补发
    `SafemodeExit`。持久化 + 崩溃检测在 Tauri 宿主入口自动打开
    （`app_config_dir`，`RecoveryStore::load` 内完成「载入 + 崩溃判定 + 写入
    in-flight 标记」三步）；**驱动信号是 `host_recover_report`**——应用每轮启动
    成功必须上报一次，否则每次重启都被计为一次崩溃，连续两次进安全模式
    （方向安全：一次 `success` 即自愈）。安全模式内逐个试启用
    `host_recover_trial_enable`，试验失败 1 次即回落 `disabled-by-safemode`。
14. **运行期订阅审批尚未接线** — `EventBus::approve` 无任何线上入口
    （适配层无命令、`@tauron/host` 无方法），`approvals` 表在生产中恒空，
    `is_approved` 分支不可达；跨插件订阅私有 topic 目前只能靠声明方标
    `public: true`。方向是 fail-closed（未授权即 `E_AUTH_DENIED`，不会误放行），
    启用需补一条批准命令 + TS 方法。**安装期**授权是另一套且可达
    （`@tauron/host` 的 `grants.ts`），两者不可混谈。
    `grants.ts` 本身是**纯策略函数库**（`requiresReapproval` 等），仓库内没有
    生产消费方——即「重新审批」策略要由接入方在安装/更新路径上显式调用才会
    生效，不调用则不构成门禁。
15. **i18n 引擎已接线，bundle 由应用侧供** — `host_i18n_t` / `t_params` /
    `set_locale` / `load` / `stats` / `cleanup_plugin` 全部落到 `tauron-i18n`
    引擎（语言状态单一来源 + 回退链 + 缺失键计数）。**回退链全部落空时返回
    key 本身**（不是空串）并计入 `missingTotal`；回退命中不计缺失——前端用
    `text === key` 检出缺失文案。`host_i18n_load` 是合并语义（同语言按 key
    合并不整体替换），传 `pluginId` 时自动加 `plugin:<id>.oc.` 前缀；插件
    Uninstall/Purge 自动按命名空间清理。语言代码在适配层做类型化校验
    （引擎的 `set_locale` 不校验），非法值拒绝而非污染回退链。

## 架构演进

tauron 的当前形态是三轮重构的结果。**原始计划文档已清理**（见
[README.md](./README.md) 的「关于已清理的历史文档」），这里保留脉络与结论——
**决策本身才是要继承的东西**，计划书的执行细节不是。

| 轮次 | 做了什么 | 耐久产出落在哪 |
|---|---|---|
| **P0–P3**（2026-01 审计） | 逐项对照**真实源码**审计「宣称 vs 实现」，建立「✅ 具备 / ⚠️ 部分 / ❌ 宣称但缺失 / 🔴 静默缺失」判定标准 | 判定标准成为本仓库文档诚实性约定的来源；能力现状见 [0.3 方案 §1 记分卡](./multi-plugin-substrate-roadmap.md) |
| **R1–R8**（轮 7–12，已完成） | **析取底座**：拆开 god object `CommandState`，让 shell / IPC / i18n / notify / recovery / settings / identity 成为可独立装配的单元；`tauron-shell` 与 `tauron-host` 的重复实现定 canonical；`ui-primitives` 与 `host` 解耦 | [canonical-owners.md](./canonical-owners.md)、[app-layer-wire.md](./app-layer-wire.md)、[incremental-adoption.md](../integration/incremental-adoption.md)、本文件「关键设计决策」 |
| **0.3**（进行中） | **兑现两个产品承诺**：任意宿主底座（今天实际是「任意 **Tauri** 宿主底座」）、多插件框架（今天实际是「**单插件运行时**框架」） | [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md) |

### R1–R8 析取出的底座边界

这条边界是后续一切改动的**前提**，不得回退：

1. 底座命令在**类型上**触达不到注册表（`SubstrateState` 无 registry 字段）。
2. `Plugin Runtime → Substrate` 单向依赖，反向不成立。
3. `@tauron/ui-primitives` 不依赖 `@tauron/host`；`@tauron/host` 不静态引入 UI 包。
4. 全部 `cmd_*` 函数体零直接 Tauri 调用（平台调用一律经 `trait *Sink`）。
5. 身份判定在**代码里**（不只是 ACL 配置里），且判定点在任何副作用之前。
6. 两套错误码不合并，但穿越边界必经 `translate_at_boundary`。

三档装配方式（只取底座 / 底座 + 插件运行时 / 完整客户端）见
[incremental-adoption.md](../integration/incremental-adoption.md)。
