# Tauron：Universal / Cross-Host / Industrial Application Substrate 最终架构方案 V4

> **文档类型**：最终总体架构、Universal Client Contract、Production Profile、Wire/FFI/Target Contract、事务一致性、性能与资源治理、按需安装、第三方集成、工业级可靠性、连续三轮新增审计与实施路线图  
> **建议落点**：`docs/Tauron-Universal-Industrial-Application-Substrate-Final-Architecture-V4.md`  
> **基线仓库**：`coeasy/tauron`  
> **审计基线**：`main@4e5c37f431b66fc5f793d72473f11a524a733ef3`（`release: prepare v1.0.2 SDK`）  
> **融合来源**：`Tauron-Modular-Application-Substrate-Architecture-V1.md` + 后续“第三方稳定基座/未来优化”方案  
> **最终目标**：让 Desktop / Hybrid / Native / Headless / Service / Remote 等第三方系统通过统一契约**快速集成、稳定依赖、按需裁剪、低内存运行、按需下载安装能力包，并具备长期兼容承诺**；“任意客户端”定义为满足 Universal Client Contract、目标平台合同和安全最低线并通过 Conformance 的宿主，而不是承诺所有技术栈零适配。  
> **审查要求**：V2/V3 的既有审计全部保留；V4 再执行 **Round V4-1 通用性/所有权/生产默认 → Round V4-2 端到端并发/事务/安全撤销 → Round V4-3 故障一致性/低内存/供应链/发布证明** 三轮新增审计。每轮发现的问题都必须先补齐唯一 Owner、状态机、终止条件、错误出口、资源回收点和可执行 Gate，才允许进入下一轮。  
> **重要说明**：本文完成的是**V4 架构与实施方案收敛**，不是对当前仓库代码已全部实施完成的声明。2026-09-28 本轮复核时 `main` 仍为 `4e5c37f431b66fc5f793d72473f11a524a733ef3`；官方 v1 支持边界仍以 Tauri 2 为主，`main` 仍未启用保护规则。V4 是后续实施目标，任何未通过真实 E2E / Failure Injection / Conformance / Release Gate 的能力不得标记为 `full`。
>
> **版本关系**：V4 完整继承 V2/V3 已确认的单 Kernel、Universal Contract、Capability fail-closed、RuntimeDriver、Process Tree、Session、Resource Ownership Tree、Settings Watch、按需 Runtime Pack、性能/RSS/Size Gate 等设计；如 V2/V3 与 V4 新增章节存在冲突，以 V4 为准。

---

# 0. 最终结论

Tauron 的最终定位收敛为：

> **Tauron：面向第三方可扩展客户端的模块化应用底座（Modular Application Substrate），提供统一 Host 能力、插件系统、运行时隔离、安全授权、生命周期、配置、恢复、消息通信、更新分发、可观测性与跨框架 SDK；支持按需裁剪、按需下载安装 Runtime/Extension Pack，并以低内存、低安装体积和稳定兼容为一等目标。**

英文定位：

> **Tauron is a host-neutral, language-neutral application substrate for building secure, extensible, resource-efficient desktop, native, headless, service and remote clients with stable third-party integration contracts.**

Tauron 未来不以“命令数量”“crate 数量”“UI 组件数量”作为成熟度指标，而以以下结果作为成熟度指标：

1. 第三方项目是否可以在很少的接线代码下完成集成；
2. 是否只有一套 canonical Kernel / Contract / Lifecycle / Registry；
3. 前端 → SDK → Transport → Kernel → Provider/Runtime → 返回值 → UI 是否完整闭环；
4. 是否不存在有类型无入口、有入口无消费者、有 Runtime 无退出路径的孤儿逻辑；
5. 是否不存在无限重试、无限队列、永久 pending、永久 stream 或锁自死循环；
6. 是否能只打包项目需要的能力；
7. 大型 Runtime 是否支持签名后按需下载安装，而不是全部塞进基础安装包；
8. 空闲状态是否基本零轮询、零无意义后台线程；
9. 是否有 N-1 兼容、Conformance Kit、外部 Consumer CI；
10. 是否有安装包体积、启动耗时、RSS、IPC 延迟与资源泄漏回归门禁。

最终执行纪律：

```text
先收敛协议和所有权
    ↓
再完成第三方接入闭环
    ↓
再完成 Runtime 工业化
    ↓
再做性能/内存/按需能力包
    ↓
最后扩 Marketplace / Remote / 生态
```

---

# 1. 项目目标与非目标

## 1.1 Tauron 必须解决的问题

Tauron 负责第三方复杂客户端反复遇到、且最容易形成技术债的基础设施：

- Host 能力统一抽象；
- Window / Menu / Tray / Dialog / Clipboard / FS / HTTP / Notification / Update；
- Plugin Manifest / Registry / Install / Enable / Disable / Update / Rollback / Uninstall；
- WebView / Process / WASM / Builtin / Remote Runtime；
- IPC / RPC / Event / Stream / Cancel / Deadline / Backpressure；
- Capability / ACL / Permission / Grant / Publisher Trust；
- Settings / Secrets / Schema / Migration / Watch；
- Recovery / Safe Mode / Trial Enable / Crash Budget；
- Runtime Supervisor / Process Tree / Resource Accounting；
- App Update / Plugin Update / Runtime Pack Update；
- Trace / Metric / Log / Audit；
- React / Vue / Svelte / Vanilla TypeScript；
- Third-party Host Adapter / Provider / RuntimeDriver SPI；
- CLI / Scaffold / Codegen / Contract Test / Conformance Kit；
- 按需编译与按需下载；
- 安装包体积、内存、启动性能的持续治理。

## 1.2 Tauron 明确不负责

以下能力由第三方产品自己拥有：

- 业务领域模型；
- 产品路由；
- 用户账号/登录业务；
- 行情、交易、回测；
- Agent / LLM 业务；
- 产品数据库模型；
- 产品后端 API；
- 产品业务工作流；
- 强制 Marketplace；
- 强制 Tauron UI；
- 强制某家云平台；
- 第三方业务插件的内部逻辑。

原则：

> **Tauron 提供机制、契约和治理，不拥有第三方产品策略。**

---

# 2. 最终总体架构

建议最终形成七层，而不是把 Tauri 和 Tauron 混成一层：

```text
┌──────────────────────────────────────────────────────────────────┐
│                       Product Layer                               │
│ Business / Domain / Product UI / Route / Product Workflow        │
└──────────────────────────────┬───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                         Tauron App Kit                            │
│ create app / lifecycle binding / SDK / adapters / optional UI    │
└──────────────────────────────┬───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                      Tauron Contract Plane                        │
│ Wire / Manifest / Capability / Error / Lifecycle / ABI / WIT     │
└──────────────────────────────┬───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                       Tauron Host Kernel                          │
│ Registry / Policy / Settings / Recovery / Message / Resource     │
│ Contributes / Runtime Manager / Capability Registry              │
└──────────────────────────────┬───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                       Tauron Runtime Plane                        │
│ WebView / Process / WASM Component / Builtin / Remote            │
└──────────────────────────────┬───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                       Provider / Adapter SPI                       │
│ Window / FS / HTTP / Secrets / Update / Process / Telemetry ...  │
└──────────────────────────────┬───────────────────────────────────┘
                               │
┌──────────────────────────────▼───────────────────────────────────┐
│                          Platform Layer                           │
│ Tauri Desktop / Mobile / Memory-Test / Remote / Custom Host      │
└──────────────────────────────────────────────────────────────────┘
```

核心原则：

> **Tauri 不再等于 Tauron。Tauri 是 Tauron 官方第一套、默认且最完整的 Platform Adapter。**

---

# 3. 五条不可回退的架构原则

## 3.1 单一事实源

每一种状态只能有一个 Canonical Owner：

| 状态/协议 | 唯一 Owner |
|---|---|
| Plugin lifecycle | `PluginRegistry` |
| Runtime lifecycle | `RuntimeSupervisor` |
| Runtime lease | `RuntimeManager` |
| Permission grant | `PolicyEngine / GrantStore` |
| Capability | `CapabilityRegistry` |
| Settings schema/version | `SettingsEngine` |
| Message correlation | `MessagePlane` |
| Wire schema | `Contract Schema` |
| Manifest schema | `Contract Schema` |
| Runtime protocol/ABI | `Runtime Contract` |
| Recovery phase | `RecoveryEngine` |
| Contributes | `ContributesRegistry` |

禁止出现：

- Rust 一份 Registry、TS 再维护一份 Registry；
- legacy shell 一份 lifecycle、host 再维护一份；
- UI 自己猜 Runtime 状态；
- command 是否存在由前端静态全集猜测；
- provider 状态与 capability 状态各写一份；
- Runtime 自己改 Plugin 生命周期状态而不经过 Registry。

## 3.2 Honest Capability

统一四态：

```text
full
partial
degraded
unsupported
```

严禁：

- 未实现返回 `Ok`；
- simulated 结果伪装成 full；
- capability 请求失败后默认认为“全部可用”；
- provider 未安装却用空值表示“用户取消”；
- Runtime 没有执行器却返回假 pid/假成功。

## 3.3 Fail Closed

以下领域全部默认拒绝而不是默认放行：

- 身份；
- 权限；
- Capability；
- Package Signature；
- Runtime ABI；
- Runtime Pack 签名；
- 未知 HostTransport principal；
- 未知 protocol version。

## 3.4 有界资源

任何动态表、队列、缓存、历史记录、pending、stream、subscriber、frame、runtime 都必须：

```text
有最大条目数
有最大字节数
有 TTL/生命周期回收
有诊断指标
有溢出策略
```

仅限制“条目数量”不够，必须同时限制“字节数量”。

## 3.5 默认自动闭环

第三方快速集成要求：

> **必须闭环的生命周期动作不能靠 README 提醒接入方手写。**

例如以下动作应由 App Kit / Platform Adapter 自动完成：

- 状态初始化；
- capability negotiation；
- window destroyed cleanup；
- app quit drain；
- Runtime stop；
- Recovery boot marker；
- required service ready 后的 recovery success；
- event subscription disposal；
- pending/stream cleanup；
- telemetry correlation context。

低级 API 仍可提供，但推荐入口必须自动接线。

---

# 4. 协议单线收敛：Tauron 1.1 的第一优先级

当前基线仍存在：

```text
tauron-shell HostState / PluginRegistry / EventBus / PluginDispatcher
                       vs
tauron-host / tauron-adapter / @tauron/host
```

`tauron-shell` 当前源码已经明确标记 legacy/non-canonical，但 legacy `HostState`、`PluginRegistry` 和同步 `PluginDispatcher` 仍真实存在，因此这仍是一条双事实源风险。

最终统一成：

```text
Tauron Contract
      │
      ▼
Tauron Host Kernel   ← 唯一实现
      │
      ├── canonical host API
      └── legacy facade translator
```

## 4.1 迁移规则

1. `tauron-host` 成为 Rust 唯一 Kernel；
2. `@tauron/host` 成为 TS 唯一运行 Client；
3. `tauron-shell` 不再拥有 Registry / EventBus / HostState；
4. 旧 `plugin_invoke / plugin_cancel / plugin_emit` 若保留，只作为 wire facade；
5. facade 最终转进 canonical Message Plane；
6. legacy facade 不新增功能；
7. `@tauron/core` 中与 Host 重复的 runtime class 删除/迁移；
8. Contract Test 验证 legacy facade 与 canonical API 行为一致；
9. 到 deprecation 窗口结束后再删除 facade。

## 4.2 协议版本必须拆分

不要再用一个 `Tauron 1.x` 同时代表所有兼容性。

```text
productVersion
wireVersion
manifestVersion
hostApiVersion
capabilityApiVersion
runtimeProtocolVersion
runtimeAbiVersion
wasmWorldVersion
packFormatVersion
```

示例：

```yaml
tauron: 1.4.0
wire: 1
manifest: 3
hostApi: 1
capabilityApi: 2
runtimeProtocol: 2
runtimeAbi: 2
wasmWorld: 1
packFormat: 1
```

---

# 5. 第三方快速集成模型

## 5.1 推荐入口：Tauri Plugin 形态，而不是占用 root invoke_handler

当前示例主要使用 root handler，复杂第三方项目很容易遇到自己已有 `invoke_handler` 的合并问题；Tauri plugin 形态目前又缺完整 permissions 定义。

最终建议：

```rust
fn main() {
    tauri::Builder::default()
        .plugin(tauron::tauri::init(
            TauronProfile::Substrate,
            TauronConfig::load()?,
        ))
        .invoke_handler(tauri::generate_handler![my_business_command])
        .run(tauri::generate_context!())?;
}
```

Tauron 自己通过 plugin namespace 提供能力，不抢第三方 App 的 root handler。

必须同步完成：

- 自动生成 `permissions/`；
- 自动生成 capability template；
- `plugin:tauron|*` 权限与 Contract Schema 同源；
- CLI 自动修改 `capabilities/*.json`；
- 不要求第三方手工复制 80 个 command 名。

root handler 形态保留为测试/高级模式，不作为默认入门路径。

## 5.2 前端推荐入口

```ts
const tauron = await createTauronApp({
  profile: 'substrate',
});

await tauron.ready();
```

`createTauronApp()` 只负责基础设施编排，不创建第二份状态：

- 拉取 Capability Snapshot；
- fail-closed 采用 runtime capabilities；
- 建立 lifecycle binding；
- 建立 error/trace context；
- 上报 ready；
- 注册 dispose；
- 不拥有业务状态。

## 5.3 统一配置入口

推荐演进成一个 `TauronConfig V2`：

```yaml
profile: substrate

capabilities:
  window: true
  settings: true
  notification: optional
  tray: false

runtime:
  process: on-demand
  wasm: on-demand

plugins:
  enabled: false

resources:
  memoryPolicy: balanced

recovery:
  mode: strict
```

CLI 根据配置生成：

- Cargo features；
- npm imports；
- Tauri permissions；
- runtime pack manifest；
- installer resources；
- capability expected set；
- Conformance fixture。

---

# 6. 三档产品接入模型

## 6.1 Tier S — Substrate

> **V4 修订**：Tier S 是基础能力档，不等于 Desktop UI 档；Window / Menu / Tray / Dialog 均为可选 Capability Bundle，Headless / Service 可以零 Surface 运行。

适合：HarnessDock、普通桌面工具、单窗口客户端。

默认只提供：

- lifecycle；
- capability；
- window；
- settings；
- secrets；
- event；
- recovery；
- filesystem（可选）；
- notification（可选）；
- menu/tray（可选）；
- update（可选）；
- observability。

不加载：

- Plugin Registry；
- Process Runtime；
- WASM Runtime；
- Marketplace；
- Plugin UI。

目标：基础安装最小、空闲常驻最小。

## 6.2 Tier E — Extension Platform

增加：

- Plugin Registry；
- signed plugin install；
- permission grant；
- contributes；
- WebView Runtime；
- Process Runtime broker；
- WASM Runtime broker；
- Message Plane；
- plugin update/rollback。

适合 IDE、量化客户端、Agent Desktop、数据分析客户端。

## 6.3 Tier P — Platform

再增加：

- marketplace registry；
- distribution；
- publisher identity；
- catalogue；
- white-label；
- developer portal contracts。

所有 Tier P 能力必须 optional。

---

# 7. 按需编译 + 按需安装：降低安装包体积的核心方案

当前 `tauron-adapter` 在 v1.0.2 基线下，`tauron-proc`、`tauron-brand`、`tauron-theme`、`tauron-distribute`、`tauron-wasm` 均为非 optional 依赖；`plugin-install` 还处于 default feature。即使逻辑上使用 substrate-only，依赖树仍不够“最小底座优先”。

最终改为“两级裁剪”。

## 7.1 第一级：编译期 Feature 裁剪

建议 feature 体系：

```toml
[features]
default = ["core"]

core = []
settings = ["dep:tauron-settings"]
i18n = ["dep:tauron-i18n"]
notify = ["dep:tauron-notify"]
theme = ["dep:tauron-theme"]
brand = ["dep:tauron-brand"]
update = ["dep:tauron-distribute"]
plugin-registry = []
plugin-install = ["plugin-registry", "dep:tauron-market", "dep:tauron-acl", "..."]
runtime-process = ["plugin-registry", "dep:tauron-proc"]
runtime-wasm-broker = ["plugin-registry"]
marketplace = ["plugin-install", "..."]
tauri = ["dep:tauri"]
```

关键变化：

- `tauron-adapter` 默认不再携带 plugin-install；
- Process/WASM/Market/Distribute 全部 optional；
- Substrate build 不编译 Runtime crates；
- 只有配置明确要求，CLI 才打开对应 feature。

## 7.2 第二级：运行时 Runtime Pack 按需下载安装

大型 Runtime 不应该全部静态编入基础 App。

定义签名能力包：

```text
Tauron Pack
  ├── manifest.json
  ├── content hashes
  ├── signature
  ├── runtime executable / engine
  ├── assets
  └── SBOM
```

Pack 类型：

```text
runtime-wasm
runtime-process-helper
provider-native-notification
provider-http-tls
marketplace-client
language-runtime-xxx
```

特别是 WASM engine：如果 Wasmtime/Extism 类引擎显著增大主安装包，则推荐以**独立签名 supervisor sidecar pack**交付，而不是动态加载不可信 Rust DLL 进入 Host。

## 7.3 Runtime Pack 安装状态机

```text
MISSING
  ↓ request
RESOLVING
  ↓
DOWNLOADING
  ↓
VERIFYING
  ↓
STAGED
  ↓
ACTIVATING
  ↓
READY
```

失败路径：

```text
任何阶段失败
  ↓
FAILED
  ↓ bounded retry / user retry
```

升级：

```text
READY(v1)
  ↓
STAGE(v2)
  ↓ verify
ACTIVATE(v2)
  ↓ success
GC(v1)
```

激活失败：

```text
ACTIVATE(v2) fail
  ↓
ROLLBACK(v1)
```

绝不允许：

```text
下载失败 → 自动立即重试 → 下载失败 → 自动立即重试 → ...
```

所有 retry：

- 有次数预算；
- 指数退避；
- 有停止 token；
- 达预算进入 user-confirm；
- 不在启动主线程无限等待。

## 7.4 本地缓存

Runtime Pack cache 必须：

- content-addressed；
- 可复用；
- 最大磁盘空间限制；
- LRU；
- pin 当前/回滚版本；
- 未使用版本后台低优先级清理；
- pack 未激活时不常驻内存。

---

# 8. Capability V2：第三方只依赖能力，不依赖内部 command

Capability 描述：

```yaml
id: runtime.process
apiVersion: 2
status: full
provider: tauron-process-pack
platform: windows
features:
  spawn: true
  processTree: true
  heartbeat: true
  gracefulShutdown: true
limits:
  maxInstances: 8
scope:
  owner: host
reason: null
```

统一字段：

| 字段 | 含义 |
|---|---|
| id | 稳定能力 ID |
| apiVersion | 该能力协议版本 |
| status | full/partial/degraded/unsupported |
| provider | 当前提供者 |
| platform | 平台 |
| features | 子能力 |
| limits | 数量/字节/时间限制 |
| scope | 授权范围 |
| source | builtin/pack/custom/remote |
| reason | 降级/不可用原因 |

## 8.1 Capability negotiation 失败必须 fail-closed

当前示例在 `refreshCapabilities()` 失败时保留静态全集，这会导致“宿主未知能力”被前端误认为存在。

最终语义：

```text
negotiation success
  → 使用宿主快照

negotiation failure
  → status = unknown/unsupported
  → 仅保留协议规定的 bootstrap 最小命令
  → 不允许静态全集乐观放行
```

引导命令只允许：

```text
host_protocol_info
host_capabilities
host_health
```

其余能力必须协商后开放。

---

# 9. Provider / Adapter SPI

统一正式 Provider：

```text
WindowProvider
MenuProvider
TrayProvider
DialogProvider
ClipboardProvider
FileSystemProvider
HttpProvider
NotificationProvider
SecretProvider
ProcessProvider
UpdateProvider
DeepLinkProvider
ThemeProvider
BrandProvider
TelemetryProvider
InstallationIdentityProvider
```

Provider 公共能力：

```text
capabilities()
health()
dispose()
```

平台能力不允许直接渗透 Host Kernel。

## 9.1 安装标识必须成为 Provider

当前 `DistributeUpdaterSink` 灰度分桶仍使用固定 `0`，这意味着所有安装实例可能落在同一个灰度桶。

新增：

```text
InstallationIdentityProvider
  └── stableInstallationId()
```

要求：

- 首次安装随机生成；
- 本机持久化；
- 不使用 PII；
- hash 后用于灰度分桶；
- 卸载/重装策略显式定义。

---

# 10. App Lifecycle Binding：消除第三方手工漏接

当前示例仍要求接入方手工：

- `on_window_event` 调 `cleanup_closed_window`；
- 前端主动 `recoverReport('success')`；
- 手工 refresh capability。

这些都是第三方最容易漏掉、又会导致资源泄漏/误进安全模式的问题。

新增：

```text
AppLifecycleBinding
```

统一自动处理：

```text
AppBoot
  ↓
KernelInit
  ↓
CapabilityNegotiated
  ↓
RequiredServicesReady
  ↓
SurfaceReady
  ↓
ApplicationReady
```

窗口：

```text
WindowCreated
WindowHidden
WindowDestroyed
```

应用退出：

```text
QuitRequested
  ↓
StopAcceptingNewWork
  ↓
DrainPending
  ↓
StopRuntimes
  ↓
DisposeProviders
  ↓
PersistRecoverySuccess/ExitMarker
  ↓
Exit
```

## 10.1 Recovery success 不能过早上报

App Kit 不能简单在 JS 初始化第一行就报 success。

默认 readiness 条件：

- Kernel 已初始化；
- Capability negotiation 已完成；
- required providers ready；
- required plugins/runtime ready；
- profile 声明为 required 的 Surface 已 ready；Headless / Service profile 可以是零 Surface。

第三方可追加自己的 readiness predicate。

---

# 11. RuntimeDriver：统一所有运行时

```text
RuntimeDriver
  ├── prepare()
  ├── start()
  ├── handshake()
  ├── invoke()
  ├── openStream()
  ├── cancel()
  ├── health()
  ├── drain()
  ├── stop()
  ├── dispose()
  └── capabilities()
```

实现：

```text
WebViewRuntimeDriver
ProcessRuntimeDriver
WasmRuntimeDriver
BuiltinRuntimeDriver
RemoteRuntimeDriver
```

Plugin Registry 不知道 Node/Python/Rust/WASM，只知道：

```text
Plugin → RuntimePlacement → RuntimeDriver
```

---

# 12. Runtime 类型重新定义

不要继续按编程语言分类。

## 12.1 Builtin Runtime

- 编译进 Host；
- 完全可信；
- 高性能；
- 不能用于第三方不可信动态库。

## 12.2 Process Runtime

用于：

- Node；
- Python；
- Go；
- Java；
- Rust native；
- 任意 sidecar。

## 12.3 WASM Runtime

用于：

- 低信任插件；
- 可移植插件；
- 多语言 Component；
- 小型扩展。

## 12.4 WebView Runtime

用于：

- UI Plugin；
- Panel；
- Browser Extension style capability。

## 12.5 Remote Runtime

用于：

- Remote Workspace；
- Server extension；
- 云端执行；
- 移动端作为 UI；
- 高风险 workload 隔离。

---

# 13. Process Runtime V2：必须先于生态扩张完成

当前实现已经解决了多项真实竞态和自死锁风险，但仍有工业化缺口。

## 13.1 Process status 从 bool 改为三态

当前 `ProcSpawner::is_alive()` 的设计是探测不了就返回 `true`；`CommandSpawner` 对“不在 children 表的 pid”也返回 true。这种设计避免误判崩溃，但可能把“失去所有权/句柄丢失”长期表现成 alive。

改成：

```rust
enum ProcessStatus {
    Alive,
    Exited(ExitInfo),
    Unknown(ProcessUnknownReason),
}
```

语义：

```text
Alive   → 正常
Exited  → crash/stop 流程
Unknown → 不记 crash，不宣称 alive；进入 reconcile/diagnostic
```

Unknown 连续超过阈值：

```text
RuntimeState = DEGRADED_UNKNOWN
```

由 Supervisor 决定人工恢复或重新 attach，不能伪装 Running。

## 13.2 Process Tree

Windows：

```text
Job Object
```

POSIX：

```text
process group / session
```

目标：

```text
sidecar
 └─ node
    ├─ python
    └─ helper
```

Runtime stop 后整棵树消失。

## 13.3 RAII / Drop 兜底

当前代码明确披露 `CommandSpawner` 被 Drop 时不会 kill/wait。

最终：

- 正常退出必须走 Supervisor drain；
- `Drop` 作为最后兜底尝试 terminate tree；
- Drop 不能无限阻塞；
- 失败写 emergency diagnostic；
- Windows/Unix 分别验证无遗留进程。

## 13.4 Runtime Handshake

启动成功不等于 Runtime Ready。

```json
{
  "type": "hello",
  "runtimeProtocol": 2,
  "pluginId": "example",
  "pluginVersion": "1.2.0",
  "abiHash": "...",
  "capabilities": ["invoke", "stream", "cancel"],
  "pid": 1234
}
```

完成 handshake 才能：

```text
STARTING → RUNNING
```

## 13.5 Supervisor

```text
RuntimeSupervisor
  ├── lifecycle
  ├── heartbeat
  ├── process status
  ├── crash detection
  ├── retry budget
  ├── idle policy
  └── shutdown escalation
```

## 13.6 Shutdown

```text
RUNNING
  ↓
DRAINING
  ↓
STOPPING
  ↓
TERMINATED
```

配置：

```text
gracefulTimeout
forceTimeout
```

禁止无预算无限 drain。

---

# 14. Process I/O 与低内存优化

当前 `CommandSpawner` 每个 sidecar 启动一个 `std::thread` 排空 stdout。这是实现简单且可靠的做法，但大量插件时每进程线程会引入额外线程栈和调度开销。

目标改成可替换：

```text
ProcIoDriver
  ├── ThreadedDriver      # 简单、小规模
  └── AsyncReactorDriver  # Tauri/desktop 默认，复用共享 runtime
```

规则：

- 小规模可继续 threaded；
- Extension Platform 默认 async reactor；
- 不为每个 runtime 建独立 Tokio runtime；
- 共享 executor；
- 每进程仍有独立 backpressure；
- stdout/stderr 读取缓冲有字节上限；
- 大帧之后主动 shrink/复用池，防 `Vec` 永久保持 1 MiB capacity；
- 日志也必须限速/限字节，防异常插件刷日志拖垮 Host。

---

# 15. WASM Component Runtime

当前 `tauron-wasm` 已进入 adapter 依赖，但没有真实 Wasm engine 依赖；这意味着它目前属于“配置/状态/校验层”，不能继续用 crate 名和描述让接入方误解为真正 Extism/Wasm Runtime 已运行。

在真实引擎完成前：

```text
runtime.wasm.status = unsupported / partial
reason = engine-not-installed
```

最终采用：

```text
WebAssembly Component Model
WIT
WASI
```

建议 world：

```text
tauron:plugin/runtime@1
```

Host imports：

```text
log
settings
secrets
events
http
filesystem
notification
clock
random
```

Plugin exports：

```text
activate
deactivate
invoke
health
```

如果 engine 较大：

> **WASM engine 作为签名 Runtime Pack 按需安装，由外部 supervisor 承载，基础安装包只包含 broker contract。**

---

# 16. Message Plane：消除轮询与重复通信面

统一：

```text
request/reply
stream
event
cancel
```

Envelope：

```yaml
messageId: uuid
traceId: uuid
correlationId: uuid
caller: ...
target: ...
timestamp: ...
deadline: ...
priority: normal
```

## 16.1 跨主体调用不再让 SDK 100ms 轮询取件

当前示例用 `callTakeResult()` 每 100ms 轮询、最多 5 秒。

最终 SDK：

```ts
const result = await shell.callPluginAndWait(target, method, args, {
  signal,
  deadline,
});
```

实现：

```text
request accepted
  ↓
Message Plane correlation
  ↓
runtime result
  ↓
completion channel/event
  ↓
Promise resolve
```

`callTakeResult` 只保留：

- reconnect；
- recovery；
- debug；
- legacy compatibility。

这样减少：

- timer wakeup；
- IPC 次数；
- CPU；
- 电量；
- pending 查询竞争。

## 16.2 Cancellation 端到端传播

```text
AbortSignal
  ↓
Message Plane cancel
  ↓
RuntimeDriver.cancel
  ↓
Process/WASM/WebView/Remote
```

不允许只删 pending 表。

## 16.3 Deadline

所有请求支持 deadline；超时由 Host 统一终结并发送终帧。

---

# 17. Event Bus：身份与窗口归属必须由 Transport 证明

Tauri 当前 `host_events_subscribe` 已用真实 window label 作为 window 参数，这是正确的；但 core EventBus API 本身仍允许调用方传 `window` 字符串。

对 Custom Host 不能允许客户端自报任意 window/surface identity。

最终：

```text
HostTransport
  └── verified Principal + SurfaceIdentity
```

Kernel 收到的：

```text
CallerContext {
  principal,
  surfaceId,
  origin,
  transportId
}
```

业务参数中不再出现可伪造 `window`。

---

# 18. Settings Engine V2：闭合 Watch 并减少内存复制

当前 `SettingsStore::watch()` 明确没有生产调用点，而且 `plugin_id` 被忽略；Watcher 为每订阅者维护 `Vec<ChangeEvent>`，满时 `remove(0)`，会产生 O(n) 移动，并复制完整 `serde_json::Value`。

最终改成：

```text
SettingsEngine
  ├── schema
  ├── version
  ├── migration
  ├── transaction
  ├── watch
  ├── namespace
  └── reload policy
```

## 18.1 Watch 接入 Message/Event Plane

订阅：

```text
settings:<namespace>:<key-pattern>
```

插件只允许自己的 namespace。

## 18.2 内存优化

Watcher 使用：

- `VecDeque`，不再 `Vec.remove(0)`；
- 按 key coalesce：同一 key 多次快速修改只保留最后值；
- 大对象默认只发 `{key, version, hash}`，需要值时再取；
- 或 `Arc<Value>` 避免 N 个 subscriber 克隆完整 JSON；
- 同时限制事件条目数和总字节数；
- subscriber dispose 自动回收。

## 18.3 Reload Policy

```yaml
hotReload: true
restartRequired: none | plugin | runtime | application
```

迁移仍保持当前“步数上限 + 全有或全无”的防死循环设计。

---

# 19. Settings 与 Secrets 必须分离

新增：

```text
SecretProvider
```

敏感数据：

- API key；
- token；
- password；
- refresh credential；
- private key。

禁止写普通 Settings。

平台实现：

```text
Windows Credential Manager
macOS Keychain
Linux Secret Service
Enterprise custom provider
```

---

# 20. 插件安装、升级、回滚完整生命周期

完整链：

```text
Resolve
  ↓
Download / local package
  ↓
Verify signature/hash
  ↓
Parse Manifest
  ↓
Compatibility check
  ↓
Permission diff
  ↓
User/Admin grant
  ↓
Atomic stage
  ↓
Registry install
  ↓
Enable
  ↓
Runtime prepare/start
```

失败时：

```text
任何阶段失败
  ↓
rollback staged filesystem
  ↓
no half-installed runtime
  ↓
visible diagnostic record
```

升级：

```text
old active
  ↓
stage new
  ↓
verify
  ↓
permission diff
  ↓
activate new
  ↓
health probation
  ↓ success        ↓ failure
commit new         rollback old
```

卸载：

```text
stop accepting calls
  ↓
drain/cancel pending
  ↓
stop runtime tree
  ↓
remove contributions
  ↓
remove subscriptions
  ↓
remove settings watchers
  ↓
remove notification state
  ↓
remove grants
  ↓
remove files
  ↓
remove registry entry
```

必须保证“卸载后零资源残留”。

---

# 21. App Update 与 Plugin/Pack Update 分域

当前存在 `host_market_*` 模拟更新命令与 `host_updater_*` 更新通道，语义容易重叠。

最终明确三个域：

```text
AppUpdateService
PluginUpdateService
RuntimePackService
```

`market` 只负责插件发现/registry metadata，不再冒充应用更新器。

模拟 `host_market_download/install`：

- 不再作为长期 public API；
- 进入 deprecated；
- 替换为真实 PluginPackageService；
- 没有 provider 时返回 unsupported。

---

# 22. Security Plane

统一安全链：

```text
Publisher Identity
  ↓
Package Signature
  ↓
Install Verification
  ↓
Permission Review
  ↓
Grant Store
  ↓
Runtime Identity
  ↓
Invocation Authorization
  ↓
Audit
```

## 22.1 Permission Diff

升级新增权限必须重审：

```diff
 filesystem.read
+network
+process.spawn
```

## 22.2 Scope

```yaml
filesystem:
  read:
    - workspace
network:
  domains:
    - api.example.com
process:
  executable:
    - bundled-sidecar
```

## 22.3 Publisher Trust

```text
publisherId
keyId
validFrom
validUntil
revoked
replacementKey
```

## 22.4 Event subscription approval

当前 EventBus 有 `approve()` 核心能力，但没有完整线上用户审批入口。

最终 Permission Broker 提供：

```text
requestApproval
approve
revoke
listGrants
```

审批必须进入 Audit。

---

# 23. Plugin Manifest V3

```text
identity
runtime
compatibility
permissions
contributes
resources
settings
dependencies
distribution
signing
```

示例：

```yaml
identity:
  id: example.plugin
  version: 1.2.0

runtime:
  kind: process
  protocol: 2
  delivery: on-demand

compatibility:
  tauron: ^1.2
  wire: 1
  hostApi: ^1
  runtimeAbi: 2

permissions:
  filesystem:
    read: [workspace]

resources:
  memoryMb: 256
  maxPending: 32
  maxStreams: 8

contributes:
  commands: []
  panels: []
```

Manifest 是 requested permission；真正有效的是 Grant Store。

---

# 24. Contributes 统一闭环

统一：

```text
commands
menus
panels
settings
themes
views
statusItems
keybindings
```

生命周期：

```text
Manifest declaration
  ↓
Runtime activate register
  ↓
Host reconcile
  ↓
UI/behavior projection
```

禁用/卸载自动撤销所有贡献。

当前 `UNWIRED_EVENTS` 可以作为过渡期显式登记，但最终目标不是永久维护“已知未接线”，而是：

```text
接线
或
从 public contract 删除
```

不允许长期第三态。

---

# 25. UI 与 SDK 收敛

当前存在 legacy iframe/plugin-sdk 与 app-plugin-sdk 等历史路线。

最终对第三方暴露的主要心智模型尽量控制为：

```text
@tauron/host
@tauron/plugin
@tauron/app
@tauron/ui          # optional
@tauron/react       # thin
@tauron/vue         # thin
@tauron/svelte      # thin
```

原则：

- Vanilla TS 一等公民；
- React/Vue/Svelte 只做薄绑定；
- `@tauron/ui-primitives` 保持 host-independent；
- demo 只展示 canonical SDK；
- legacy iframe demo 移入 `examples/legacy-*`，不与主示例混在一起；
- public export 无消费者时必须选择：接线 / testing-only / 删除。

---

# 26. MemoryHost：从 Mock 升级成 Reference Host

当前 `MemoryTransport` 已有 HostTransport 形态，但仍应进一步升级为真正 reference implementation。

目标：

```text
MemoryHost Kernel Adapter
```

不是一张任意 command-handler map，而是：

- 使用真实 Contract；
- 使用真实 Kernel；
- 不依赖 Tauri；
- HostClient 在 Memory 与 Tauri 下行为一致；
- Conformance Kit 以 MemoryHost 作为行为基准。

这可以把大量集成测试从 GUI/Tauri runtime 中抽离，加快 CI。

---

# 27. Schema / Codegen：彻底减少 Rust/TS/权限/文档漂移

建立：

```text
contracts/
  wire/
  manifest/
  capability/
  lifecycle/
  error/
  runtime/
  permission/
  pack/
```

生成：

```text
Rust types
TypeScript types
JSON Schema
WIT
Tauri permissions
Capability reference
Error reference
Docs tables
Test fixtures
Compatibility fixtures
```

现有 wire-gate 保留，作为第二道防线。

最终：

```text
Contract Schema
  ↓ codegen
Rust / TS / WIT / permissions / docs
  ↓
Contract Tests
  ↓
Runtime Conformance
```

---

# 28. 性能与内存总体策略

## 28.1 空闲状态原则

Tier S 空闲时：

```text
0 plugin runtime
0 wasm engine
0 process supervisor worker（无进程时）
0 polling timer
0 marketplace refresh
0 updater timer（除非用户配置）
```

只保留：

- Kernel small state；
- Recovery marker；
- Capability Registry；
- 必要 Settings metadata；
- Platform Adapter。

## 28.2 Lazy Initialization

Eager：

```text
Kernel
Recovery
Capability Registry
Installation Identity
```

Lazy：

```text
i18n bundle
Theme Registry
Notification history
Plugin Registry（Tier S 不建）
Process Runtime
WASM Engine
Marketplace
Updater network client
Telemetry exporter
```

## 28.3 数据结构

优先：

- `VecDeque` 用于 FIFO；
- `Arc` 共享大 payload；
- 状态类数据 coalesce；
- content-address cache；
- 避免复制完整 Manifest/Value；
- 大 payload 走 frame/binary channel，不 base64；
- 有界 LRU。

## 28.4 消除无效轮询

以下全部事件驱动：

- call completion；
- runtime exit；
- settings change；
- plugin lifecycle；
- pack install progress。

Heartbeat 是唯一合理的定期检查之一，也必须共享 scheduler，而不是每 Runtime 一个无限 timer。

---

# 29. 性能合同与回归门禁

当前 CI 没有正式 benchmark、RSS、bundle size gate。

第一阶段不先拍脑袋规定绝对数字，先建立同机 baseline；随后使用相对回归门禁。

记录：

```text
Host cold startup
Kernel init
Capability negotiation
Plugin attach
RPC latency p50/p95/p99
Stream throughput
Event delivery latency
Process spawn/handshake
WASM pack load/instantiate
Idle RSS
RSS per plugin
CPU idle
Shutdown duration
Installer size
Core binary size
Frontend JS gzip/brotli size
```

建议初始回归规则：

```text
同平台同 profile：
- 启动/RPC p95 不允许无说明恶化 > 10%
- Idle RSS 不允许无说明恶化 > 10%
- Installer size 不允许无说明恶化 > 5%
- 零资源泄漏项必须始终为 0
```

绝对预算在获得 3~5 个稳定 release baseline 后再锁死。

## 29.1 三套体积基线

必须分别记录：

```text
substrate-minimal
extension-standard
platform-full
```

不能只测 full build 后声称 Tauron 小。

---

# 30. 资源预算

当前已有 pending / streams / subscriptions / notifications 数量上限，继续升级为统一 ResourcePolicy：

```text
maxPendingCount
maxPendingBytes
maxStreamCount
maxStreamBufferedBytes
maxSubscriptions
maxEventQueueCount
maxEventQueueBytes
maxNotificationCount
maxRuntimeCount
maxRuntimeMemory
maxRuntimeCpu
maxLogBytesPerMinute
maxPackDiskBytes
```

ResourceManager 是唯一 Owner。

UI 通过 `resourceStats()` 读取，不能各模块自己统计第二份。

---

# 31. Observability

统一：

```text
Trace
Metric
Log
AuditEvent
```

上下文：

```text
traceId
hostId
pluginId
runtimeId
callId
transportId
```

标准事件：

```text
host.start
host.ready
host.stop
plugin.install
plugin.enable
plugin.disable
plugin.invoke
runtime.spawn
runtime.handshake
runtime.ready
runtime.crash
runtime.stop
pack.download
pack.verify
pack.activate
permission.denied
recovery.enter
recovery.exit
update.rollback
```

Provider：

```text
Noop
File
OTLP
Custom
```

默认 Noop，不强迫第三方引入 telemetry 后端。

---

# 32. Recovery：禁止重启死循环

Recovery 保留当前安全模式思想，并增加统一 retry budget。

任何自动动作必须具备：

```text
attempt count
window
backoff
max retry
terminal state
manual resume path
```

Runtime crash：

```text
RUNNING
  ↓ crash
ERRORED_RETRYABLE
  ↓ budget remains
BACKOFF
  ↓
STARTING
```

预算耗尽：

```text
ERRORED_USER_CONFIRM
```

绝不自动无限重启。

更新失败、Pack 下载失败、WASM 启动失败遵循同一模式。

---

# 33. 三轮完整审查与修复结果

以下是基于当前 `main@4e5c37f...` 和融合方案的三轮审查结论。每轮问题都已经在最终架构中安排确定性修复，不保留“知道有问题但没有 Owner/Exit Gate”的设计悬案。

## Round 1 — Ownership / Architecture

### R1-1 双 Kernel / 双事实源

**发现**：`tauron-shell` legacy HostState/Registry/EventBus 与 canonical `tauron-host` 并存。  
**修复**：W1 单线化；shell 仅 facade，Kernel 只剩一套。  
**Gate**：源码门禁禁止 `tauron-shell` 再定义 Registry/EventBus/HostState。

### R1-2 最小底座依赖仍偏重

**发现**：adapter 无条件依赖 proc/brand/theme/distribute/wasm，plugin-install 默认打开。  
**修复**：按 domain optional feature + 三 Profile；base default 仅 core。  
**Gate**：`cargo tree` profile snapshot + binary size budget。

### R1-3 Tauri plugin 默认接入仍缺 permissions 完整闭环

**发现**：第三方最快的 plugin namespace 接入没有成为 canonical happy path。  
**修复**：Schema 自动生成 permissions；plugin form 成为默认集成路径。  
**Gate**：fresh Tauri app 只 `.plugin(tauron::init())` 即可编译运行。

### R1-4 Capability negotiation 失败方向不安全

**发现**：示例拉能力失败后仍保留静态全集。  
**修复**：unknown = unsupported；只有 bootstrap 三命令可用。  
**Gate**：断开 capability 命令时，feature-gated API 必须 fail-closed。

### R1-5 WASM 名义能力与真实执行能力不一致

**发现**：wasm crate 已接 adapter，但没有真实 engine。  
**修复**：Capability 标 partial/unsupported；完成 Runtime Pack 前不宣称执行能力。  
**Gate**：`runtime.wasm=full` 必须通过真 Component E2E。

### R1-6 App Update / Market Update 语义重叠

**发现**：`host_market_*` 模拟更新与 `host_updater_*` 并存。  
**修复**：AppUpdate / PluginUpdate / RuntimePackUpdate 三域分离。  
**Gate**：同一更新行为只有一个 canonical service。

**Round 1 结果**：所有发现均已有唯一 Owner 与迁移路径，可以进入 Round 2。

---

## Round 2 — End-to-End / Frontend ↔ Backend

### R2-1 Window cleanup 依赖第三方手工钩子

**发现**：当前示例要求 `on_window_event` 手工调用 cleanup。漏接会残留 subscription/pending。  
**修复**：Platform Adapter 内建 AppLifecycleBinding。  
**Gate**：第三方无任何 cleanup 代码时，关闭插件窗口资源仍归零。

### R2-2 Recovery success 依赖前端手工调用

**发现**：漏掉 `recoverReport('success')` 会累计失败并误进安全模式。  
**修复**：App Kit readiness transaction 自动上报；业务可追加 readiness predicate。  
**Gate**：最小模板不写 recovery 调用仍能正确完成启动/退出标记。

### R2-3 跨主体调用用 100ms 轮询

**发现**：主示例 50 × 100ms polling。  
**修复**：completion channel + Promise；take 仅 reconnect/debug。  
**Gate**：正常 call 过程中 `host_call_take` 次数为 0。

### R2-4 Settings Watch 无生产出口

**发现**：watch API 当前只有测试调用，且 plugin_id ignored。  
**修复**：按命名空间接 Message Plane；coalesce + VecDeque/Arc。  
**Gate**：settings set → subscriber callback E2E；跨插件观察被拒。

### R2-5 Event Approval 无线上审批入口

**发现**：核心 `approve()` 有逻辑，但无用户/管理面闭环。  
**修复**：Permission Broker + approval/revoke API + UI optional。  
**Gate**：private topic 先拒绝，approve 后可订阅，revoke 后再次拒绝。

### R2-6 Canonical SDK 与 legacy iframe 示例混用

**发现**：主示例同时展示 legacy PluginBridge iframe 与主推 App Plugin SDK。  
**修复**：主示例只保留 canonical；legacy 独立示例。  
**Gate**：主 quickstart 不 import legacy SDK。

### R2-7 配置失败后的回退策略不明确

**发现**：ClientConfig 失败后示例回到默认 AdapterConfig，安全/安装能力可能与预期不同。  
**修复**：ConfigPolicy：`strict | safe-default | custom`；生产默认 strict/safe-default，错误可观测。  
**Gate**：错误配置不能静默打开额外能力。

### R2-8 灰度 bucket 使用固定 0

**发现**：当前 updater 示例/实现注释明确使用固定安装 hash 0。  
**修复**：InstallationIdentityProvider。  
**Gate**：测试不同 installation id 分桶不同且稳定。

### R2-9 Custom Host surface identity 不能靠客户端自报

**发现**：核心订阅模型包含 `window` 维度，Tauri 由真实 label 提供，但其他 transport 未来可能误用入参。  
**修复**：CallerContext 由 HostTransport 校验后注入。  
**Gate**：所有 public kernel API 不接受用户可伪造 surface identity。

### R2-10 本地插件安装还不是完整按需包体系

**发现**：目前重点是 signed local package；还缺 Runtime Pack / Registry resolved on-demand。  
**修复**：统一 Pack Manager 与插件安装事务。  
**Gate**：空白系统首次调用缺失 runtime → 下载 → 验签 → 激活 → 调用成功 E2E。

**Round 2 结果**：所有主链都有“起点、终点、回收点、错误出口”；进入 Round 3。

---

## Round 3 — Failure / Recovery / Performance / Memory

### R3-1 每 sidecar 一个 OS reader thread

**风险**：多 Runtime 时线程栈与调度开销上升。  
**修复**：ProcIoDriver，Extension profile 默认 shared async reactor。  
**Gate**：N runtime 压测记录 thread count/RSS，不按 runtime 线性增加 reader thread。

### R3-2 `is_alive: bool` 把 Unknown 等同 Alive

**风险**：句柄丢失可能形成长期 stale lease。  
**修复**：三态 ProcessStatus + reconcile。  
**Gate**：失去跟踪句柄时必须显示 Unknown/Degraded，不能继续报 Running。

### R3-3 Drop / Process Tree 不完整

**风险**：异常退出可能遗留直接子进程/孙进程。  
**修复**：Supervisor shutdown + Job Object/process group + Drop emergency cleanup。  
**Gate**：Win/macOS/Linux no-orphan E2E。

### R3-4 Settings watcher 数据结构成本

**风险**：`Vec.remove(0)` O(n)，每 subscriber clone 完整 Value。  
**修复**：VecDeque + key coalesce + Arc/delta + byte budget。  
**Gate**：10k settings writes 压测内存有上限，CPU 不出现 O(n²) 增长。

### R3-5 只有数量配额，没有统一字节配额

**风险**：1000 个“大 Value”仍可耗尽内存。  
**修复**：ResourcePolicy 同时约束 count + bytes。  
**Gate**：大 payload fuzz 不可突破总字节上限。

### R3-6 CI 没有 performance/RSS/bundle-size gate

**修复**：三 profile baseline + benchmark artifact + regression gate。  
**Gate**：每 PR 产生 size/perf diff。

### R3-7 Registry consumer 验证发生太晚

**发现**：当前 clean registry consumer 在 publish-crates 后执行，且脚本硬编码 `1.0.2`，主要跑 Windows。  
**修复**：增加 pre-publish tarball/.crate consumer（三平台），动态版本；发布后 registry smoke 保留。  
**Gate**：PR 阶段就能证明仓外消费者可构建。

### R3-8 安全审计 gate 仍是 advisory

**发现**：`cargo-deny` job `continue-on-error: true`。  
**修复**：license/bans/source + 高危 RustSec 为 hard gate；确需例外必须带 expiry/issue。  
**Gate**：无理由的高危漏洞不得发布。

### R3-9 main 未保护

**发现**：审计基线时 `main.protected=false`，required checks 未启用。  
**修复**：branch ruleset + required CI + release provenance。  
**Gate**：发布 tag 必须来自 required-checks green commit。

### R3-10 真 sidecar E2E 尚无运行证据

**发现**：`tauron-proc` 明确记录测试不启动真实 sidecar。  
**修复**：仓内增加 tiny deterministic fixture sidecar。  
**Gate**：三平台验证 spawn → handshake → request → response → cancel → exit → no orphan。

### R3-11 大 frame 后缓冲容量可能长期保持

**修复**：buffer pool cap / shrink policy；连续大帧使用 bounded reusable slab。  
**Gate**：峰值 frame 后 RSS 可回落到预算区间。

### R3-12 所有 Runtime 自动重试必须有终态

**修复**：统一 RetryPolicy + backoff + max attempts + user-confirm。  
**Gate**：故障注入测试运行固定时长，restart count 有明确上限。

**Round 3 结果**：主链、失败链、性能与资源链全部具备明确的闭环设计；没有保留无 Owner 的设计悬案。

---

# 34. 防死循环/防死锁硬规则

## 34.1 禁止无限重试

所有 retry loop 必须包含：

```text
attempt++
maxAttempts
backoff
cancel token
terminal state
```

## 34.2 禁止锁内回调

任何：

```text
lock → user/plugin/provider callback
```

一律禁止。

先 clone 必要状态，释放锁，再 callback。

当前 process spawner 已修复过 `if let` 临时 guard 导致的同锁再入风险，这类源码形状门禁必须永久保留。

## 34.3 锁序唯一

每模块声明 lock order；禁止新的反序嵌套。

## 34.4 生命周期只能经状态机

禁止直接：

```text
state = RUNNING
state = DISABLED
```

必须：

```text
Event → transition → state
```

## 34.5 Migration 必须有步数上限

保持现有全有或全无 + step limit，不允许循环 migration。

---

# 35. 第三方项目接入体验目标

## 35.1 新项目

目标命令：

```bash
npm create tauron-app@latest -- my-app --profile substrate
```

生成后：

```bash
cd my-app
pnpm install
pnpm tauri dev
```

无需手工：

- 拼 host command；
- 写 window cleanup；
- 写 capability 列表；
- 写 recovery success；
- 配 80 条 Tauri permission。

## 35.2 已有 Tauri 项目

```bash
npx @tauron/app-cli init --profile substrate
```

CLI：

- 检测现有 Tauri；
- 注册 Tauron plugin；
- 合并 capability；
- 保留用户 root commands；
- 生成 TauronConfig；
- 执行 conformance smoke；
- 报告冲突，不静默覆盖。

## 35.3 添加能力

```bash
tauron add settings
tauron add plugin-runtime
tauron add wasm --delivery on-demand
```

CLI 自动调整编译 feature 和 pack manifest。

---

# 36. Conformance Kit

第三方 Host：

```bash
tauron conform host
```

检查：

```text
protocol
capabilities
identity
invoke
stream
event
cancel
settings
permission
lifecycle
recovery
shutdown
resource cleanup
```

插件：

```bash
tauron conform plugin
```

检查：

```text
manifest
signature
compatibility
permission
handshake
ABI
invoke
stream
cancel
shutdown
resource cleanup
```

Runtime Pack：

```bash
tauron conform pack
```

检查签名、hash、原子激活、rollback、cache cleanup。

---

# 37. CI / Release Gate

必须包含：

```text
Rust unit
TS unit
Typecheck
Lint
Format
Wire Contract
Schema Drift
Generated Code Drift
Permission Drift
Dependency Cycle
Tauri substrate integration
Extension integration
MemoryHost Conformance
Process true-sidecar E2E
WASM Component E2E
Crash Recovery E2E
Cancel E2E
Permission E2E
Plugin install/update/rollback E2E
Runtime Pack E2E
No-Orphan Process Gate
Pre-publish External Consumer Gate
Post-publish Registry Consumer Smoke
N-1 Compatibility
Security Audit
Performance Baseline
Memory Baseline
Installer/Binary/JS Size Budget
```

## 37.1 External Consumer 分两层

PR 前：

```text
pnpm pack + cargo package
  ↓
全新目录消费本地产物
  ↓
Win/macOS/Linux build
```

发布后：

```text
npm/crates.io clean install smoke
```

版本从 workspace 自动读取，不能硬编码 `1.0.2`。

---

# 38. 仓库治理

稳定第三方底座必须启用：

```text
main branch protection / ruleset
required CI
CODEOWNERS
SECURITY.md
CONTRIBUTING.md
release checklist
SBOM
artifact provenance
signed release/tag where possible
ADR
RFC
deprecation policy
```

RFC 必须覆盖：

```text
Wire
Manifest
Runtime ABI
WIT
Capability
Security Model
Lifecycle
Pack Format
```

普通 bugfix 不需要 RFC。

---

# 39. Definition of Done

任何 Tauron 能力只有同时满足以下条件才能标 `full`：

1. Canonical Contract；
2. 单一 Owner；
3. Rust/TS/WIT 类型由同源生成或有强 Contract Gate；
4. 有真实入口；
5. 有真实消费者；
6. 有真实 Provider/Runtime 或明确 unsupported；
7. 有身份与权限检查；
8. 有错误模型；
9. 有生命周期回收；
10. 有字节/数量资源预算；
11. 有观察出口；
12. unit test；
13. contract test；
14. 至少一条真实 E2E；
15. failure/recovery test；
16. 文档与 capability 自动同步；
17. profile build 不引入无关依赖；
18. 性能/体积没有超 budget。

否则只能：

```text
partial
experimental
planned
unsupported
```

---

# 40. 实施工作流

## P0 — Tauron 1.1 Platform Foundation

### W1 Protocol Single-Line

- 删除 legacy state engine；
- shell 变 facade；
- command/contract 单源。

### W2 Minimal Profile / Feature Graph

- adapter 默认 core；
- proc/wasm/market/distribute optional；
- 三 profile size gate。

### W3 Tauri Plugin Canonical Integration

- permissions codegen；
- capability template；
- 不占 root handler。

### W4 Capability V2

- fail-closed negotiation；
- provider health；
- pack source。

### W5 AppLifecycleBinding

- auto cleanup；
- boot readiness；
- quit drain。

### W6 Settings/Secrets Foundation

- watch E2E；
- SecretProvider。

### W7 Observability + InstallationIdentity

### W8 External Consumer + Branch Governance

**1.1 Exit Gate**：

> 第三方现有 Tauri 应用通过一个 Tauron plugin + 一份生成配置即可获得最小 Substrate；无手工生命周期补丁；基础 profile 不包含 Runtime/Market 重依赖。

---

## P1 — Tauron 1.2 Runtime Platform

### W9 RuntimeDriver

### W10 Process Runtime V2

- ProcessStatus 三态；
- process tree；
- handshake；
- supervisor；
- graceful shutdown；
- RAII cleanup；
- async I/O；
- true sidecar E2E。

### W11 Message Plane Completion Push

### W12 ResourcePolicy count + bytes

### W13 WASM Runtime Pack + WIT

**1.2 Exit Gate**：

```text
WebView
Process
WASM
```

通过同一 Runtime Conformance，且关闭 Host 后无任何 Runtime/孙进程残留。

---

## P1/P2 — Tauron 1.3 Extension Platform

- Manifest V3；
- Permission Diff；
- Contributes 全闭环；
- Plugin dependency resolver；
- Plugin update/rollback；
- Runtime Pack Manager；
- Publisher Trust；
- Remote Runtime POC。

---

## P2 — Tauron 1.4 Ecosystem

- Marketplace registry protocol；
- catalogue；
- distribution；
- white-label；
- developer portal contracts。

全部 optional。

---

## Tauron 2.0

只有以下全部满足才进入 2.0：

```text
Wire Stable
Manifest Stable
Capability Stable
Runtime Protocol Stable
Runtime ABI Stable
WIT Stable
Pack Format Stable
Tauri Adapter Stable
N-1 Compatibility
Public Conformance Kit
Cross-platform Packaged E2E
No-Orphan Process Gate
Security Review
Performance/Memory/Size budgets stable
至少 3 个真实第三方 Consumer
```

---

# 41. HarnessDock 与其他项目的验证顺序

推荐真实 Consumer 顺序：

```text
1. Minimal Substrate App
2. HarnessDock
3. Data Analysis Client
4. QMT/Quant Desktop
5. Agent Desktop
6. IDE-like Multi-plugin Client
```

HarnessDock 仍拥有：

- DSH Runtime Lifecycle；
- RuntimeLease；
- DSH Supervisor；
- DSH process tree；
- Rescue Web；
- DSH-specific updater。

Tauron 提供：

- 通用 Host Substrate；
- Window/Menu/Tray；
- Settings/Secrets；
- Capability；
- Recovery primitives；
- UI-independent Event/Message；
- 通用插件能力。

原则：

> Tauron 不能为了适配 HarnessDock 抢走 HarnessDock 的 DSH Runtime 所有权；真实 Consumer 用来验证通用性，而不是把特定产品逻辑塞回 Tauron。

---

# 42. 最终主体链路验收

## 42.1 Substrate

```text
Install dependencies
  ↓
Tauron Tauri plugin init
  ↓
Contract bootstrap
  ↓
Capability negotiate
  ↓
Providers ready/lazy
  ↓
App ready
  ↓
UI call
  ↓
SDK
  ↓
Transport
  ↓
Kernel
  ↓
Provider
  ↓
OS
  ↓
Response
  ↓
UI
```

## 42.2 Extension

```text
Resolve Plugin
  ↓
Verify
  ↓
Grant
  ↓
Install
  ↓
Register
  ↓
Runtime resolve
  ↓
Missing pack? → on-demand install
  ↓
Prepare/Start/Handshake
  ↓
RUNNING
  ↓
Invoke/Stream/Event
  ↓
Disable/Uninstall
  ↓
Drain
  ↓
Stop tree
  ↓
Cleanup all resources
```

## 42.3 Quit

```text
QuitRequested
  ↓
NoNewCalls
  ↓
Drain/Cancel
  ↓
Close Streams
  ↓
Stop Runtimes
  ↓
Dispose Providers
  ↓
Persist Recovery Exit
  ↓
Assert no child process
  ↓
Exit
```

这三条链必须全部有 E2E。

---

# 43. 最终验收清单

必须全部满足：

```text
[ ] 无双 Kernel
[ ] 无双 Registry
[ ] 无手工镜像协议事实源
[ ] 无有类型无入口
[ ] 无有入口无消费者
[ ] 无 UI 事件无人处理（除明确内部/删除）
[ ] 无 Runtime 无退出路径
[ ] 无 provider 假成功
[ ] 无 capability 乐观回退
[ ] 无永久 pending
[ ] 无永久 stream
[ ] 无无限 queue
[ ] 无无限 retry
[ ] 无锁内 callback
[ ] 无锁序环
[ ] 无 process child/grandchild orphan
[ ] 无 Settings subscriber orphan
[ ] 无 Plugin contributes orphan
[ ] 无 Permission grant orphan
[ ] 无 Runtime Pack 半安装
[ ] 无更新半激活
[ ] substrate profile 不编译 Runtime/Market 重依赖
[ ] 空闲无无意义轮询
[ ] 性能/内存/体积可持续测量
[ ] pre-publish clean consumer 三平台通过
[ ] N-1 compatibility 通过
[ ] security hard gate 通过
[ ] main required checks 启用
```

---

# 44. 最终推荐执行顺序

```text
1. W1 协议/Kernel 单线
2. W2 Feature/Profile 最小化
3. W3 Tauri Plugin + permissions 自动接入
4. W4 Capability V2 fail-closed
5. W5 Lifecycle 自动闭环
6. W6 Settings Watch + Secrets
7. W7 Observability + InstallationIdentity
8. W8 外部 Consumer / 治理 / 性能体积 baseline
9. W9 RuntimeDriver
10. W10 Process Runtime V2
11. W11 Message Plane push completion
12. W12 Resource bytes policy
13. W13 WASM Runtime Pack/WIT
14. Manifest V3 / Permission Diff / Plugin update rollback
15. Remote Runtime
16. Marketplace / Ecosystem
```

不要调整为“先做 Marketplace / WASM UI / 更多命令再回来收敛基础”。

---

# 45. 最终架构目标

```text
                         Third-party Product
                                 │
                      ┌──────────▼──────────┐
                      │   Tauron App Kit     │
                      └──────────┬──────────┘
                                 │
                      ┌──────────▼──────────┐
                      │  Contract / Codegen  │
                      └──────────┬──────────┘
                                 │
              ┌──────────────────▼──────────────────┐
              │           Tauron Host Kernel         │
              │ Registry / Policy / Lifecycle       │
              │ Settings / Recovery / Message       │
              │ Capability / Resource / Contributes │
              └──────────────────┬──────────────────┘
                                 │
        ┌────────────────────────┼─────────────────────────┐
        │                        │                         │
┌───────▼────────┐     ┌─────────▼─────────┐     ┌────────▼────────┐
│ WebView Runtime│     │ Process Runtime   │     │ WASM Runtime    │
└────────────────┘     └───────────────────┘     └─────────────────┘
                                 │
                      ┌──────────▼──────────┐
                      │ Provider/Adapter SPI │
                      └──────────┬──────────┘
                                 │
             ┌───────────────────┼───────────────────┐
             │                   │                   │
      ┌──────▼───────┐   ┌───────▼──────┐   ┌──────▼──────┐
      │ Tauri Adapter │   │ Remote Adapter│   │ Custom Host │
      └───────────────┘   └──────────────┘   └─────────────┘

                 Optional Runtime / Provider Packs
                ┌──────────┼──────────┐
                ▼          ▼          ▼
             WASM Pack   TLS Pack   Other Pack
```

最终第三方项目只需要开发自己的：

```text
业务逻辑
产品 UI
领域模型
业务后端
```

而不再重复实现：

```text
窗口
菜单
托盘
配置
Secrets
权限
插件
Runtime
IPC/RPC
事件
恢复
更新
日志
诊断
分发
进程树
兼容门禁
```

---

# 46. 最终结论

Tauron 未来最重要的升级，不是“继续增加更多功能”，而是完成三个转变：

### 转变一：从 Tauri 插件框架 → 稳定 Application Substrate

核心是 Contract、Kernel、Provider、Runtime 的单线化。

### 转变二：从全量打包 → Profile + Feature + Signed Runtime Pack 按需能力

使基础安装包更小、内存更低、功能按需下载启用。

### 转变三：从“仓库内测试通过” → “第三方长期依赖可证明”

依赖：

```text
Conformance
External Consumer
N-1 Compatibility
No-Orphan
Performance/RSS/Size Gate
Security Gate
Branch Governance
```

最终产品目标可以正式固定为：

> **Tauron 是一个可被 HarnessDock、量化客户端、Agent Desktop、IDE、数据客户端以及其他第三方系统快速集成并长期稳定依赖的模块化应用底座；它以低耦合、低常驻资源、按需能力、运行时隔离、安全可审计和稳定兼容为核心竞争力。**

---


# 47. V3 结论：是否可以适配“任意客户端”

结论必须分成两层理解。

## 47.1 架构意义上的“任意客户端”：可以作为目标

V3 收敛后的 Tauron 可以把以下系统统一纳入同一个 Contract / Kernel / Runtime / Provider 模型：

```text
Tauri Desktop
Electron
Qt / C++
Flutter Desktop
.NET / WinUI / WPF / Avalonia
Swift / AppKit
Kotlin / Compose Desktop
Native Mobile Shell
React Native / Flutter Mobile
Headless CLI
Background Service
Local Daemon
Remote Web Client
Remote Workspace
Server-side Host
Custom Enterprise Client
```

但成立条件不是“这些客户端天然兼容 Tauron”，而是：

```text
Client
  ↓
实现一个受支持的 Integration Profile
  ↓
Universal Client Contract
  ↓
tauron conform host
  ↓
通过
```

因此正式承诺应写成：

> **Tauron 可适配任何能够实现其 Universal Client Contract 的宿主；Tauri 是官方默认且最完整 Adapter。**

而不能写成：

> “任意客户端无需适配即可直接使用 Tauron”。

## 47.2 物理能力受宿主环境限制

纯浏览器不能直接拥有：

```text
process.spawn
native filesystem
system tray
native updater
OS credential store
```

这时不是 Tauron 失败，而是 Capability 必须诚实返回：

```text
unsupported
```

或者通过：

```text
Browser UI
   ↓ secure transport
Remote/Local Tauron Host
   ↓
Native Provider
```

获得能力。

同理，Headless Service 不应被要求提供 Window/Menu/Tray。

因此 V3 的“Universal”含义是：

```text
同一套协议
同一套能力模型
同一套生命周期语义
同一套权限模型
同一套错误模型
同一套 Conformance

≠

所有宿主拥有相同 OS 能力
```

---

# 48. Universal Client Contract：从 TypeScript HostTransport 再抽一层

V2 已提出 Host-neutral `HostTransport`，但当前代码形态仍是 TypeScript：

```ts
invoke()
listen()
channel()
principal()
capabilities()
```

其中 `listen()` / `channel()` 明显带 Web/Tauri 心智模型。

如果要真正支持任意客户端，最终 canonical contract 不能是一个 TS interface，而必须是**语言无关协议定义**。

## 48.1 最终四层

```text
Language SDK
   ↓
Client Binding
   ↓
Universal Wire Protocol
   ↓
Transport Adapter
   ↓
Host Kernel
```

### Language SDK

可生成或实现：

```text
TypeScript
Rust
C ABI
C++
C#
Kotlin
Swift
Python        # 管理/自动化客户端可选
Go            # 服务/工具客户端可选
```

不是第一天全部维护，而是 Wire 稳定后按真实 Consumer 增加。

### Client Binding

语言层只做：

```text
typed request
typed response
stream binding
event binding
cancel
deadline
error mapping
capability mapping
```

绝不拥有第二份业务状态或生命周期状态。

### Universal Wire Protocol

只认识：

```text
handshake
request
response
event
stream
cancel
ack
capability
health
session
```

不认识：

```text
Tauri Channel
WebviewWindow
Electron ipcRenderer
Qt Signal
Swift NSWindow
```

### Transport Adapter

负责把 wire 映射到：

```text
Tauri IPC
Named Pipe
Unix Domain Socket
in-process channel
WebSocket
HTTP/2 stream
custom enterprise bus
```

## 48.2 Transport 接口不得暴露平台专属形状

建议内部 canonical trait 概念化为：

```text
MessageTransport
  connect()
  handshake()
  send(Frame)
  receive(Frame)
  close()
```

所有 request/event/stream/cancel 都是 Frame 语义。

Tauri `Channel` 只是 `TauriTransport` 的优化实现，不是 Contract。

这样才能真正让：

```text
Tauri
Electron
Qt
Native Mobile
Remote
Headless
```

共享一套协议。

---

# 49. 三种 Host 部署形态：不是所有客户端都嵌 Rust

任意客户端适配必须允许三种部署方式。

## 49.1 Embedded Host

```text
Client Process
  ├── Product
  └── Tauron Kernel
```

适合：

```text
Tauri
Rust Native
部分 C/C++ 宿主
```

优点：

```text
低延迟
少进程
低 IPC 成本
```

缺点：

```text
语言/FFI 集成要求高
Host 崩溃域与产品同进程
```

## 49.2 Local Host Service

```text
Client
  ↓ local authenticated IPC
Tauron Host Service
  ↓
Providers / Runtimes
```

适合：

```text
Electron
Qt
.NET
Flutter
Swift/Kotlin native shell
复杂企业客户端
```

这是 V3 推荐的第二条官方形态。

优点：

```text
客户端语言无关
Runtime 隔离更强
Host 可独立升级
大型 WASM/Process 能力不进入 UI 主进程
```

## 49.3 Remote Host

```text
Client
  ↓ authenticated remote transport
Tauron Gateway / Host
  ↓
Runtime / Provider
```

适合：

```text
Web
Mobile
Remote Workspace
Cloud Client
薄客户端
```

Remote 不能只是“Local Transport 换 URL”，必须额外具备：

```text
authentication
session
reconnect
replay protection
rate limit
transport encryption
remote resource policy
```

---

# 50. Principal V2：身份不能只分 Plugin / MainWindow

当前身份模型：

```text
plugin
main-window
invalid
```

对于任意客户端不够。

V3 统一：

```text
Principal {
  subjectId
  subjectKind
  hostInstanceId
  sessionId
  surfaceId?
  runtimeId?
  transportId
  authnLevel
  claims
}
```

`subjectKind`：

```text
application
surface
plugin
runtime
service
automation
remote-user-session
system
```

注意：

- `main-window` 不再因为“不是 plugin”就天然拥有最高权限；
- Privileged 权限由 PolicyEngine 判断；
- `surfaceId` 是上下文，不是权限本身；
- Custom Host 必须由 transport 证明 Principal，客户端 payload 不得自报；
- Remote transport 必须把认证 session 与 Principal 绑定；
- session 变化必须使旧 capability/grant token 失效或重新校验。

---

# 51. Surface Model：从 Window 扩展到任意交互面

V2 的 window cleanup 对桌面正确，但 Universal Client 不能假设所有客户端都有 Window。

统一：

```text
Surface {
  id
  kind
  owner
  lifecycle
}
```

`kind`：

```text
window
webview
tab
panel
native-view
headless
cli
service
remote-session
```

资源挂在：

```text
HostInstance
  └── Principal
       └── Surface
            ├── Subscription
            ├── Stream
            ├── Pending Call
            └── UI Contribution Binding
```

Surface 销毁自动级联释放资源。

这比“窗口关闭时调用 cleanup 函数”更通用，也能避免未来 Electron/Qt/移动端各自复制清理逻辑。

---

# 52. Host Instance 隔离：禁止隐式全局单例

工业级第三方库必须支持：

```text
同一进程多个 Tauron Host 实例
测试并行运行多个 Host
嵌入式产品创建/销毁 Host
```

新增：

```text
HostInstanceId
```

所有资源必须能追溯到：

```text
HostInstanceId
→ Principal
→ Plugin/Runtime/Surface
→ Resource
```

禁止：

- 隐式 process-global mutable registry；
- process-global current plugin；
- process-global current transport；
- 测试间共享状态；
- 多 AppHandle 意外共享权限/恢复记录。

真正的全局内容只允许是：

```text
immutable generated schema
immutable protocol constants
read-only static tables
```

---

# 53. Capability V3：从“命令是否存在”升级为可移植能力合同

V2 的 Capability V2 是正确方向，V3 再补三个工业化维度。

```yaml
id: filesystem.read
apiVersion: 2
status: full
provider: native-fs
transport: local
enforcement:
  level: hard
  isolation: process
constraints:
  maxBytes: 4194304
  roots:
    - workspace
quality:
  latencyClass: local
  persistence: durable
features:
  streaming: true
reason: null
```

新增字段：

```text
enforcement
constraints
quality
transport
maturity
```

`maturity`：

```text
stable
beta
experimental
```

`enforcement.level`：

```text
hard
soft
observed
none
```

非常重要：

> **不能因为 Manifest 写了 `memoryMb: 256` 就宣称宿主真的硬限制了 256 MiB。**

例如：

- Process Runtime + OS Job/cgroup 类机制：可能是 `hard`；
- 同进程 Builtin plugin：通常只能 `observed/soft`；
- Browser：某些系统能力 `none/unsupported`。

Capability 必须如实反映执行强度。

---

# 54. Protocol Handshake V2：所有客户端先协商再工作

连接建立：

```text
CONNECTING
  ↓
HELLO
  ↓
VERSION_NEGOTIATION
  ↓
AUTHENTICATED
  ↓
CAPABILITY_NEGOTIATION
  ↓
READY
```

建议 `HELLO` 包含：

```yaml
wireMin: 1
wireMax: 2
clientKind: electron
sdk:
  language: typescript
  version: 1.3.0
hostInstanceHint: null
features:
  - stream
  - cancel
  - resume
maxReceiveFrameBytes: 4194304
```

Host 回：

```yaml
wire: 2
sessionId: ...
hostInstanceId: ...
serverFeatures:
  - stream
  - cancel
  - resume
limits:
  maxFrameBytes: ...
  maxInflight: ...
```

没有共同 Wire 版本：

```text
拒绝连接
```

不能进入“先调用看看能不能用”的模糊状态。

---

# 55. Delivery Semantics：必须定义“消息到底会执行几次”

工业级 RPC 最大的隐患之一是断线重试。

V3 明确：

## 55.1 默认 Request

```text
at-most-once attempt
```

Transport 不得在不知道服务端是否执行成功的情况下自动重放非幂等命令。

## 55.2 可重试 Request

必须显式携带：

```text
idempotencyKey
operationClass = idempotent
```

Host 保留有界 dedupe window。

## 55.3 Event

缺省：

```text
at-most-once
```

状态类消息不要用 event 补历史，而使用：

```text
snapshot + version
```

## 55.4 Stream

每帧：

```text
streamId
seq
```

允许检测：

```text
duplicate
gap
out-of-order
```

但不默认承诺跨断线 exactly-once。

“Exactly once”不作为 Tauron 的泛化口号；只有具体服务能提供事务事实时才声明。

---

# 56. Session / Reconnect / Resume：Remote 和 Local Service 的必修课

当前桌面进程内 IPC 可以把断线等同 surface 销毁，但 Remote/Local Service 不行。

新增：

```text
SessionManager
```

Session 状态：

```text
NEW
AUTHENTICATED
ACTIVE
SUSPENDED
RESUMING
CLOSED
EXPIRED
```

断线时：

```text
transport lost
  ↓
session SUSPENDED
  ↓
bounded resume window
  ├── resume success → ACTIVE
  └── timeout → CLOSED → cascade cleanup
```

Resume 只恢复可恢复资源：

```text
subscriptions        可重新绑定
state snapshots      可重新同步
idempotent requests  可查询结果
streams              依 capability 决定
native window handle 不跨 host 恢复
```

每种资源必须定义：

```text
resumable: true/false
```

禁止“断线后所有 pending 永久留着等客户端回来”。

---

# 57. Deadline：避免跨机器时钟错误

Wire 不应依赖客户端自己给绝对 epoch deadline 作为唯一超时依据。

推荐：

```text
timeoutMs
```

Host 在接收瞬间转换为自身 monotonic deadline。

Remote 可额外带：

```text
clientSentAt
```

只用于诊断延迟。

这样避免：

```text
客户端时钟漂移
服务器时钟漂移
NTP 回拨
时区错误
```

影响超时语义。

---

# 58. Resource Ownership Tree：所有资源必须有父节点

V2 已要求所有资源有界，V3 再加“所有权树”。

```text
HostInstance
 ├── Session
 │    ├── Surface
 │    │    ├── Subscription
 │    │    ├── PendingCall
 │    │    └── Stream
 │    └── Principal
 ├── Plugin
 │    ├── Contributions
 │    ├── SettingsWatch
 │    └── Runtime
 │         ├── ProcessTree
 │         ├── Calls
 │         └── Streams
 └── Provider
```

规则：

> **任何动态资源创建时必须指定 Owner；Owner 销毁时资源自动级联销毁。**

如果一个资源不能回答：

```text
owner是谁？
什么时候释放？
异常路径谁释放？
```

就不能进入 production。

---

# 59. Plugin Dependency Graph：Manifest V3 必须补依赖模型

当前 Manifest 没有正式 dependencies 字段，无法工业化支持：

```text
插件 A 依赖插件 B
版本范围
optional dependency
冲突插件
循环依赖
启动顺序
反向卸载检查
```

新增：

```yaml
dependencies:
  required:
    - id: com.example.core
      version: "^2.0"
  optional:
    - id: com.example.chart
      version: ">=1 <3"

conflicts:
  - id: com.example.legacy
    version: "*"
```

新增：

```text
DependencyResolver
ResolvedPluginGraph
PluginLockfile
```

安装前：

```text
parse graph
  ↓
version solve
  ↓
conflict check
  ↓
cycle detection
  ↓
permission aggregate preview
  ↓
stage transaction
```

启动：

```text
topological order
```

停止/卸载：

```text
reverse topological order
```

依赖图循环：

```text
硬失败
```

不能靠 retry 试图“最终启动成功”。

---

# 60. Runtime Placement Resolver：Fallback 也必须防循环

V2 提出：

```text
local-webview
local-process
local-wasm
remote
```

V3 明确 placement 解析：

```text
requirements
  ↓
available capabilities
  ↓
security policy
  ↓
resource policy
  ↓
preferred placement
  ↓
single bounded fallback chain
```

每次调用只允许一个有限候选列表：

```text
wasm → process → remote
```

必须记录已尝试集合。

禁止：

```text
wasm fail → process
process fail → remote
remote fail → wasm
...
```

Placement failure 到达末尾后：

```text
ERRORED_USER_CONFIRM / UNSUPPORTED
```

---

# 61. Unified Scheduler：禁止模块各自无限 spawn/timer

工业级低内存和防死循环需要：

```text
TaskScheduler
```

统一承载：

```text
runtime heartbeat
retry backoff
pack download
update check
telemetry flush
maintenance GC
background reconciliation
```

Task metadata：

```text
owner
taskKind
deadline
priority
cancelToken
retryPolicy
resourceClass
```

原则：

- 没有 Runtime 时不运行 heartbeat worker；
- 不为每插件创建永久 timer；
- 不为每 provider 建独立线程池；
- 所有周期任务支持 disable；
- Host shutdown 先停止 Scheduler 接受新任务，再 drain；
- 任务必须归属 Resource Ownership Tree；
- 队列 count + bytes 有界；
- 同类任务支持 coalesce。

---

# 62. 内存治理：区分“统计、软限制、硬隔离”

V2 的 `maxRuntimeMemory` 需要进一步定义执行语义。

## 62.1 Builtin / 同进程 WebView

通常难以精确做单插件硬内存限制：

```text
enforcement = observed / soft
```

可：

- 统计；
- 限制 Tauron 自有 buffer/cache；
- 熔断请求；
- 拒绝新增资源。

但不能谎称 OS 级 hard quota。

## 62.2 Process Runtime

平台支持时：

```text
enforcement = hard
```

结合：

```text
Windows Job Object
POSIX resource/cgroup/provider-specific control
```

平台无法硬限制时：

```text
partial / observed
```

## 62.3 WASM

若引擎支持 memory/fuel/epoch 限制，可作为更强隔离域。

最终 Capability 必须公开：

```text
memoryLimit.enforcement
cpuLimit.enforcement
```

---

# 63. Parser / Payload 边界：限额必须发生在反序列化之前

工业级 DoS 防护不能只在对象进入 Kernel 后检查。

所有 ingress：

```text
Tauri IPC
Named Pipe
Unix Socket
WebSocket
sidecar stdout
WASM boundary
Pack download
Plugin archive
```

必须先执行：

```text
frame byte limit
header limit
field count limit
nesting depth limit
string length limit
binary payload limit
```

再做：

```text
JSON/Message decoding
```

否则攻击者可以在资源策略生效前就让 parser 分配巨量内存。

现有 `.tpkg` 已有条目数、单文件、解压总量、压缩比等限制，应保留并推广到所有 Pack/Wire 入口。

---

# 64. Install / Update Transaction V2：原子 rename 还不等于完整事务

当前插件安装已使用 temp dir + rename，这是正确基础。

但一次真实安装同时涉及：

```text
filesystem
ACL grants
registry
settings/schema
dependency graph
contributes metadata
runtime pack references
```

这些不是一个文件系统 rename 能原子覆盖的。

新增：

```text
InstallTransactionJournal
```

阶段：

```text
PREPARED
FILES_STAGED
FILES_COMMITTED
GRANTS_COMMITTED
REGISTRY_COMMITTED
ACTIVATION_STARTED
COMMITTED
```

启动时：

```text
scan incomplete journal
  ↓
deterministic reconcile
  ├── finish commit
  └── rollback
```

必须保证 crash 发生在任意一行之后，下一次启动都能收敛到：

```text
old version fully active
或
new version fully active
```

而不是半态。

---

# 65. 并发安装与更新：按资源加事务锁

必须防：

```text
两个窗口同时安装同一 plugin
自动更新与用户卸载同时发生
Pack GC 与 Pack activate 同时发生
进程退出与升级切换同时发生
```

新增：

```text
OperationCoordinator
```

锁粒度优先：

```text
plugin:<id>
pack:<content-hash>
runtime:<id>
```

避免全局大锁。

每个长事务：

- 锁内不做网络下载；
- 锁内不做用户 callback；
- prepare 在锁外；
- commit 使用短临界区；
- 冲突返回结构化 Busy/Conflict，不无限等待。

---

# 66. Supply Chain V2：签名之外还要防回滚与冻结攻击

现有 `.tpkg` Ed25519 + 文件 hash + issuedAt 是很好的基础，但第三方生态长期运行还需要：

```text
Root Trust
Key Rotation
Revocation
Repository Metadata Version
Anti-Rollback Counter
Metadata Expiry
Target Hash/Length
SBOM
Build Provenance
```

Pack/Plugin Repository 建议采用**TUF-like roles/metadata semantics**，不要求一定依赖某个具体实现，但必须具备以下性质：

```text
离线根信任
线上签名密钥可轮换
旧 metadata 不可无限回放
撤销后的 key 不再接受
目标版本有单调/策略约束
```

高价值发布可选增加：

```text
transparency log / external attestation
```

所有 release artifact 生成：

```text
SBOM
provenance
hash manifest
signature/attestation
```

---

# 67. Remote Transport Security

Remote Runtime / Web Client 一旦进入正式能力，必须独立安全设计。

最低要求：

```text
TLS
server authentication
short-lived session credential
token rotation
replay protection
request nonce/sequence
per-session rate limit
per-principal quota
origin/audience binding
session idle timeout
absolute max session age
```

企业模式可选：

```text
mTLS
enterprise identity provider
device binding
custom authorization provider
```

远程 transport 的 Principal 必须由认证层铸造，不能从 JSON payload 里的 `pluginId/userId` 信任。

---

# 68. Secrets V2：不仅“不放 Settings”，还要防泄漏到日志/内存快照

SecretProvider 需要附加：

```text
SecretHandle
```

优先避免把 secret 长期复制成普通 String。

规则：

- 日志自动 redact；
- Trace/Audit 不记录 secret value；
- error message 不回显 secret；
- IPC 只在必要路径传输；
- 可用时使用 zeroize/短生命周期 buffer；
- 插件只能读取被授予 scope 的 secret；
- 支持 revoke；
- provider health 不返回 secret metadata 之外的敏感信息。

---

# 69. Observability V2：可观测性自身也必须有资源预算

Telemetry 很容易成为新的内存/性能问题。

必须有：

```text
maxQueuedSpans
maxQueuedLogs
maxAttributeCount
maxAttributeBytes
maxCardinality
flushTimeout
dropPolicy
```

高基数字段：

```text
pluginId
runtimeId
errorCode
```

可以进入 label/tag。

禁止默认把以下作为 metric label：

```text
callId
URL full path
user input
request payload
```

否则会造成 cardinality 爆炸。

日志/Trace 必须做：

```text
PII redaction
secret redaction
payload sampling
```

默认 Noop 继续保持。

---

# 70. Crash Consistency：所有持久化状态都要可对账

需要建立统一的：

```text
StartupReconciler
```

启动时检查：

```text
install journals
runtime leases
pack activation pointer
plugin registry
grant store
recovery marker
settings schema version
cache pins
```

原则：

> **磁盘上的每份状态都必须能够从其它 canonical facts 验证，无法验证就进入 degraded/repair，而不是继续假装正常。**

持久化写入要求：

```text
temp
write
flush
fsync where required
atomic rename
parent directory durability where required
```

具体是否 fsync 由平台 Provider 实现，但 Contract 要区分：

```text
durable
best-effort
memory-only
```

---

# 71. 错误模型 V2：错误不仅要有 code/retryable

现有：

```text
code
message
retryable
```

继续扩展：

```yaml
code: E_RUNTIME_HANDSHAKE
category: runtime
retry:
  allowed: true
  strategy: backoff
blame: plugin
severity: error
operationId: ...
details:
  runtimeId: ...
remediation:
  action: reinstall-runtime
```

要求：

- `message` 可变，不作为机器判定；
- `code` 稳定；
- `details` 只放结构化非敏感字段；
- retryable 不等于“SDK 自动重试”；
- 自动重试仍必须过 operation class + RetryPolicy；
- 不允许任何模块通过字符串匹配错误信息驱动状态机。

---

# 72. 多实例 / 多用户 / 企业策略

Tauron 不负责业务“多租户”，但基础设施应支持**策略域隔离**：

```text
PolicyDomainId
```

例如：

```text
personal
workspace-A
workspace-B
enterprise-managed
```

用途：

```text
permission grant
secret namespace
plugin enablement
runtime quota
pack policy
update channel
```

不要把企业策略写死成应用全局单例。

策略继承：

```text
system policy
  ↓
organization policy
  ↓
application policy
  ↓
workspace/user grant
```

上层不能放宽被更高层明确禁止的能力。

---

# 73. Adapter Support Matrix：正式定义“支持”级别

不能只写“理论可适配”。

建议：

| Host | 官方 Adapter | 目标级别 | 备注 |
|---|---:|---|---|
| Tauri Desktop | 是 | Full | 首要参考实现 |
| Memory/Reference | 是 | Full-contract | 测试基准 |
| Local IPC Host Service | 是 | Full-contract | 任意 Native Shell 的通用桥 |
| Remote Gateway | 后续 | Extension | 需 Remote Security |
| Tauri Mobile | 后续 | Partial | Provider 能力按平台裁剪 |
| Electron | 社区/参考 | Conformant | 不强制官方长期维护 |
| Qt/C++ | 社区/参考 | Conformant | 推荐 Local Host Service/C ABI |
| .NET | 社区/参考 | Conformant | 推荐 Local Host Service |
| Flutter/React Native | 社区/参考 | Conformant | 推荐 native bridge/local/remote host |
| Browser | Gateway | Partial | 无本地 native provider |
| Headless Service | 是/参考 | Full-contract | 无 UI Provider |

术语：

```text
Official
Conformant
Experimental
Unsupported
```

只有通过对应 Conformance 才能写 `Conformant`。

---

# 74. SDK 策略：不要维护 N 套手写业务逻辑

多语言 SDK 应遵循：

```text
Schema/IDL
  ↓
generated low-level bindings
  ↓
thin ergonomic wrapper
```

核心：

```text
wire model
error model
capability model
manifest model
message envelope
```

全部生成。

手写部分只允许：

```text
language ergonomics
async abstraction
stream adapter
framework integration
```

这样 Swift/Kotlin/C#/TS 不会各自演化出不同生命周期。

---

# 75. API Maturity / Deprecation Contract

公共 API 每项必须声明：

```text
stable
beta
experimental
deprecated
internal
```

规则：

### stable

- SemVer 承诺；
- N-1 Compatibility；
- 有 Conformance；
- 有迁移文档。

### beta

- 可变；
- 变更必须 release note；
- 不承诺完整 N-1。

### experimental

- 默认可关闭；
- 不进入稳定 Compatibility Gate。

### deprecated

必须有：

```text
replacement
deprecatedSince
removalNotBefore
migration guide
```

禁止今天标 deprecated、下个 patch 就删除。

---

# 76. 工业级测试体系 V2

在现有 unit/contract/E2E 上新增。

## 76.1 Property / Fuzz

目标：

```text
Wire decoder
Manifest parser
Package/Pack verifier
Lifecycle state machine
Dependency resolver
Settings migration
Permission scope parser
Message correlation
```

必须验证：

```text
不 panic
资源有界
非法输入 fail-closed
状态机最终可收敛
```

## 76.2 Failure Injection

随机注入：

```text
disk full
permission denied
network disconnect
process crash
stdout malformed frame
pack download truncation
power-loss-like transaction interruption
clock skew
provider unavailable
session reconnect
```

## 76.3 Concurrency Stress

重点：

```text
install vs uninstall
disable vs invoke
runtime crash vs shutdown
stream close vs transport disconnect
window/surface destroy vs event publish
GC vs pack activation
```

## 76.4 Long-running Soak

至少覆盖：

```text
24h/72h optional nightly
repeated plugin enable/disable
reconnect cycles
runtime restart cycles
settings writes
event bursts
```

观察：

```text
RSS trend
handle count
thread count
fd count
pending count
orphan count
```

---

# 77. SLO / Release Quality

工业级不能只说“测试很多”。

建议为 Tauron Stable 定义至少：

```text
No orphan process: 100%
No unreconciled install transaction after restart: 100%
No permanent pending/stream after owner cleanup: 100%
Capability false-positive: 0
Crash-loop beyond retry budget: 0
Schema/codegen drift: 0
```

性能 SLO 在积累基线后锁定：

```text
startup p95
RPC p95/p99
shutdown p95
idle RSS
CPU idle
installer size
```

每次 release 附带：

```text
Compatibility Report
Conformance Report
Security Report
Performance/Size Diff
Known Limitations
```

---

# 78. V3 Round 1 — Universal Client / Architecture 审查

本轮在 V2 架构上继续检查“是否真的能适配任意客户端”。

## V3-R1-1 HostTransport 仍是 TypeScript/Tauri 形状

**发现**：当前 `Backend/HostTransport` 以 `invoke/listen/channel` 为核心，不能作为 Swift/Kotlin/C++ 等语言的 canonical contract。

**修复**：§48 Universal Wire Protocol；TS `HostTransport` 降为其中一个 Binding。

**Gate**：

```text
同一 wire fixture
→ Rust/TS/Memory/Local IPC 四端结果一致
```

## V3-R1-2 Principal 类型不足

**发现**：plugin/main-window 无法表达 service/headless/remote session。

**修复**：§50 Principal V2。

**Gate**：Kernel authorization API 不依赖 WebView label 类型；Tauri label 只在 Tauri Adapter 内转换。

## V3-R1-3 Window 是过强假设

**发现**：资源清理主要以窗口/label 心智模型组织，Headless/CLI/Remote 没有窗口。

**修复**：§51 Surface Model。

**Gate**：Headless fixture 无 WindowProvider 仍通过完整 lifecycle/resource cleanup conformance。

## V3-R1-4 没有明确 Embedded/Local Service/Remote 三种宿主方式

**修复**：§49。

**Gate**：至少 Embedded + Local IPC 两个 reference integration；Remote 在标 stable 前必须有第三个。

## V3-R1-5 Capability 缺执行强度

**风险**：软配额被误认为硬隔离。

**修复**：§53 `enforcement.level`。

**Gate**：Capability 文档不得宣称未验证的 hard isolation。

## V3-R1-6 多 Host 实例边界未正式定义

**修复**：§52 HostInstanceId。

**Gate**：同进程创建两个 MemoryHost/Kernel，权限、注册表、事件、恢复完全隔离。

**Round V3-1 结论**：完成这些设计后，“任意客户端”从营销表述变为可验证 Contract；进入 Round V3-2。

---

# 79. V3 Round 2 — End-to-End / Transaction / Reconnect 审查

## V3-R2-1 断线重试的执行次数未定义

**修复**：§55 Delivery Semantics + idempotency key。

**Gate**：故障注入在 response 返回前断线，非幂等请求不得被 SDK 自动重复执行。

## V3-R2-2 Remote/Local Service 没有 Session Resume

**修复**：§56 SessionManager。

**Gate**：断线 → resume / expiry 两条路径均能最终清零资源。

## V3-R2-3 Deadline 可能受跨机器时钟影响

**修复**：§57 timeoutMs → host monotonic deadline。

**Gate**：模拟 ±24h client clock skew 不改变 Host timeout 行为。

## V3-R2-4 Plugin Manifest 没有依赖 DAG

**修复**：§59 DependencyResolver/Lockfile。

**Gate**：

```text
cycle → hard fail
version conflict → hard fail
required dependency missing → hard fail
optional missing → explicit degraded
```

## V3-R2-5 Placement fallback 可能形成循环

**修复**：§60 bounded fallback + attempted set。

**Gate**：故障注入下每次 placement 尝试数 ≤ 候选数量。

## V3-R2-6 安装原子性只覆盖目录 rename

**修复**：§64 InstallTransactionJournal。

**Gate**：在每个事务阶段模拟 crash，重启后都收敛为 old 或 new 完整状态。

## V3-R2-7 安装/卸载/更新并发冲突

**修复**：§65 OperationCoordinator。

**Gate**：1000 次并发 fuzz 不出现半目录、双 registry entry、残留 grant。

## V3-R2-8 Signed package 缺 repository anti-rollback/freeze 模型

**修复**：§66 Supply Chain V2。

**Gate**：旧 metadata、revoked key、过期 metadata、版本回退均按 policy 被拒。

## V3-R2-9 Remote principal 缺强认证绑定

**修复**：§67 Remote Transport Security。

**Gate**：伪造 payload principal 不影响服务端认证 Principal。

## V3-R2-10 所有资源没有统一父级模型

**修复**：§58 Resource Ownership Tree。

**Gate**：随机销毁任意父节点，后代资源最终必须全部为 0。

**Round V3-2 结论**：正常、断线、升级、崩溃、并发事务均有确定终态；进入 Round V3-3。

---

# 80. V3 Round 3 — Industrial Reliability / Security / Performance 审查

## V3-R3-1 后台任务可能继续各模块自行 spawn/timer

**修复**：§61 TaskScheduler。

**Gate**：idle Tier S timer/task 数稳定；Runtime/Marketplace 未启用时无其 worker。

## V3-R3-2 内存配额语义可能过度承诺

**修复**：§62 区分 hard/soft/observed。

**Gate**：capability 与实际平台 enforcement 一致。

## V3-R3-3 大 payload 可能在解析后才被拒

**修复**：§63 pre-parse byte/depth/count limit。

**Gate**：fuzz 超大 payload 时 RSS 不突破 ingress budget。

## V3-R3-4 Telemetry 可能形成高基数和秘密泄漏

**修复**：§68/§69。

**Gate**：自动扫描日志/trace fixture，secret 不得出现；cardinality 有上限。

## V3-R3-5 持久化状态缺统一 startup reconcile

**修复**：§70 StartupReconciler。

**Gate**：随机删除/损坏单个状态文件后进入可诊断 degraded/repair，不 silent success。

## V3-R3-6 Error 模型不足以驱动自动恢复策略

**修复**：§71 Error V2。

**Gate**：状态机只依据 code/category/operation policy，不解析 message。

## V3-R3-7 企业策略隔离未建模

**修复**：§72 PolicyDomain。

**Gate**：上层 grant 不能突破 system/org deny。

## V3-R3-8 多语言 SDK 手写会漂移

**修复**：§74 generated binding + thin wrapper。

**Gate**：所有 binding 通过相同 wire golden fixtures。

## V3-R3-9 缺 API maturity

**修复**：§75。

**Gate**：Public API 清单每项都有 maturity；stable breaking diff 阻断 PR。

## V3-R3-10 缺 fuzz/failure/concurrency/soak 体系

**修复**：§76。

**Gate**：Stable release 必须完成 mandatory fuzz corpus + failure matrix；soak 可 nightly 但 release 前必须读取最新结果。

## V3-R3-11 缺工业级 SLO 产物

**修复**：§77。

**Gate**：Release artifact 必须附 Compatibility/Conformance/Security/Perf 报告。

## V3-R3-12 “任意客户端”容易被误写成零适配承诺

**修复**：§47/§73 重新定义支持等级。

**Gate**：文档只允许 `Official / Conformant / Experimental / Unsupported` 四类，不用未经 Conformance 的“支持”。

**Round V3-3 结论**：架构已具备向工业级底座演进所需的协议、事务、安全、资源、测试和治理闭环；是否“达到工业级”必须由实现后的 Gate 证明，不能仅由设计文档宣布。

---

# 81. V3 最终工业级架构

```text
                         Third-party Client
                                │
                  ┌─────────────┼─────────────┐
                  │             │             │
               TS SDK        Native SDK    Service SDK
                  │             │             │
                  └─────────────┼─────────────┘
                                ▼
                    Universal Client Contract
                 Wire / Error / Capability / Session
                                │
           ┌────────────────────┼────────────────────┐
           │                    │                    │
       Tauri IPC            Local IPC            Remote
           │                    │                    │
           └────────────────────┼────────────────────┘
                                ▼
                         Tauron Host Kernel
          ┌───────────────────────────────────────────────┐
          │ Policy / Registry / Lifecycle / Message      │
          │ Resource / Settings / Recovery / Dependency  │
          │ Session / Scheduler / Transaction / Observe  │
          └──────────────────────┬────────────────────────┘
                                 │
                    Runtime Placement Resolver
           ┌─────────────────────┼──────────────────────┐
           │                     │                      │
        WebView               Process                 WASM
           │                     │                      │
           └─────────────────────┼──────────────────────┘
                                 │
                          Provider / Adapter SPI
                                 │
                OS / Native / Enterprise / Remote
```

其中：

```text
Contract       决定“大家说同一种语言”
Kernel         决定“状态只有一份真相”
Policy         决定“谁能做什么”
Session        决定“连接断了怎么办”
Transaction    决定“崩一半怎么办”
Scheduler      决定“后台任务不会失控”
Resource       决定“内存/队列/流不会无限长”
Runtime        决定“不可信扩展在哪里执行”
Provider       决定“不同 OS 如何落地”
Conformance    决定“第三方适配是不是真的兼容”
```

---

# 82. V3 实施优先级重排

V2 的 A01-A32 保留，V3 增加并插入以下关键任务。

## P0 — 先把 Universal Contract 做实

```text
A33 Universal Wire Protocol schema
A34 MessageTransport canonical abstraction
A35 Principal V2 + CallerContext V2
A36 Surface Model + cascading cleanup
A37 HostInstanceId + multi-instance isolation
A38 Protocol Handshake / version negotiation
A39 Capability V3 enforcement/quality/maturity
A40 Embedded + Local IPC reference transport
```

只有 A33-A40 完成，才可以正式宣称：

> Tauron 的 Kernel/Contract 架构不再绑定 Tauri/TypeScript。

## P0/P1 — 再补端到端可靠性

```text
A41 Delivery semantics + idempotency
A42 Session resume/expiry
A43 monotonic deadline
A44 Resource Ownership Tree
A45 Plugin DependencyResolver + lockfile
A46 RuntimePlacement bounded resolver
A47 OperationCoordinator
A48 InstallTransactionJournal
```

## P1 — 工业安全与资源治理

```text
A49 Supply-chain repository metadata / anti-rollback / revocation
A50 Remote Transport Security
A51 TaskScheduler
A52 hard/soft/observed quota model
A53 ingress pre-parse resource limits
A54 Secrets redaction/short-lived buffers
A55 Telemetry cardinality/resource policy
A56 StartupReconciler
A57 Error Model V2
```

## P1/P2 — 可证明的多客户端支持

```text
A58 Adapter support matrix + Local Host Service
A59 generated language bindings + golden wire fixtures
A60 Stable API maturity/deprecation gate
A61 property/fuzz/failure/concurrency test matrix
A62 soak test + leak trend report
A63 release SLO/compat/conformance/security/perf reports
```

---

# 83. “可以达到工业级”的最终判定标准

Tauron 只有全部满足下列条件，才建议对外使用“工业级稳定底座”描述。

## Architecture

```text
[ ] 单 Kernel
[ ] 单 Contract source
[ ] Transport 不绑定 Tauri/TS
[ ] 多 Host 实例隔离
[ ] Principal/Surface/Session 模型完整
[ ] Resource Ownership Tree 完整
```

## Reliability

```text
[ ] crash-safe install/update transaction
[ ] reconnect 有终态
[ ] retry 有预算
[ ] placement fallback 无环
[ ] dependency graph 无环
[ ] no orphan runtime/process
[ ] no permanent pending/stream/subscription
[ ] startup reconcile 可恢复半态
```

## Security

```text
[ ] fail-closed auth/capability
[ ] signed packages
[ ] key rotation/revocation
[ ] repository anti-rollback/expiry
[ ] secrets redaction
[ ] remote replay protection
[ ] security hard gate
```

## Performance / Resource

```text
[ ] minimal profile 真正不链接重能力
[ ] idle 无无意义 worker/timer
[ ] count + byte quota
[ ] parser pre-limit
[ ] thread/fd/handle 有界
[ ] RSS/CPU/size regression gate
```

## Compatibility

```text
[ ] Wire version negotiation
[ ] N-1 stable compatibility
[ ] generated binding golden fixtures
[ ] stable API maturity gate
[ ] deprecation window
```

## Ecosystem

```text
[ ] Tauri Official Adapter
[ ] Memory Reference Host
[ ] Local IPC Reference Host
[ ] 至少一个非 Tauri Conformant Consumer
[ ] 至少三个真实第三方 Consumer
[ ] Conformance Kit public
```

## Operations

```text
[ ] metrics/log/trace/audit resource-safe
[ ] release provenance/SBOM
[ ] Compatibility Report
[ ] Conformance Report
[ ] Security Report
[ ] Performance/Size Report
```

---

# 84. V3 最终回答

## 是否可以适配任意客户端？

**架构上可以做到广泛适配，但必须把“任意客户端”定义为“能够实现 Universal Client Contract 并通过 Conformance 的宿主”。**

不承诺：

```text
所有客户端零代码接入
所有平台拥有完全相同 native capability
纯浏览器直接获得本地 process/fs 能力
```

承诺目标：

```text
同一协议
同一语义
同一权限
同一错误模型
同一生命周期
同一 Conformance
```

## 扩展性是否强？

完成 V3 P0 后，扩展点形成六个正交维度：

```text
Transport
Provider
RuntimeDriver
Language Binding
UI Framework Adapter
Plugin/Pack Ecosystem
```

新增 Electron/Qt/.NET/Swift/Kotlin/Remote 等宿主，不需要修改 Host Kernel 的业务事实源。

## 是否可以达到工业级？

**方案结构可以达到；当前代码不能因为有这份方案就直接宣称已达到。**

工业级必须以 §83 的 Gate 为证据。

当前正确阶段描述应是：

```text
v1.0.x:
Tauri-first extensible substrate with significant real wiring

V3 target:
host-neutral, language-neutral, crash-consistent,
resource-bounded, conformant industrial application substrate
```

最终方向保持：

> **不继续用“更多 command / 更多 crate”衡量进步，而以“第三方接入代码更少、契约更稳定、故障有终态、资源有上限、升级可回滚、非 Tauri Consumer 可通过同一 Conformance”为成熟度标准。**




# 85. V4 增量结论：V3 已很强，但仍不能直接宣布“任意客户端 + 工业级已完成”

V3 已经解决了最重要的结构性问题：

```text
Tauri ≠ Tauron
Universal Client Contract
Embedded / Local Host / Remote Host
Principal V2
Surface Model
HostInstanceId
Capability V3
Session / Resume
Resource Ownership Tree
DependencyResolver
OperationCoordinator
InstallTransactionJournal
Supply Chain V2
TaskScheduler
StartupReconciler
多语言 Binding
Conformance / SLO
```

这些方向正确。

本轮对 V3 与当前 `main@4e5c37f...` 再审查后，结论进一步收敛：

> **V3 已具备“工业级目标架构”的骨架；V4 要解决的是工业系统最容易在真实并发、生产默认、崩溃中间态、供应链、跨平台二进制、调用环、权限撤销和慢消费者上暴露的问题。**

所以 V4 不再继续横向增加产品功能，而是继续强化：

```text
production-safe defaults
wire framing/evolution
target compatibility
FFI ownership
execution affinity
service dependency graph
atomic call termination
re-entrancy/call-cycle control
credit backpressure
fair scheduling
policy epoch/revocation
settings commit consistency
review-token TOCTOU closure
installed-content integrity
local IPC peer authentication
generational activation
panic fault containment
durable-store corruption detection
streaming package verification
filesystem TOCTOU defense
network scope enforcement
reproducible builds
release evidence
```

---

# 86. Production Profile：开发默认与生产默认必须彻底分开

这是 V4 最重要的新修正之一。

当前代码中存在两个非常明确的生产风险：

```text
origin_allowlist = []
    → origin gate 被关闭
    → unknown/empty origin 也被放行

recovery_data_dir = None
    → recovery 只有内存态
    → 进程退出后失败计数丢失
    → safe mode 无法跨启动可靠生效
```

它们为了测试/兼容是合理的，但不能成为“工业级 Production 默认”。

## 86.1 DeploymentMode

新增：

```text
DeploymentMode
  ├── Development
  ├── Test
  └── Production
```

### Development

允许：

```text
memory-only recovery
explicitly disabled origin gate
mock/degraded provider
unsigned local development plugin
verbose diagnostics
```

但必须在 Capability/Doctor 中显示：

```text
productionSafe = false
```

### Test

允许：

```text
MemoryHost
fake clock
fake provider
fault injection
deterministic keys
```

测试能力不得进入正式发行 artifact。

### Production

必须执行：

```text
ProductionReadinessCheck
```

至少验证：

```text
[ ] origin / caller identity policy 已启用
[ ] recovery store 可持久化或明确声明 recovery=unsupported
[ ] plugin-install 开启时 trust root / signing key 配置完整
[ ] Secret capability 开启时不得落普通 settings
[ ] debug/mock provider 未启用
[ ] unsupported capability 不被宣传成 full
[ ] writable data dir 可用
[ ] audit sink 对高风险管理操作可用
[ ] package/cache 目录权限满足最小权限
[ ] Local Host endpoint 权限满足当前用户/服务范围
```

失败策略：

```text
security-critical missing
    → refuse READY

optional service missing
    → degraded capability
    → app can READY
```

禁止：

```text
生产配置错误
  → 静默回落到开发默认
```

---

# 87. Profile V2：把“能力档”与“UI 形态”彻底解耦

V3 的 Tier S 已经可裁剪，但部分描述仍默认：

```text
window
main surface
desktop shell
```

这会妨碍 Headless / Service / CLI。

V4 将配置拆成两个正交维度。

## 87.1 Product Tier

```text
S = Substrate
E = Extension
P = Platform
```

表示“基础设施深度”。

## 87.2 Capability Bundle

```text
core
desktop-ui
mobile-ui
headless
service
remote-client
plugin-runtime
marketplace
observability
enterprise-policy
```

例如：

```yaml
tier: S
bundles:
  - core
  - headless
  - service
```

与：

```yaml
tier: S
bundles:
  - core
  - desktop-ui
```

都是合法配置。

因此：

```text
Tier S != Window
Tier E != Desktop
Tier P != Marketplace UI
```

## 87.3 Readiness Predicate

不再硬编码：

```text
main surface mounted
```

统一：

```text
ReadinessSet
  ├── kernelReady
  ├── contractNegotiated
  ├── requiredProvidersReady
  ├── requiredRuntimesReady
  ├── requiredSurfacesReady[]
  └── productPredicates[]
```

Headless：

```text
requiredSurfacesReady = []
```

仍可达到 `ApplicationReady`。

---

# 88. Universal Wire V2：必须把“语义合同”和“字节传输”都定义清楚

V3 定义了 Universal Wire 的消息类型，但还缺工业级 framing 与 schema evolution 规则。

## 88.1 逻辑层

稳定语义：

```text
HELLO
AUTH
CAPABILITIES
REQUEST
RESPONSE
EVENT
STREAM_OPEN
STREAM_DATA
STREAM_END
CANCEL
ACK
HEALTH
GOAWAY
```

## 88.2 Framing 层

任何 Stream/Socket transport 必须有：

```text
fixed small header
validated length
frame kind
wire version
codec
flags
sequence
payload length
```

读取顺序：

```text
read fixed header
  ↓
validate magic/version
  ↓
validate payloadLength <= hardMaxFrameBytes
  ↓
allocate/read payload
  ↓
decode
```

禁止：

```text
先按对端声明长度分配巨大 Vec
再判断是否超限
```

## 88.3 Codec

V4 建议：

```text
mandatory control codec: json-v1
optional negotiated codec: cbor-v1 / future
binary payload: raw binary stream frame
```

核心原则：

```text
codec 可换
wire semantics 不换
```

所有官方 Binding 必须至少支持 `json-v1`，保证最低互操作性。

## 88.4 Unknown Field Policy

不同合同使用不同策略。

### Wire Core

核心 envelope：

```text
严格字段
+
extensions {}
```

扩展必须进入：

```text
extensions.<namespace>
```

禁止随意往顶层塞字段导致旧客户端解码失败。

### Manifest

继续保持强校验，但增加：

```text
manifestVersion
extensions
```

标准字段未知：

```text
reject
```

厂商扩展：

```text
extensions["com.vendor.xxx"]
```

### Capability

允许 additive feature key，但：

```text
required feature unknown
    → incompatible

optional feature unknown
    → ignore + keep descriptor
```

---

# 89. Error Registry V3：协议兼容不能依赖 Rust 枚举声明顺序

当前源码明确要求：

```text
ErrorCode 新码只能追加到 enum 末尾
因为 TS HOST_ERROR_CODES 按声明顺序比对
```

这是一种可以门禁住、但不适合作为长期公共协议的耦合。

V4 改为：

```text
contracts/error/error-codes.yaml
    ↓
codegen
    ├── Rust ErrorCode
    ├── TS ErrorCode
    ├── C ABI constants
    ├── Swift/Kotlin/C# constants
    └── docs
```

协议身份只由：

```text
stable string code
```

决定，例如：

```text
E_RUNTIME_HANDSHAKE
```

**不由 ordinal / source order 决定。**

兼容 Gate 比较：

```text
removed?
renamed?
semantic category changed?
retry class changed?
```

而不是：

```text
第 N 项是否还是第 N 项
```

---

# 90. Retry Semantics V3：`retryable: bool` 不足以保证安全

当前 `E_HOST_PANIC` 被标记为 `retryable=true`。

这是工业级风险：

```text
operation
  ↓
修改了一半状态
  ↓
panic
  ↓
catch_unwind → E_HOST_PANIC
  ↓
客户端看到 retryable=true
  ↓
再次执行
  ↓
重复副作用
```

`catch_unwind` 只能证明：

```text
进程没有因为这个 panic 直接退出
```

不能证明：

```text
事务已经回滚
状态仍一致
操作可安全重放
```

V4 删除公共语义上的简单 boolean，改为：

```text
RetryClass
  ├── Never
  ├── Manual
  ├── AutoIdempotent
  └── AfterReconnect
```

错误只提供“错误性质”。

是否真的重试还必须同时满足：

```text
Error.retryClass
AND
OperationClass
AND
IdempotencyState
AND
RetryBudget
AND
FaultDomain health
```

`E_HOST_PANIC` 默认：

```text
RetryClass = Never
```

只有具体操作通过：

```text
transaction rolled back
+
operation declared idempotent
+
fault domain recovered
```

才允许上层重新发起。

---

# 91. Target Contract：只写 win/mac/linux 不足以支撑任意客户端

当前 Manifest 平台只有：

```text
win
mac
linux
ios
android
```

对 Native/Process/Runtime Pack 不够。

同一个 `linux` 可能是：

```text
x86_64-gnu
aarch64-gnu
x86_64-musl
aarch64-musl
```

同一个 macOS 也有：

```text
x86_64
aarch64
minimum OS version
```

V4 新增：

```text
TargetSpec
```

示例：

```yaml
os: linux
arch: x86_64
abi: gnu
libc: glibc
minOsVersion: "..."
cpuFeatures: []
```

Artifact：

```yaml
artifacts:
  - target:
      os: windows
      arch: x86_64
    file: bin/win-x64/plugin.exe

  - target:
      os: macos
      arch: aarch64
    file: bin/macos-arm64/plugin
```

新增：

```text
ArtifactVariantResolver
```

选择顺序：

```text
exact target
  ↓
compatible target
  ↓
portable WASM
  ↓
remote placement
  ↓
unsupported
```

不能：

```text
找不到本机二进制
  → 随便挑一个同 OS artifact
```

---

# 92. FFI Contract：跨语言不能把 Rust 内存模型泄漏出去

V3 已提出 C ABI / C++ / C# / Swift / Kotlin Binding，但要真正工业化必须定义 FFI 所有权。

原则：

```text
不跨 FFI 暴露 Rust struct layout
不跨 FFI 暴露 Rust panic
不要求调用方使用 Rust allocator
```

建议 C ABI：

```text
opaque_handle
tauron_result_t
tauron_error_t
tauron_buffer_t
```

必须规定：

```text
谁分配
谁释放
释放函数是哪一个
callback 在什么线程调用
callback 可否 re-enter
handle 何时失效
shutdown 后调用返回什么
ABI version
```

例如：

```text
tauron_buffer_free()
tauron_error_free()
tauron_host_retain()
tauron_host_release()
```

所有高级语言 Binding：

```text
generated C/wire binding
  ↓
thin safe wrapper
```

不得每种语言自己解释生命周期。

---

# 93. Execution Domain：Provider 不是全部都能在任意线程执行

桌面平台大量能力具有线程亲和性。

例如：

```text
Window
Menu
Dialog
Clipboard
某些 WebView 操作
```

可能需要 UI/Main Thread。

而：

```text
filesystem
hash
archive
network
package verification
```

不应阻塞 UI Thread。

新增：

```text
ExecutionDomain
  ├── UiMain
  ├── HostAsync
  ├── BlockingIo
  ├── CpuBound
  └── RuntimeActor(runtimeId)
```

Provider 声明：

```text
executionDomain()
reentrant()
concurrencyLimit()
```

`ProviderDispatcher` 负责切换执行域。

硬规则：

```text
Kernel lock
  → release
  → dispatch provider
```

禁止：

```text
持 Kernel lock
  → await UI main thread
  → UI callback 再进 Kernel
```

这类路径非常容易形成桌面客户端死锁。

---

# 94. Provider Lifecycle / Capability Epoch：Provider 不能只有 ready / missing 两态

真实产品会发生：

```text
网络 Provider 掉线
系统权限被撤销
外接服务重启
Runtime Pack 被更新
企业策略变化
```

新增：

```text
ProviderState
  ├── REGISTERING
  ├── READY
  ├── DEGRADED
  ├── DRAINING
  ├── UNAVAILABLE
  └── DISPOSED
```

每次能力变化：

```text
CapabilityEpoch++
```

Client 持有：

```text
capabilitySnapshot(epoch=N)
```

新调用必须校验当前 epoch。

Provider 热替换：

```text
new provider prepare
  ↓
health
  ↓
publish epoch N+1
  ↓
new calls → new provider
old calls → old generation drain
  ↓
dispose old
```

禁止：

```text
直接替换全局指针
导致一半调用走旧、一半状态走新
```

---

# 95. Service Graph：启动和退出不能继续依赖手写固定顺序

V3 已定义粗粒度：

```text
Kernel
Providers
Runtime
Scheduler
Session
```

但服务数量增多后，手写顺序很容易产生孤儿逻辑。

新增：

```text
ServiceGraph
```

每个服务声明：

```text
requires
optional
startPhase
shutdownPhase
```

示例：

```text
PolicyEngine
  requires: Contract

RuntimeSupervisor
  requires:
    - PolicyEngine
    - ResourceManager
    - Scheduler

PluginManager
  requires:
    - RuntimeSupervisor
    - SettingsEngine
```

启动：

```text
topological start
```

退出：

```text
reverse topological shutdown
```

图出现环：

```text
启动前 hard fail
```

禁止：

```text
A start 等 B
B start 等 A
```

---

# 96. CallState：Cancel / Timeout / Result 必须只有一个终帧

V3 已有 cancel/deadline，但还必须解决并发竞态：

```text
runtime result arrives
       ↘
        race
       ↗
user cancel

timeout
       ↘
        race
       ↗
stream end
```

新增：

```text
CallState
  PENDING
    ├── COMPLETED
    ├── CANCELLED
    ├── TIMED_OUT
    └── FAILED
```

终态只允许通过一个原子 transition：

```text
compare-and-set(PENDING → TERMINAL)
```

失败的竞争方：

```text
只做幂等 cleanup
不得再发送第二个 terminal frame
```

Gate：

```text
10k cancel/result/timeout race
→ 每 call 恰好 1 个 terminal state
→ pending=0
→ stream=0
```

---

# 97. Call Graph / Re-entrancy：插件 A → B → A 是新的死循环来源

即使所有 retry 都有限，调用图本身也可能循环：

```text
A.invoke()
  ↓ waits B
B.invoke()
  ↓ waits A
A already waiting
```

新增：

```text
CallContext {
  rootCallId
  parentCallId
  hopCount
  visitedRuntimeIds
}
```

策略：

```text
maxHopCount
reentrantAllowed
cyclePolicy
```

默认：

```text
同步 request/reply:
    cycle detected → reject E_CALL_CYCLE

异步 event:
    可形成业务回路，但 EventBudget / queue limit 仍限制资源
```

不能把：

```text
event A → B → event B → A
```

当作“不是 retry 所以不会死循环”。

对事件流增加：

```text
eventHop
causationId
maxCausationDepth
```

---

# 98. Backpressure V2：只有队列上限还不够，还需要端到端 Credit

慢消费者问题：

```text
Process Runtime
  → Host
  → Remote Transport
  → Slow UI
```

如果 Host 只是不断把数据堆进 bounded queue：

```text
queue full
```

之后仍然需要明确：

```text
阻塞？
丢弃？
断开？
合并？
```

V4 为不同消息定义策略：

### RPC Result

```text
不得 drop
超限 → fail call / close unhealthy session
```

### State Update

```text
coalesce latest
```

### Telemetry

```text
drop/sample allowed
```

### Stream Data

采用：

```text
credit/window based flow control
```

Consumer：

```text
grant N bytes / frames
```

Producer 超过 credit：

```text
pause / apply runtime-specific backpressure
```

这样才能避免：

```text
一个慢 UI
拖垮整个 Host
```

---

# 99. Fair Admission：一个插件不能吃掉整个 Host 的资源额度

V3 有：

```text
count + byte limit
```

V4 增加层级预算：

```text
Host budget
  ↓
Session budget
  ↓
Principal budget
  ↓
Plugin budget
  ↓
Call/Stream budget
```

新增：

```text
AdmissionController
```

支持：

```text
weighted fair queue
per-owner concurrency
burst
sustained rate
priority ceiling
```

禁止插件把所有：

```text
pending slots
stream slots
worker tasks
HTTP connections
log budget
```

抢完。

系统/退出类任务可高优先级，但：

```text
plugin 不得自报 arbitrary priority=system
```

priority 由 Policy/Kernel 铸造。

---

# 100. Policy Epoch：权限撤销必须解决 TOCTOU

典型竞态：

```text
Call 已通过 permission check
   ↓
用户 revoke permission
   ↓
Call 正准备写文件
```

V4 新增：

```text
PolicyEpoch
GrantVersion
```

授权决策返回：

```text
DecisionToken {
  principal
  operation
  scope
  policyEpoch
  grantVersion
}
```

高风险 Provider 在执行副作用前：

```text
revalidate(token)
```

撤销策略按权限类型声明：

```text
new-calls-only
cancel-inflight
revalidate-before-side-effect
```

例如：

```text
filesystem.read
    → new-calls-only 可接受

process.spawn
    → spawn 前 revalidate

secret.read
    → value materialize 前 revalidate
```

禁止权限撤销后继续凭旧内存 snapshot 无限使用。

---

# 101. Settings V3：Watch 必须只观察已提交状态

V3 已解决 Watch 出口与内存复制。

V4 增加一致性：

```text
SettingsRevision
```

写事务：

```text
BEGIN revision=N
  ↓
validate schema
  ↓
apply all changes
  ↓
persist
  ↓
COMMIT revision=N+1
  ↓
publish watch events(revision=N+1)
```

Watcher 永远不能观察：

```text
key A 已更新
key B 还没更新
```

同一事务的半状态。

大对象事件仍使用：

```text
key
revision
hash
```

需要值时：

```text
getAtLeastRevision()
```

避免事件到了、读取却拿到旧值。

---

# 102. Install Approval V3：Preview 与 Commit 必须绑定同一份包

当前生产链路：

```text
install_preview(package_path)
  ↓
返回 plugin/name/version/permissions

用户审批

install(package_path, approved_permissions)
  ↓
重新读取 package_path
```

虽然 Commit 会重新验签并精确比较 permission set，但：

```text
Preview 时看到的包 A
```

与：

```text
Commit 时路径上的包 B
```

没有 cryptographic binding。

如果 B：

```text
由可信 publisher 签名
权限集合相同
但代码/版本内容不同
```

用户审批与最终安装内容就可能错位。

V4 新增：

```text
InstallReviewToken
```

包含：

```text
packageDigest
manifestDigest
publisherId
keyId
pluginId
version
permissionDigest
issuedAt
expiresAt
nonce
```

Preview：

```text
verify package
  ↓
produce reviewToken
```

Commit：

```text
re-verify package
  ↓
digest == reviewToken.packageDigest ?
  ↓
manifestDigest match ?
  ↓
permissionDigest match ?
  ↓
publisher/key match ?
  ↓
not expired ?
  ↓
commit
```

任何一项变化：

```text
E_REVIEW_STALE
```

必须重新审批。

---

# 103. Installed Content Integrity：验签一次不代表文件永远可信

安装后：

```text
.tpkg 已验签
  ↓
解压到 plugin directory
```

如果该目录后续被其它进程/用户修改：

```text
启动时只按路径打开
```

签名保证就被绕过。

V4 要求安装内容：

```text
content-addressed
immutable-by-policy
```

安装记录：

```text
ContentManifest
  path
  hash
  size
```

激活前：

```text
fast integrity policy
```

选择：

```text
1. read-only immutable store + trusted activation pointer
2. activation-time hash verify
3. OS-protected package location + periodic reconcile
```

高风险 native/sidecar：

```text
启动前必须验证实际执行文件 hash
```

不能只相信安装时的旧 hash。

---

# 104. Portable Archive Path：跨平台路径冲突要在安装前发现

跨平台包还要处理：

```text
A.txt
a.txt
```

在 Linux 可能共存，在默认 Windows/macOS 文件系统语义上可能冲突。

还包括：

```text
reserved names
Unicode normalization
trailing dot/space
ADS / reparse semantics
```

新增：

```text
PortablePathValidator(target)
```

包签名/解析阶段就检查目标平台语义。

不能：

```text
验签成功
→ 解压到一半
→ 才因为平台路径冲突失败
```

---

# 105. Local Host Broker：Local IPC 不能默认“本机就是可信”

V3 定义了 Local Host Service，但工业化还必须解决：

```text
谁启动服务？
两个客户端同时启动怎么办？
旧 socket / named pipe 怎么处理？
别的本机用户能不能连？
客户端如何确认连接的是自己的 Tauron Host？
Host 如何确认客户端是谁？
```

新增：

```text
LocalHostBroker
```

职责：

```text
single-instance lease
endpoint discovery
stale endpoint cleanup
peer authentication
instance secret/bootstrap token
protocol negotiation
graceful takeover/update
```

平台要求：

### Windows

```text
Named Pipe ACL
peer SID / process identity where available
per-user endpoint
```

### Unix

```text
Unix Domain Socket
0700 parent directory
peer credentials
socket ownership
no world-writable endpoint
```

原则：

```text
local != authenticated
```

---

# 106. Data Directory Ownership：多 Host 实例不能同时无协调写同一份状态

V3 要支持多 HostInstance。

但如果两个实例都指向：

```text
same plugin install dir
same recovery file
same grant store
same cache
```

会产生跨进程竞态。

新增：

```text
StorageNamespace
SingleWriterLease
SharedCacheLease
```

状态分为：

### Per Host Instance

```text
session
surface
pending
runtime lease
recovery boot marker
```

### Per Application Installation

```text
trusted keys
plugin registry metadata
pack cache
installed plugin content
```

共享写必须：

```text
file/process lock
or dedicated Local Host Service owner
```

禁止：

```text
两个独立 Kernel
同时写同一 registry/grant 文件
```

---

# 107. Generational Activation：升级过程中旧调用与新版本不能混线

Plugin/Provider/Runtime 更新必须引入：

```text
Generation
```

例如：

```text
plugin com.example@gen42
plugin com.example@gen43
```

切换：

```text
gen43 stage
  ↓
verify/health
  ↓
atomic activeGeneration = 43
  ↓
new calls → gen43
old calls → gen42
  ↓
gen42 drain
  ↓
GC
```

Call/Stream 绑定 generation：

```text
callId → runtimeGeneration
```

禁止：

```text
请求由旧 runtime 执行
结果却被新 runtime 的 pending table 接走
```

---

# 108. Pack / Cache Lease：GC 不得删除仍被运行中的 Host 使用的版本

V3 有 pin + LRU。

V4 进一步要求跨实例 Lease：

```text
PackLease {
  hostInstanceId
  packId
  version
  generation
  expiresAt / heartbeat
}
```

GC 条件：

```text
not active
AND
not rollback-pinned
AND
no live lease
AND
not transaction-staged
```

Host crash 后：

```text
lease expires / StartupReconciler confirms owner gone
```

再允许删除。

---

# 109. Panic FaultBoundary：catch_unwind 后不能假装系统还完全健康

`catch_unwind` 是必要的 API 兜底，但不是状态一致性证明。

新增：

```text
FaultBoundary
```

范围：

```text
Provider
Plugin Runtime
Settings Engine
Install Transaction
Kernel Command
```

panic 后：

```text
capture fault
  ↓
mark boundary FAULTED
  ↓
stop new work for that boundary
  ↓
attempt deterministic reconcile
  ├── clean → DEGRADED/READY
  └── unknown → QUARANTINED / repair
```

如果 panic 发生在：

```text
security-critical mutation
install transaction
grant mutation
registry mutation
```

且无法证明 rollback：

```text
不得自动 retry
```

必须：

```text
repair/reconcile first
```

---

# 110. RecoveryExecutor：现有幂等 RecoveryAction 不能继续停留在“只有单测引用”

当前代码已经诚实说明：

```text
RecoveryAction
EffectRecord
executed_actions
executed_effects
```

今天**没有生产调用方**。

V4 将其纳入：

```text
RecoveryExecutor
```

生产链：

```text
StartupReconciler
  ↓
derive RecoveryPlan
  ↓
RecoveryExecutor
  ↓
idempotency key check
  ↓
execute effect
  ↓
persist EffectRecord
  ↓
advance recovery state
```

Gate：

```text
在每个 effect 写入前/后 crash
重启执行
→ 外部副作用最多一次
或
→ 明确可重复且结果相同
```

如果暂时不接：

```text
删除/隐藏 public claim
```

不能继续有“类型存在 = 能力已实现”的孤儿逻辑。

---

# 111. DurableStore V2：不仅要有 StartupReconciler，还要能识别“状态文件本身坏了”

每份关键持久化记录统一 envelope：

```text
DurableEnvelope {
  schemaVersion
  generation
  writerInstance
  payloadHash
  createdAt
  payload
}
```

读取：

```text
parse
  ↓
schema check
  ↓
hash check
  ↓
generation check
  ↓
semantic reconcile
```

损坏：

```text
quarantine corrupt copy
  ↓
try backup / journal
  ↓
repair/degraded
```

禁止：

```text
JSON parse error
  → 当作“第一次运行”
  → 清空真实历史
```

关键状态至少：

```text
grant store
registry
recovery marker
install journal
pack activation pointer
settings version
```

需要：

```text
atomic write
generation
checksum/hash
backup/journal policy
```

---

# 112. Streaming Package Pipeline：低内存目标要求不再把整个 `.tpkg` 放进 RAM

当前安装链路：

```text
std::fs::read(package)
  ↓
Vec<u8> archive
  ↓
ZipArchive(Cursor<Vec<u8>>)
```

验签时还会对每个文件：

```text
Vec::with_capacity(file.size)
read_to_end
hash
```

虽然已有单文件/总量/压缩比限制，但这仍不是“低内存工业级安装器”。

V4 改为：

```text
PackageSource
  ↓ stream
StagingFile
  ↓
incremental archive reader
  ↓
incremental SHA-256
  ↓
bounded buffer
  ↓
verified extraction
```

目标：

```text
peak RAM ≈ fixed buffer + manifest + metadata
```

而不是：

```text
peak RAM ≈ archive size + largest unpacked file
```

要求：

```text
download size hard limit
compressed bytes hard limit
unpacked bytes hard limit
per-file limit
compression ratio limit
entry count limit
stream hash
stream copy
```

---

# 113. FileSystem Provider V2：canonicalize + starts_with 仍要防 TOCTOU

路径：

```text
canonicalize
  ↓
check within root
  ↓
open/write
```

在恶意并发环境下可能发生：

```text
check 后
symlink/reparse point 被替换
open 时已指向别处
```

工业级 `ScopedFsProvider` 应优先使用：

```text
directory handle / capability handle
no-follow
open relative to trusted dir
revalidate final handle
```

Unix 可采用能力式 `openat`/dirfd 思路。

Windows 需要处理：

```text
reparse points
junctions
final path validation
```

若平台无法完全硬防：

```text
capability.enforcement = partial
```

不得宣称 hard filesystem sandbox。

---

# 114. HttpProvider V2：domain scope 不能只做字符串前缀匹配

`network.domains` 权限需要定义：

```text
scheme
host
port
redirect policy
DNS resolution policy
private-network policy
proxy policy
TLS policy
```

最低规则：

```text
URL parse before policy
redirect 每一跳重新授权
host exact/wildcard grammar 固定
默认拒绝 file:// / custom dangerous scheme
DNS rebinding/private address 按 policy 处理
credential 不跨 origin 自动转发
```

Enterprise 可插入：

```text
NetworkPolicyProvider
```

---

# 115. Process Sandbox：进程树可回收 ≠ 进程已隔离

V3 的 Job Object/process group 解决：

```text
ownership
cleanup
resource accounting
```

但并不等于：

```text
sandbox
```

V4 明确：

```text
ProcessSandboxProvider
```

Capability：

```yaml
runtime.process:
  isolation:
    processTree: hard
    filesystem: partial
    network: partial
    syscall: unsupported
```

平台可实现：

```text
Windows Job / AppContainer-like policy where practical
macOS sandbox/provider-specific restrictions
Linux namespaces/seccomp/cgroup/provider-specific restrictions
```

Tauron 不承诺所有平台都有同等级 sandbox。

必须 Honest Capability。

---

# 116. Provider / Runtime Handle 防 ABA

任何 opaque handle：

```text
runtimeId
streamId
sessionId
surfaceId
providerId
```

都必须包含或关联：

```text
generation / nonce
```

避免：

```text
旧 handle 被释放
新资源恰好复用同 id
迟到消息误作用于新资源
```

推荐：

```text
opaque UUID / random id
+
hostInstanceId
+
generation where lifecycle reuse exists
```

收到 stale generation：

```text
E_STALE_HANDLE
```

---

# 117. Build Reproducibility：`Cargo.lock` 不等于整个构建已经可复现

当前 CI 仍使用：

```text
dtolnay/rust-toolchain@stable
actions/checkout@v4
actions/setup-node@v4
```

`Cargo.lock` 很重要，但：

```text
floating stable Rust
floating action tag
Node 22 major
```

仍会随时间变化。

V4 要求发布链：

```text
rust-toolchain.toml exact channel/version
exact Node release or centrally pinned image
pnpm exact
GitHub Actions pin immutable commit SHA
Cargo.lock / pnpm-lock
```

生成产物：

```text
禁止非必要构建时间戳
稳定文件排序
稳定 manifest serialization
SOURCE_DATE_EPOCH where applicable
```

Gate：

```text
same source + same toolchain
  → build twice
  → compare reproducible subset hashes
```

OS installer 如因平台签名时间戳不能 bit-identical：

```text
报告 non-reproducible field
而不是宣称 fully reproducible
```

---

# 118. Target Matrix CI：三 OS 还不够

工业级目标矩阵至少按：

```text
OS
×
arch
×
profile
×
runtime
```

例如：

```text
windows x86_64
windows arm64      # when supported
macOS x86_64
macOS arm64
linux x86_64 gnu
linux arm64 gnu    # when supported
```

移动端标 experimental 时单独：

```text
ios
android
```

每个不支持组合必须：

```text
explicit unsupported
```

而不是没跑 CI 却写“支持”。

---

# 119. Trusted Time Policy：签名/metadata 过期依赖系统时间，时钟异常要有明确语义

当前签名校验已经检查：

```text
issuedAt
max age
clock skew
```

这是安全加强。

但桌面设备系统时间可能严重错误。

V4 增加：

```text
TimeTrustState
  ├── Trusted
  ├── Suspicious
  └── Unknown
```

当系统时间明显异常：

```text
不能偷偷跳过 expiry
```

而应：

```text
supply-chain operation → blocked/degraded
diagnostic → CLOCK_UNTRUSTED
user/admin remediation
```

企业环境可提供：

```text
TrustedTimeProvider
```

---

# 120. Update + Settings Migration：代码回滚不代表数据一定能回滚

V2/V3 已有 plugin update rollback。

但如果新版本执行：

```text
destructive settings migration
```

之后 runtime 健康失败，旧版本可能读不了新数据。

V4 规定每个 migration 声明：

```text
reversible
forwardCompatible
requiresSnapshot
```

升级：

```text
snapshot old settings
  ↓
stage new plugin
  ↓
migration in transaction
  ↓
health probation
  ├── success → commit
  └── failure → rollback code + data
```

不可逆 migration：

```text
不能进入“可自动 rollback”的更新策略
```

必须：

```text
explicit user/admin acknowledgement
or
forward-compatible old reader
```

---

# 121. Event Ordering：不能让不同 Transport 自己猜顺序保证

V4 明确：

```text
global total order = 不承诺
```

最低保证：

```text
per stream sequence order
per sender→receiver request order（若 transport 支持并按协议实现）
state snapshot has monotonic revision
```

Event：

```text
eventId
causationId
producer
producerSeq
```

Consumer 可以检测：

```text
duplicate
gap
out-of-order
```

如果业务要求全局排序：

```text
由特定 Service 提供 sequencer
```

不能把它隐式塞进通用 EventBus。

---

# 122. Health 模型拆分：Liveness / Readiness / Degraded 不是一个 health()

Provider/Runtime/Host 统一：

```text
liveness
readiness
degradation
diagnostics
```

### Liveness

```text
进程/服务是否还活着
```

### Readiness

```text
是否接受新工作
```

### Degradation

```text
是否能工作但能力下降
```

例如：

```text
HTTP Provider
  liveness = true
  readiness = false
  reason = credential-expired
```

不能因为对象仍存在就宣称 ready。

---

# 123. Admin Authority：`main-window` 不再等于管理员

当前部分命令使用：

```text
require_main_window()
```

V3 已引入 Principal V2，V4 把迁移规则写死：

```text
surface kind != authorization role
```

最终：

```text
PolicyEngine.can(principal, adminOperation)
```

Tauri 默认主窗口可以由宿主配置授予：

```text
host-admin
```

但：

```text
secondary window
remote session
headless service
```

也可以按策略拥有或不拥有管理能力。

这能避免未来 Custom Host 为了调用管理命令伪装成“main-window”。

---

# 124. No-Lock-Across-Await：锁纪律需要从“建议”升级成硬规则

V3 已规定：

```text
禁止锁内 callback
锁序唯一
```

V4 再加：

```text
禁止跨 await / blocking provider call 持 Kernel mutex
```

优先模型：

```text
短锁
copy immutable snapshot
unlock
await
CAS/epoch commit
```

复杂单写状态优先：

```text
actor / serialized owner
```

而不是不断增加嵌套 mutex。

CI/static review Gate：

```text
核心模块 async fn 中
关键 lock guard 生命周期不得跨 await
```

---

# 125. 完整 Client ↔ Host ↔ Runtime/Provider 主链 V4

最终调用链：

```text
Client UI / Headless Caller
  ↓
Language SDK
  ↓
Client Binding
  ↓
Wire Encoder
  ↓
MessageTransport
  ↓
SessionManager
  ↓
Principal Resolver
  ↓
AdmissionController
  ↓
Protocol / Capability Epoch Check
  ↓
PolicyEngine
  ↓
CallState(PENDING)
  ↓
Message Router
  ├── Host Service
  ├── ProviderDispatcher
  └── RuntimeDriver
        ↓
      Runtime / OS / Remote
        ↓
      result
  ↓
CallState terminal CAS
  ↓
Wire Response / Stream End
  ↓
SDK Promise / callback
  ↓
Client
```

所有节点都有：

```text
owner
deadline
cancel
resource budget
trace context
error mapping
cleanup
```

错误链：

```text
任意节点失败
  ↓
structured error
  ↓
single terminal state
  ↓
release owner resources
  ↓
audit/metric
```

---

# 126. 插件安装 / 更新完整链 V4

```text
Source Resolve
  ↓
Target Variant Resolve
  ↓
Stream Download / Open
  ↓
Bounded Framing / Archive Limits
  ↓
Signature + Repository Metadata
  ↓
Publisher / Revocation / Anti-Rollback
  ↓
Portable Path Validation
  ↓
Manifest / Compatibility / Dependency DAG
  ↓
Permission Diff
  ↓
InstallReviewToken
  ↓
User/Admin Approval
  ↓
OperationCoordinator(plugin:<id>)
  ↓
InstallTransactionJournal
  ↓
Stream Verify + Extract
  ↓
Immutable Content Store
  ↓
Grant Commit
  ↓
Registry / Lockfile Commit
  ↓
Settings Migration Transaction
  ↓
Generation Stage
  ↓
Runtime Prepare / Handshake
  ↓
Health Probation
  ├── success → active generation switch
  └── fail    → code/data rollback
  ↓
Old Generation Drain
  ↓
Lease Release
  ↓
GC
```

任何步骤失败都有：

```text
明确 rollback / quarantine / user-confirm terminal
```

不存在：

```text
半安装目录
半权限
半 registry
半 migration
旧新 runtime 混用
```

---

# 127. Local Host Service 完整链 V4

```text
Client starts
  ↓
Broker discovery
  ↓
existing endpoint?
  ├── yes → peer verify
  └── no  → acquire single-instance lease
              ↓
           start service
              ↓
           publish protected endpoint
  ↓
HELLO/version
  ↓
peer auth
  ↓
Session ACTIVE
  ↓
Capability negotiate
  ↓
calls
```

Service crash：

```text
transport lost
  ↓
Session SUSPENDED
  ↓
bounded reconnect
  ↓
Broker confirms owner
  ↓
resume or expire
```

旧 endpoint：

```text
owner gone
  ↓
stale cleanup
```

不能：

```text
client connect 到任意同名 socket 就当成可信 Host
```

---

# 128. Shutdown V4：用 ServiceGraph + Resource Tree 双重保证无孤儿

```text
QuitRequested
  ↓
Application readiness=false
  ↓
Session GOAWAY / stop accepting new work
  ↓
Scheduler stop accepting
  ↓
AdmissionController reject new user work
  ↓
Call cancellation/drain with deadline
  ↓
Close streams
  ↓
Runtime generations drain
  ↓
Stop process trees
  ↓
Release pack/runtime leases
  ↓
Dispose plugin resources
  ↓
Dispose providers in reverse ServiceGraph order
  ↓
Flush durable audit/recovery within deadline
  ↓
Release LocalHost/DataDir leases
  ↓
assert ownership tree empty
  ↓
Exit
```

如果 graceful deadline 到：

```text
escalate
  ↓
force terminate owned runtime tree
  ↓
mark incomplete durable transaction for StartupReconciler
  ↓
exit
```

禁止：

```text
无限等某个 plugin/provider 自己退出
```

---

# 129. 防死循环 V4：所有可能形成环的地方逐项封死

| 环类型 | 终止机制 |
|---|---|
| Runtime crash retry | maxAttempts + backoff + user-confirm terminal |
| Pack download retry | retry budget + cancel token |
| Session reconnect | resume window + max attempts |
| Dependency graph | cycle detection → hard fail |
| ServiceGraph | cycle detection → boot hard fail |
| Runtime placement | attempted set + finite candidates |
| Migration | max steps + monotonic version |
| Call graph | max hops + visited chain |
| Event causation | max causation depth + queue budget |
| Update rollback | one bounded rollback path，失败进 repair |
| Provider reconnect | bounded policy + degraded terminal |
| Scheduler recurring task | explicit interval + cancellation + owner lifecycle |
| Recovery effect | idempotency key + persisted effect record |
| Local Host relaunch | single-instance lease + attempt budget |
| Credential refresh | bounded retry + auth-required terminal |

任何新增 loop 进入代码审查时必须回答：

```text
budget 是什么？
停止条件是什么？
terminal state 是什么？
cancel 从哪里来？
owner 销毁后 loop 如何停止？
```

答不出来：

```text
禁止合并
```

---

# 130. 防孤儿逻辑 V4：Public Surface Ledger

增加自动生成：

```text
PublicSurfaceLedger
```

每一项 public contract 必须记录：

```text
symbol / command / field / event
owner
producer
consumer
authorization
lifecycle
cleanup
test
maturity
```

例如：

```text
manifest.minAllowedVersion
```

当前代码已经明确：

```text
被解析
但 update path 没有消费
```

V4 规则：

```text
要么接入 UpdateResolver
要么标 reserved/experimental
要么删除
```

禁止：

```text
“先留个字段，以后可能用”
```

长期存在于 stable schema。

同理：

```text
RecoveryAction
EffectRecord
```

如果没有 production consumer：

```text
不允许在 CURRENT 能力表宣称已实现
```

---

# 131. 三轮新增审查结果：Round V4-1 — 通用性 / Owner / Production Defaults

本轮必须先把“任意客户端”真正从概念变成无桌面假设的基础合同。

## V4-R1-1 Production 默认存在 fail-open 兼容模式

**发现**

当前：

```text
origin allowlist empty → allow all
```

**修复**

§86 `DeploymentMode::Production + ProductionReadinessCheck`。

**Gate**

Production 下 origin/identity gate 未配置：

```text
Host 不得 READY
```

---

## V4-R1-2 Recovery durability 可以为 None

**发现**

当前 `recovery_data_dir=None` 时恢复状态不跨进程持久化。

**修复**

Production profile 要求 durability contract；若产品显式关闭 recovery，则 Capability 为 unsupported，而不是“看起来启用了 recovery”。

**Gate**

杀进程后重新启动，boot counter/safe-mode 结果符合 durable contract。

---

## V4-R1-3 Tier S 与 readiness 仍带桌面假设

**发现**

V3 仍残留 `main surface mounted` 和 Window-centric 描述。

**修复**

§87 Profile V2 + zero-surface readiness。

**Gate**

Headless reference host 无 WindowProvider 仍完整通过：

```text
boot → ready → invoke → shutdown
```

---

## V4-R1-4 Wire 只有语义、缺 framing/evolution

**修复**

§88。

**Gate**

随机切包、partial read、oversized length、unknown extension 全部有确定结果且 RSS 有界。

---

## V4-R1-5 Error contract 依赖声明顺序

**修复**

§89 generated stable error registry。

**Gate**

Rust enum 重排不改变 wire；删除/改名 stable code 直接阻断。

---

## V4-R1-6 平台只有 OS，无 arch/ABI/min OS

**修复**

§91 TargetSpec。

**Gate**

错误 target artifact 在启动前被拒，绝不执行到 OS loader 才失败。

---

## V4-R1-7 FFI 内存/线程所有权未定

**修复**

§92。

**Gate**

C ABI sanitizer/ASan fixture 验证 retain/release/buffer ownership；panic 不跨 FFI。

---

## V4-R1-8 Provider 缺 thread-affinity contract

**修复**

§93 ExecutionDomain。

**Gate**

UI-only provider 从 worker 调用时自动 marshal；持 Kernel lock 等 UI 被静态/测试拦截。

---

## V4-R1-9 服务启动/退出依赖手工顺序

**修复**

§95 ServiceGraph。

**Gate**

依赖环在启动前失败；关闭后 ServiceGraph node 全为 TERMINATED。

---

## V4-R1-10 main-window 仍被部分生产命令当作管理权限

**修复**

§123 policy-based admin authority。

**Gate**

Surface 类型变化不改变权限；只有 Policy grant 决定 admin。

---

**Round V4-1 结果**

全部问题已经获得：

```text
Canonical Owner
Contract
状态/阶段
失败出口
Gate
```

因此才进入 V4-R2。

---

# 132. Round V4-2 — 并发 / 事务 / Client↔Host↔Runtime 闭环

## V4-R2-1 Cancel / Timeout / Completion 存在竞争终帧风险

**修复**

§96 atomic CallState。

**Gate**

10k race：

```text
exactly one terminal state
```

---

## V4-R2-2 Plugin call graph 可形成 A→B→A 同步环

**修复**

§97 call graph / hop / cycle policy。

**Gate**

循环 request 在 bounded hop 内明确失败，不形成永久 pending。

---

## V4-R2-3 Event 也可形成业务回流死循环

**修复**

`causationId + causationDepth + budget`。

**Gate**

A/B 相互发布事件不会无限增长队列。

---

## V4-R2-4 慢消费者只有 queue limit，没有流控语义

**修复**

§98 credit-based flow control + per-message overflow policy。

**Gate**

慢客户端压测时 Host RSS/queue/thread 都有上限。

---

## V4-R2-5 一个插件可吃满全部 Host 配额

**修复**

§99 hierarchical AdmissionController。

**Gate**

恶意插件打满自身 quota，不影响 system/other plugin 最小服务预算。

---

## V4-R2-6 Permission revoke 与 in-flight side effect 存 TOCTOU

**修复**

§100 PolicyEpoch / DecisionToken。

**Gate**

revoke process.spawn 后，尚未真正 spawn 的请求必须被阻断。

---

## V4-R2-7 Settings Watch 可能观察到事务半态

**修复**

§101 revisioned commit + post-commit event。

**Gate**

多 key transaction 的 subscriber 永远看不到 mixed revision。

---

## V4-R2-8 Install Preview/Commit 未绑定包摘要

**修复**

§102 InstallReviewToken。

**Gate**

Preview 后替换 package path 内容，即使权限集合相同，也必须 `E_REVIEW_STALE`。

---

## V4-R2-9 安装后的内容可被外部修改

**修复**

§103 content-addressed immutable activation / activation hash check。

**Gate**

篡改已安装 sidecar 后，下一次激活前失败，不执行被改二进制。

---

## V4-R2-10 Local IPC 缺 peer auth / single-instance / stale endpoint 模型

**修复**

§105 LocalHostBroker。

**Gate**

另一 OS 用户/错误 endpoint 无法建立 authenticated session。

---

## V4-R2-11 多 HostInstance 写共享 data dir 会竞态

**修复**

§106 storage ownership / single-writer lease。

**Gate**

双进程并发启动不会同时成为同 registry 的 writer。

---

## V4-R2-12 升级新旧 generation 可能混线

**修复**

§107 generational activation。

**Gate**

旧 call 的 result 永远只结算到旧 generation 的 call context。

---

## V4-R2-13 Pack GC 与运行实例并发

**修复**

§108 PackLease。

**Gate**

持 lease 的 pack 绝不能被 GC。

---

## V4-R2-14 stale handle/late frame 可能误命中新资源

**修复**

§116 generation/nonce handle。

**Gate**

迟到旧 frame 对新 generation 返回 `E_STALE_HANDLE`。

---

**Round V4-2 结果**

客户端、Host、Provider、Runtime、Storage、Update 的并发链已经都有：

```text
identity
epoch/generation
owner
transaction
terminal state
cleanup
```

进入 V4-R3。

---

# 133. Round V4-3 — 故障一致性 / 低内存 / 安全 / 发布证明

## V4-R3-1 `catch_unwind` 后状态一致性未知

**修复**

§109 FaultBoundary。

**Gate**

panic fault injection 后 subsystem 不能继续以 READY 对外服务，除非 reconcile 证明一致。

---

## V4-R3-2 `E_HOST_PANIC` 当前可自动重试语义过强

**修复**

§90 RetryClass。

**Gate**

panic 默认 Never；只有显式 idempotent + reconciled operation 才可重新提交。

---

## V4-R3-3 RecoveryAction 幂等结构未进入生产链

**修复**

§110 RecoveryExecutor。

**Gate**

recovery 外部 effect crash/restart 测试无重复副作用。

---

## V4-R3-4 Durable state 损坏可能被当成空状态或普通失败

**修复**

§111 DurableEnvelope / checksum / generation / quarantine。

**Gate**

截断、bit flip、旧 generation fixture 均不能 silent success。

---

## V4-R3-5 `.tpkg` 整包/单文件内存加载违背低内存目标

**修复**

§112 streaming verification/extraction。

**Gate**

大包验证峰值 RSS 与 archive 大小基本解耦。

---

## V4-R3-6 Filesystem sandbox 存 check-use race

**修复**

§113 handle/capability-based scoped FS。

**Gate**

symlink/reparse race fuzz 不可逃逸允许根。

---

## V4-R3-7 Network domain policy 语义不足

**修复**

§114 redirect/DNS/private-network/TLS policy。

**Gate**

redirect 越出 scope、DNS rebinding/private target 按策略被拒。

---

## V4-R3-8 Process tree 不是完整 sandbox

**修复**

§115 ProcessSandboxProvider + honest enforcement。

**Gate**

Capability 不再把 process ownership 宣称为 filesystem/network/syscall sandbox。

---

## V4-R3-9 工具链仍有 floating version

**修复**

§117 reproducible build contract。

**Gate**

release provenance 记录精确 rust/node/pnpm/actions SHA。

---

## V4-R3-10 CI 只按 OS 仍不能证明 target compatibility

**修复**

§118 OS×arch×profile×runtime matrix。

**Gate**

所有 Official target 有真实 build/package/conformance evidence。

---

## V4-R3-11 系统时间异常可能影响供应链判定

**修复**

§119 TimeTrustState。

**Gate**

极端时钟偏移不允许通过“跳过 expiry”解决。

---

## V4-R3-12 数据 migration 与代码 rollback 可能不兼容

**修复**

§120 migration rollback contract。

**Gate**

update probation fail 后，旧版本能读取恢复后的旧数据。

---

## V4-R3-13 Event 顺序保证未正式定义

**修复**

§121 explicit ordering contract。

**Gate**

跨 transport golden fixture 对 ordering/duplicate/gap 语义一致。

---

## V4-R3-14 `health()` 容易把 alive 与 ready 混用

**修复**

§122 liveness/readiness/degradation 分离。

**Gate**

Provider alive-but-not-ready 不得进入 capability full/ready。

---

## V4-R3-15 锁纪律还缺“不得跨 await”

**修复**

§124。

**Gate**

核心 async 路径 lock guard 跨 await 检测/审查为 hard gate。

---

**Round V4-3 结果**

V4 新增问题全部已经在方案层面完成：

```text
Owner
Contract
State Machine
Failure Path
Termination Condition
Cleanup
Test Gate
Release Gate
```

没有留下“发现了但不知道谁修”的设计悬案。

---

# 134. V4 工业级 Definition of Done

任何 Stable 能力除了 V3 §39 的条件，还必须同时满足：

```text
[ ] Production mode secure-by-default
[ ] no desktop-only assumption in core contract
[ ] wire frame pre-allocation limit
[ ] schema evolution policy
[ ] target os/arch/abi compatibility
[ ] FFI ownership defined if exposed
[ ] execution/thread affinity declared
[ ] ServiceGraph acyclic
[ ] Call terminal state exactly once
[ ] synchronous call graph bounded / cycle-safe
[ ] event causation bounded
[ ] end-to-end backpressure policy
[ ] per-owner fair resource admission
[ ] permission revoke TOCTOU semantics
[ ] Settings post-commit watch semantics
[ ] InstallReviewToken binds preview to commit
[ ] installed content integrity retained after install
[ ] local IPC peer authenticated
[ ] shared persistent store has single-writer semantics
[ ] upgrade generation separation
[ ] pack/cache GC lease-safe
[ ] panic fault boundary has deterministic recovery policy
[ ] durable records detect corruption/stale generation
[ ] package verification/extraction uses bounded memory
[ ] filesystem scope resistant to symlink/reparse TOCTOU
[ ] network redirects/resolution respect scope
[ ] process isolation capability is honestly classified
[ ] build toolchain/provenance pinned
[ ] Official target matrix actually tested
[ ] migration rollback/data compatibility proven
[ ] liveness/readiness/degraded separated
[ ] no lock held across await/provider callback
```

少一项：

```text
不得标 stable/full
```

---

# 135. V4 实施顺序：不能把全部 A01-A100 同时开工

V4 保留 A01-A63。

新增任务：

```text
A64  DeploymentMode + ProductionReadinessCheck
A65  Profile V2：Tier 与 Capability Bundle 解耦
A66  zero-surface/headless ReadinessSet
A67  Wire framing + mandatory json-v1 codec
A68  schema extension/unknown-field policy
A69  generated Error Registry，取消 ordinal/order coupling
A70  RetryClass，E_HOST_PANIC 默认 Never
A71  TargetSpec + ArtifactVariantResolver
A72  C ABI / FFI ownership contract
A73  ExecutionDomain / thread-affinity dispatcher
A74  ProviderLifecycle + CapabilityEpoch
A75  ServiceGraph start/shutdown planner
A76  atomic CallState terminal transition
A77  CallGraph hop/cycle/re-entrancy policy
A78  event causation depth/budget
A79  credit-based stream backpressure
A80  hierarchical AdmissionController / fair scheduling
A81  PolicyEpoch / GrantVersion / DecisionToken
A82  SettingsRevision + post-commit watch
A83  InstallReviewToken
A84  immutable installed content + activation integrity
A85  PortablePathValidator
A86  LocalHostBroker + peer auth + single-instance lease
A87  StorageNamespace + SingleWriterLease
A88  generational plugin/provider/runtime activation
A89  PackLease / CacheLease
A90  stale-handle generation protection
A91  FaultBoundary / subsystem quarantine
A92  RecoveryExecutor production wiring
A93  DurableEnvelope/checksum/generation/quarantine
A94  streaming package verify/extract
A95  ScopedFsProvider no-follow / handle-based enforcement
A96  HttpProvider redirect/DNS/private-network policy
A97  ProcessSandboxProvider + enforcement descriptor
A98  pinned reproducible toolchain/actions/provenance
A99  OS×arch×profile×runtime target matrix
A100 TimeTrustState + TrustedTimeProvider SPI
A101 migration reversible/snapshot contract
A102 Event ordering contract
A103 liveness/readiness/degradation split
A104 no-lock-across-await gate
A105 PublicSurfaceLedger
A106 non-Tauri Local Host reference application
A107 FFI/C#/Swift/Kotlin golden conformance fixtures
A108 local/remote chaos + peer-auth test suite
A109 production security self-test / doctor
A110 release evidence bundle generator
```

## 135.1 实际推荐批次

### Batch 1 — 先封生产风险

```text
A64 A65 A66 A69 A70 A91 A92 A93 A109
```

目标：

```text
secure production defaults
panic 不误重试
recovery 真正可恢复
CURRENT 不再过度声明
```

### Batch 2 — 固化 Universal Contract

```text
A67 A68 A71 A72 A73 A74 A75
```

目标：

```text
非 Tauri Host 可以实现稳定 wire/target/FFI
```

### Batch 3 — 封并发与资源竞态

```text
A76 A77 A78 A79 A80 A81 A82 A90 A104
```

目标：

```text
无永久 pending
无调用环
无慢消费者拖垮 Host
无 revoke TOCTOU
无 lock-await deadlock
```

### Batch 4 — 封安装/升级/存储一致性

```text
A83 A84 A85 A87 A88 A89 A94 A101
```

目标：

```text
preview/commit 同一事实
低内存安装
升级 generation 隔离
crash-safe persistence
```

### Batch 5 — 扩跨宿主与安全隔离

```text
A86 A95 A96 A97 A100 A106 A107 A108
```

目标：

```text
Local Host Service 成为第二官方参考 Host
```

### Batch 6 — 发布证明

```text
A98 A99 A102 A103 A105 A110
```

目标：

```text
Industrial claim 有证据，不靠文档自我声明
```

---

# 136. V4 CI / Release Gate 增量

在 V3 §37 基础上新增：

```text
Production Config Gate
Headless Zero-Surface Conformance
Wire Framing Fuzz
Schema Evolution Compatibility
Error Registry Drift
Target Variant Resolution
FFI Ownership / Sanitizer
Thread Affinity Test
ServiceGraph Cycle Gate
Call Terminal Race Test
Call Graph Cycle Test
Event Causation Loop Test
Slow Consumer / Credit Backpressure
Fair Admission Stress
Permission Revocation Race
Settings Transaction Watch E2E
Install Review Token TOCTOU
Installed Artifact Tamper Test
Local IPC Peer Auth
Single Writer Storage Test
Generation Isolation Test
Pack GC Lease Test
Panic Fault Injection
Recovery Effect Idempotency
Durable Store Corruption Test
Streaming Installer RSS Gate
Filesystem Symlink/Reparse Race
HTTP Redirect/SSRF Policy Test
Process Sandbox Capability Honesty
Reproducible Build Report
Toolchain/Action Pin Gate
OS×arch×profile Matrix
Clock Trust Fault Injection
Migration Rollback E2E
Event Ordering Golden Fixture
Liveness vs Readiness Gate
No-Lock-Across-Await Gate
Public Surface Orphan Gate
```

---

# 137. V4 最终支持等级

正式对外不使用含糊的：

```text
“支持任意客户端”
```

改成：

### Universal Contract Compatible

宿主：

```text
实现 Universal Client Contract
+
通过 core conformance
```

### Tauron Conformant Host

再通过：

```text
identity
policy
resource
lifecycle
shutdown
failure
```

### Official Host

还必须：

```text
由 Tauron 项目维护 Adapter
进入 required CI matrix
有 release compatibility commitment
```

因此未来可以准确写：

```text
Tauri Desktop
  → Official / Full

MemoryHost
  → Official / Reference

Local Host Service
  → Official / Full-contract（V4目标）

Electron / Qt / .NET / Flutter
  → 可通过 Local Host Service 或自定义 Binding 达到 Conformant

Browser
  → Remote/Local Gateway Conformant，native capability 按能力诚实降级
```

---

# 138. “是否达到工业级”的最终判定

## 138.1 方案层面

完成 V4 后：

```text
架构覆盖面：工业级目标完整
扩展模型：强
跨宿主模型：完整
错误/恢复模型：完整
资源模型：完整
事务模型：完整
供应链模型：完整
测试/发布证明模型：完整
```

## 138.2 当前代码层面

仍必须明确：

```text
当前 main != V4 已实施
```

尤其当前仍可观察到：

```text
Tauri 2 是正式支持边界
origin empty allowlist = gate disabled
recovery durability 可关闭
RecoveryAction 幂等生产链未接
whole-package memory read
ErrorCode 顺序耦合 TS gate
E_HOST_PANIC = retryable
manifest target 只有 OS 粒度
cargo-deny 仍 advisory
main protection 未开启
```

所以当前阶段更准确的描述是：

> **Tauron 已有较强的 Tauri-first substrate 基础，并拥有一条完整的 Universal/Industrial V4 演进设计；工业级称号必须等 V4 的 P0/P1 Gates 真实变绿后再对外承诺。**

---

# 139. 最终推荐项目定位 V4

中文：

> **Tauron 是面向第三方多系统的 Host-neutral、Language-neutral 模块化 Application Substrate。它通过稳定的 Universal Wire Contract、Host Kernel、Policy、Capability、Runtime、Provider、Session、Transaction、Resource Ownership 与 Conformance，为 Desktop、Native、Headless、Service 和 Remote Client 提供可裁剪、低资源、可恢复、可审计、可升级和长期兼容的基础设施。**

英文：

> **Tauron is a host-neutral and language-neutral modular application substrate that provides stable contracts for capabilities, policy, runtimes, providers, sessions, transactions, resource ownership and conformance across desktop, native, headless, service and remote clients.**

最重要的边界仍然是：

```text
Tauron owns infrastructure mechanisms and contracts.

Product owns:
business domain
product workflow
business backend
product-specific runtime ownership
product UX decisions
```

---

# 140. V4 最终原则：以后每增加一个能力，都必须回答 12 个问题

任何 PR 增加 public capability 前必须填写：

```text
1. Canonical Owner 是谁？
2. Contract 在哪里？
3. 调用入口是谁？
4. 真实消费者是谁？
5. 身份从哪里来？
6. 权限在哪里检查？
7. 资源预算是什么？
8. 正常回收点在哪里？
9. 异常回收点在哪里？
10. Loop 的终止条件是什么？
11. 并发竞态如何收敛？
12. 哪条真实 E2E / Failure Gate 证明它？
```

任何一个回答：

```text
TODO
unknown
以后再说
```

则该能力最多只能是：

```text
experimental
```

不能进入：

```text
stable/full
```

这条规则是 V4 防止 Tauron 再次出现：

```text
有类型无入口
有入口无消费者
有状态无 Owner
有 Runtime 无退出
有 retry 无终态
有权限无撤销
有安装无恢复
有文档无真实能力
```

的最终治理闸门。

---

# 141. V4 最终验收矩阵

## Universal

```text
[ ] Core contract 无 Tauri/WebView/Window 必需类型
[ ] Tauri 只是官方 Adapter
[ ] Local Host Service 是第二参考 Host
[ ] Headless 可零 Surface 运行
[ ] TargetSpec 覆盖 os/arch/abi
[ ] C/Native FFI ownership 明确
```

## End-to-End

```text
[ ] SDK→Wire→Transport→Session→Kernel→Provider/Runtime→Response 全链
[ ] cancel/timeout/result 恰好一个终态
[ ] reconnect 后资源可恢复或明确释放
[ ] provider hot swap generation 不混线
[ ] upgrade generation 不混线
```

## No Orphan

```text
[ ] no pending orphan
[ ] no stream orphan
[ ] no subscription orphan
[ ] no task orphan
[ ] no runtime/process-tree orphan
[ ] no pack/cache lease orphan
[ ] no storage writer lease orphan
[ ] no contribution/grant/watch orphan
```

## No Infinite Loop

```text
[ ] retry bounded
[ ] reconnect bounded
[ ] placement bounded
[ ] dependency graph acyclic
[ ] service graph acyclic
[ ] call graph bounded
[ ] event causation bounded
[ ] migration bounded
[ ] recovery idempotent
[ ] updater rollback bounded
```

## Production Security

```text
[ ] Production default fail-closed
[ ] admin != main-window
[ ] permission epoch/revoke safe
[ ] preview/commit review token safe
[ ] installed artifact integrity safe
[ ] local IPC peer authenticated
[ ] FS scope resists symlink/reparse TOCTOU
[ ] HTTP scope covers redirect/resolution
[ ] secrets never enter logs/traces
[ ] supply chain anti-rollback/revocation
```

## Performance / Memory

```text
[ ] minimal core does not link optional heavy runtime
[ ] idle zero meaningless worker/polling
[ ] stream credit backpressure
[ ] fair per-owner admission
[ ] package verification bounded memory
[ ] parser pre-allocation hard limits
[ ] RSS/CPU/thread/fd/handle budgets
```

## Failure / Recovery

```text
[ ] panic → fault boundary, not blind retry
[ ] durable state checksum/generation
[ ] install/update crash consistency
[ ] recovery effect idempotency
[ ] data migration rollback contract
[ ] StartupReconciler deterministic
```

## Release Proof

```text
[ ] pinned toolchain/actions
[ ] reproducible-build report
[ ] OS×arch×profile test matrix
[ ] External Consumer
[ ] non-Tauri reference Consumer
[ ] N-1 compatibility
[ ] security hard gate
[ ] Conformance report
[ ] performance/size report
[ ] known limitations
```

全部满足之后，再正式对外使用：

> **Industrial-grade Universal Application Substrate**



## Appendix A：首批可直接实施任务

```text
A01  删除 tauron-shell legacy state engine，改 canonical facade
A02  统一 Wire/Command Schema
A03  adapter features 重新拆分，default=minimal core
A04  proc/wasm/market/distribute 变 optional
A05  建立 substrate/extension/platform 三 profile
A06  Tauri plugin permissions 自动生成
A07  plugin namespace 形态成为默认接入
A08  Capability V2 + fail-closed negotiation
A09  InstallationIdentityProvider
A10  AppLifecycleBinding 自动 cleanup/recovery ready/quit drain
A11  Settings Watch 接 Message Plane
A12  SecretProvider
A13  Message completion push，移除正常调用轮询
A14  Event Approval Broker
A15  RuntimeDriver
A16  ProcessStatus 三态
A17  Windows Job Object / POSIX process group
A18  Runtime handshake + Supervisor
A19  ProcIoDriver shared async reactor
A20  真 sidecar fixture + 三平台 E2E
A21  ResourcePolicy count + bytes
A22  Settings Watch VecDeque/coalesce/Arc
A23  Runtime Pack format + signed install transaction
A24  WASM engine on-demand pack + WIT
A25  AppUpdate/PluginUpdate/PackUpdate 分域
A26  Schema/Codegen 扩展到 permissions/docs/WIT
A27  MemoryHost reference implementation
A28  Pre-publish external consumer 三平台
A29  Perf/RSS/installer-size baseline + regression gate
A30  cargo-deny 高危 hard gate
A31  main branch required checks/ruleset
A32  legacy iframe demo 移出主 quickstart
A33  Universal Wire Protocol schema
A34  MessageTransport canonical abstraction
A35  Principal V2 + CallerContext V2
A36  Surface Model + cascading cleanup
A37  HostInstanceId + multi-instance isolation
A38  Protocol Handshake / version negotiation
A39  Capability V3 enforcement/quality/maturity
A40  Embedded + Local IPC reference transport
A41  Delivery semantics + idempotency
A42  Session resume/expiry
A43  monotonic deadline
A44  Resource Ownership Tree
A45  Plugin DependencyResolver + lockfile
A46  RuntimePlacement bounded resolver
A47  OperationCoordinator
A48  InstallTransactionJournal
A49  Supply-chain anti-rollback / revocation / repository metadata
A50  Remote Transport Security
A51  TaskScheduler
A52  hard/soft/observed quota model
A53  ingress pre-parse resource limits
A54  Secrets redaction / short-lived buffers
A55  Telemetry cardinality/resource policy
A56  StartupReconciler
A57  Error Model V2
A58  Adapter support matrix + Local Host Service
A59  generated language bindings + golden fixtures
A60  Stable API maturity / deprecation gate
A61  property/fuzz/failure/concurrency test matrix
A62  soak/leak trend report
A63  release SLO/compat/conformance/security/perf reports
A64  DeploymentMode + ProductionReadinessCheck
A65  Profile V2：Tier 与 Capability Bundle 解耦
A66  zero-surface/headless ReadinessSet
A67  Wire framing + mandatory json-v1 codec
A68  schema extension / unknown-field policy
A69  generated Error Registry，取消 declaration-order coupling
A70  RetryClass，E_HOST_PANIC 默认 Never
A71  TargetSpec + ArtifactVariantResolver
A72  C ABI / FFI ownership contract
A73  ExecutionDomain / thread-affinity dispatcher
A74  ProviderLifecycle + CapabilityEpoch
A75  ServiceGraph start/shutdown planner
A76  atomic CallState terminal transition
A77  CallGraph hop/cycle/re-entrancy policy
A78  event causation depth/budget
A79  credit-based stream backpressure
A80  hierarchical AdmissionController / fair scheduling
A81  PolicyEpoch / GrantVersion / DecisionToken
A82  SettingsRevision + post-commit watch
A83  InstallReviewToken
A84  immutable installed content + activation integrity
A85  PortablePathValidator
A86  LocalHostBroker + peer auth + single-instance lease
A87  StorageNamespace + SingleWriterLease
A88  generational plugin/provider/runtime activation
A89  PackLease / CacheLease
A90  stale-handle generation protection
A91  FaultBoundary / subsystem quarantine
A92  RecoveryExecutor production wiring
A93  DurableEnvelope / checksum / generation / quarantine
A94  streaming package verify/extract
A95  ScopedFsProvider no-follow / handle-based enforcement
A96  HttpProvider redirect/DNS/private-network policy
A97  ProcessSandboxProvider + enforcement descriptor
A98  pinned reproducible toolchain/actions/provenance
A99  OS×arch×profile×runtime target matrix
A100 TimeTrustState + TrustedTimeProvider SPI
A101 migration reversible/snapshot contract
A102 Event ordering contract
A103 liveness/readiness/degradation split
A104 no-lock-across-await gate
A105 PublicSurfaceLedger
A106 non-Tauri Local Host reference application
A107 FFI/C#/Swift/Kotlin golden conformance fixtures
A108 local/remote chaos + peer-auth test suite
A109 production security self-test / doctor
A110 release evidence bundle generator
```

## Appendix B：建议文档保存路径

```text
docs/Tauron-Universal-Industrial-Application-Substrate-Final-Architecture-V4.md
```

后续拆分：

```text
docs/architecture/CURRENT.md
docs/architecture/kernel.md
docs/architecture/runtime.md
docs/architecture/security.md
docs/architecture/performance.md
docs/architecture/distribution-packs.md
docs/contracts/
docs/integration/
docs/roadmap/NOW.md
docs/roadmap/NEXT.md
docs/history/
```

最重要的文档规则：

> **CURRENT 只写当前真实已实现状态；ROADMAP 只写未完成；历史方案进入 history。不得再把架构愿景与当前可用能力混在同一张能力表中。**