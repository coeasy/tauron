# Tauron 项目架构、真实实现与竞品对比：V5 改进优化方案

> 审计日期：2026-10-03  
> 仓库：`coeasy/tauron`  
> 审计基线：`main @ 305e2d68f95bc8f3a3c8c182cce8ab7c830f7bef`  
> 最新基线 CI：GitHub Actions run `37093152277`，全部 12 个主要 Job 通过  
> 文档性质：**基于当前代码真实接线情况的架构审计与后续优化方案**。凡“规划”“模拟”“参考实现”和“生产可用”均分开表述，不按设计文档中的目标能力反推代码实现。

---

## 1. 结论摘要

### 1.1 当前 Tauron 到底是什么

当前 Tauron 最准确的定位不是“已经完成的通用工业级应用底座”，而是：

> **Tauri 2 Desktop First 的插件化应用 Substrate，已经具备较强的安全治理、插件生命周期、事件/调用协议、配置/恢复、安装信任链与跨语言契约门禁；但 Universal Host、统一 Runtime、真实 WASM、插件升级/回滚、生产 Process 隔离、完整可观测性和真实生态闭环仍未完成。**

从代码成熟度看，Tauron 已经越过了“Demo / 概念验证”阶段。其优势不是简单“模块很多”，而是已经建立了比较少见的工程约束体系：

- `tauron-host` 被明确为 canonical kernel；
- `SubstrateState` / `PluginRuntimeState` 在类型层分离底座与插件运行时；
- Production 启动采用 fail-closed readiness；
- `origin gate + Caller principal + command-level guard` 构成实际授权路径；
- 85 条 `host_*` 命令可以由代码自动生成接口参考，并检查孤儿命令；
- EventBus 已具有审批、撤销、队列作废、ordering watcher；
- 插件安装已经有“签名包 → preview → digest 绑定 review token → approved permissions → install”的信任链；
- Process 调用已经有真实 `stdin/stdout` JSON-RPC 帧回路、PID/generation fencing、进程树回收基础；
- CI 已覆盖 Linux / Windows / macOS arm64 / macOS x64、cargo-deny、格式、Clippy、Rust/TS 测试、性能/体积预算与 target matrix。

因此，下一阶段不应该继续“横向增加更多 feature”，而应该优先完成 **架构收敛 + 真实 Runtime 闭环 + 第二 Host + Package Manager 事务化**。

### 1.2 当前最关键的六个问题

1. **Canonical 尚未真正收敛**  
   Rust 侧 `tauron-shell` 仍保存 legacy `HostState / PluginRegistry / EventBus`；TS 侧 `@tauron/core` 仍被 React/Vue/Svelte adapter 消费，同时又存在新的 `@tauron/host`。现在靠“冻结 + 门禁”避免继续分叉，但双栈仍然真实存在。

2. **Runtime 形态声明大于可执行能力**  
   当前四个 `PluginType` 中：
   - JS：有真实调用路径；
   - Process：有真实 transport / supervisor 基础，但缺真 sidecar E2E，Production 默认又因无 Hard sandbox provider 而 fail-closed；
   - WASM：只有配置/ABI/崩溃预算层接线，执行仍是模拟或显式 Unsupported；
   - Rust Native：没有动态执行器。

   因此“4 类插件 Runtime”目前不能作为同等级能力对外宣传。

3. **Tauri-independent kernel 已存在，但 Tauri-independent product path 未成立**  
   `tauron-host` 本身平台无关，Local/Remote Host contract 也已经有不少实现；但 v1 官方支持边界仍只有 Tauri 2 Desktop。Local Host 仍是 reference/example 级，而不是第二个官方可分发 Host。

4. **Extension 安装安全基础不错，但 Package Lifecycle 尚不完整**  
   安装已比较扎实；但缺少：
   - Manifest V3；
   - dependency resolver / lockfile；
   - immutable generation store；
   - plugin update；
   - rollback；
   - health-gated activation；
   - registry/catalogue 协议闭环。

   现在 `host_market_*` 仍有 simulated 路径，而 app updater 又由 `tauron-distribute` 单独承载，两个概念边界容易混淆。

5. **适配层过于集中**  
   当前：
   - `crates/tauron-adapter/src/lib.rs` 约 **16,959 行**；
   - `crates/tauron-adapter/src/tauri.rs` 约 **4,114 行**；
   - `tauron-host/src/registry.rs` 约 **3,082 行**；
   - `tauron-host/src/eventbus.rs` 约 **2,147 行**。

   功能虽然有测试兜底，但代码审阅成本、冲突概率、领域耦合、回归定位成本已经明显上升。

6. **“机械一致性文档”很强，但“语义状态文档”仍会漂移**  
   当前 command surface / version / 文档行号已经自动校验，但仍能看到：
   - `architecture/overview.md` 的 crate 接线状态滞后于当前 Cargo 依赖；
   - `canonical-owners.md` 对 brand/theme/distribute 的状态存在旧口径；
   - 旧 competitive analysis 仍使用 15 crates / 20 packages 等历史数字；
   - 当前真实根目录已经是 16 Rust crates / 21 TS packages。

   说明现有 docs gate 主要防“格式和引用漂移”，还没有完全解决“能力成熟度语义漂移”。

---

# 2. 仓库当前结构与事实基线

## 2.1 Rust Workspace

当前 `crates/` 共 16 个：

| Crate | 当前角色 | 实际成熟度 |
|---|---|---|
| `tauron-host` | canonical kernel：registry、eventbus、authz、runtime lease、stream、production、Local/Remote Host contract | **核心已落地** |
| `tauron-adapter` | 应用层装配、Tauri 命令、provider、插件安装、Process/WASM delivery | **核心已落地，但过度集中** |
| `tauron-proc` | Process spawn / kill / stdin/stdout frame / sandbox provider | **实现较深，缺真实 sidecar E2E** |
| `tauron-wasm` | WASM config/cache/pool/crash tracking/模拟 execute | **状态层真实，执行层未完成** |
| `tauron-settings` | SettingsStore/schema/migration | **已接 canonical 路径** |
| `tauron-recovery` | boot/recovery/safe-mode | **已接线** |
| `tauron-acl` | install ACL / grant 相关能力 | **feature-gated 接线** |
| `tauron-market` | 包签名/安装相关市场层能力 | **安装链部分接线** |
| `tauron-distribute` | app update/distribution | **updater 已出现真实接线** |
| `tauron-brand` | Brand config | **当前代码已接 `host_brand_info`** |
| `tauron-theme` | Theme registry | **当前代码已接 theme commands** |
| `tauron-i18n` | i18n | **已接线** |
| `tauron-notify` | notification | **已接线** |
| `tauron-schema` | schema | **被 settings 等传递消费** |
| `tauron-ffi` | FFI / 多语言接口基础 | **有测试，但真实第三方消费仍不足** |
| `tauron-shell` | 旧 framework 协议面 + legacy registry/eventbus/HostState | **冻结 legacy，尚未删除** |

## 2.2 TypeScript Workspace

当前 `packages/` 共 21 个。可以大致分为：

### Consumer / Host API

- `@tauron/host`
- `@tauron/types`
- `@tauron/ui`
- `@tauron/ui-primitives`
- `@tauron/shell-events`

### Plugin SDK

- `@tauron/plugin-sdk`
- `@tauron/app-plugin-sdk`
- `@tauron/plugin-context-contract`

### Legacy / Framework Core

- `@tauron/core`
- `@tauron/dual-world`
- `@tauron/shell-matrix`

### Framework Adapters

- `@tauron/adapter-react`
- `@tauron/adapter-vue`
- `@tauron/adapter-svelte`

目前三个 UI adapter 仍直接依赖 `@tauron/core`，这说明 TS canonical 收敛还没有完成。

### Tooling / Contract

- `@tauron/app-cli`
- `create-tauron-app`
- `@tauron/cli`
- `@tauron/app-contract-kit`
- `@tauron/contract-tests`
- `@tauron/market`
- `@tauron/framework`

---

# 3. 当前真实架构

## 3.1 逻辑架构图

```text
┌──────────────────────────────────────────────────────────────┐
│                   Application / Product                      │
│ React / Vue / Svelte / Vanilla TS                           │
└──────────────────────────────┬───────────────────────────────┘
                               │
           ┌───────────────────┴───────────────────┐
           │                                       │
           ▼                                       ▼
 @tauron/adapter-*                         @tauron/host
      │                                         │
      ▼                                         │
 @tauron/core  ← legacy framework path          │
      │                                         │
      └─────────────────────┬───────────────────┘
                            ▼
                    HostTransport / IPC
                            │
                            ▼
┌──────────────────────────────────────────────────────────────┐
│                    tauron-adapter                            │
│                                                              │
│  TauriCallerSource → Origin Gate → Command Guard             │
│          │                     │                             │
│          ▼                     ▼                             │
│  SubstrateState         PluginRuntimeState                   │
│          │                     │                             │
│          └──────────────┬──────┘                             │
│                         ▼                                    │
│             domain commands / providers                      │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│                 tauron-host canonical kernel                 │
│ Registry / AuthZ / EventBus / Lifecycle / Stream /           │
│ Admission / Runtime Lease / Recovery Contract /              │
│ LocalHost / RemoteHost / Universal Wire / Production Doctor  │
└──────────────┬────────────────────┬───────────────────────────┘
               │                    │
        runtime delivery       platform services
               │                    │
    ┌──────────┼──────────┐         ├─ settings
    │          │          │         ├─ i18n
    ▼          ▼          ▼         ├─ notify
   JS       Process      WASM       ├─ theme
 EventBus   tauron-proc  validation ├─ brand
                         only       ├─ updater
                                    └─ install trust
```

注意：这不是“完全单栈”。当前仍有 `tauron-shell + @tauron/core` 的旧框架路径，因此真实架构应理解成：

> **canonical app-layer 已形成，但 legacy framework-layer 仍在迁移期。**

---

# 4. 启动与装配真实逻辑

## 4.1 Tauri 初始化链路

当前 Tauri 插件入口已经比较清楚：

```text
init()
  ↓
init_with_adapter_config(AdapterConfig)
  ↓
Tauri plugin setup
  ↓
command_state_with_dir_and_config()
  ↓
validate_production_readiness()
  ↓
SubstrateState::with_adapter_config()
  ↓
PluginRuntimeState（完整插件宿主才创建）
  ↓
manage_states()
  ↓
invoke_handler
```

当前有两个编译期命令集合：

- `tauron_substrate_handler![]`
  - 61 条底座命令；
  - 不创建/暴露插件 Registry runtime 面；
- `tauron_plugin_handler![]`
  - 61 条 substrate；
  - 22 条 plugin runtime；
  - `plugin-install` feature 打开后再增加 2 条；
  - 最大共 85 条。

这个设计是当前架构里比较正确的一步，因为“底座宿主不运行插件”不再只是运行时策略，而是在类型与命令注册面上直接缩小攻击面。

---

# 5. Production 安全链路

## 5.1 Production Readiness

Production 启动并不是“配置缺了就先运行再降级”，而是 fail-closed。

当前 readiness 至少覆盖：

- caller identity policy；
- origin gate 是否真正 armed；
- recovery durability / explicit unsupported；
- plugin install trust；
- trusted time；
- admin audit；
- writable durable data dir；
- process sandbox；
- no mock provider。

这是 Tauron 当前比很多桌面“应用模板”更像基础设施产品的地方。

## 5.2 Command 授权真实路径

当前实际授权不是单靠 Tauri capability，而是：

```text
WebView invocation
  ↓
TauriCallerSource
  ↓
Caller::from_label()
  ↓
origin_gated_handler
  ↓
command-specific guard
    ├─ require_main_window()
    ├─ require_self_plugin_scope()
    ├─ require_settings_key_scope()
    ├─ visible_* filtering
    └─ admin_gate()
  ↓
domain command
```

这一点非常重要。

当前代码已经明确修正了过去“把 Tauri root invoke handler 当成按命令 ACL”的错误假设。对 Tauron 自己注册在 root handler 的 `host_*` 命令，真正的细粒度授权事实位于 Tauron 的代码层。

## 5.3 这一设计的优势

- Principal 在进入业务前明确解析；
- malformed plugin label 不会退化成 MainWindow；
- privileged write 统一经过 `admin_gate`；
- admin allow/deny 都能写审计事实；
- command-surface generator 能复算命令是否有 guard；
- 9 条无需 guard 的命令也要求代码注释解释原因。

## 5.4 后续仍需改进

Tauri 自身已经有成熟的 plugin permission / scope 模型，因此长期不应该让：

> Tauri capability + Tauron origin gate + Tauron command guard

变成三套独立且重复的 policy source。

建议 V5 将其明确分层：

```text
Tauri capability
  = WebView 能否触达某类 Tauron bridge

Tauron Principal/Policy
  = 某个真实主体能否执行具体业务能力

Provider Scope
  = 该能力允许访问哪些资源
```

三者必须职责不同，而不是互相兜底。

---

# 6. 插件调用与 Runtime 的真实状态

## 6.1 当前调用分发

当前 `PluginRuntimeState::default_deliveries()` 已经把不同形态映射到 delivery：

```text
Plugin Call
   ↓
Registry / PendingCall
   ↓
PluginType → DeliveryKind
   ├─ JS      → JsCallDelivery
   ├─ Process → ProcessCallDelivery
   ├─ WASM    → WasmCallDelivery (feature)
   └─ Rust    → no runtime
```

## 6.2 JS Runtime

当前 JS 是四类中闭环程度最高的动态插件路径：

- 复用 EventBus request channel；
- 有 pending call；
- 有 settle/cancel；
- 有生命周期与 contributes；
- 可通过 SDK / host 接线。

建议后续将现有 JS delivery 包成标准 `RuntimeDriver`，不要再让 JS 成为特殊路径。

## 6.3 Process Runtime

当前 Process 已经不是纯 stub。

已经存在：

- `std::process::Command` 真 spawn；
- stdin/stdout `Stdio::piped()`；
- JSON-RPC line frame；
- 每进程 stdout reader；
- `callId` 回帧关联；
- PID + runtime generation 双重校验；
- stale reply fencing；
- `ProcessFrameSinkImpl → Registry::settle_call`；
- frame size 上限；
- per-pid writer lock；
- pid reuse generation；
- Unix process group；
- Windows Job Object；
- Drop/kill/wait 相关清理逻辑；
- crash budget。

### 但是有三个关键缺口

#### 缺口 P-R1：没有真实 sidecar E2E

仓库没有一个真正可执行、被测试矩阵启动的 sidecar fixture。

因此目前能证明：

> Host transport implementation 写对了。

但还不能证明：

> 一个真实 sidecar 在 Windows/macOS/Linux 上被 spawn → 收帧 → 执行业务 → 回帧 → settle → crash → restart → teardown → 无 orphan。

这是进入工业级最重要的测试缺口之一。

#### 缺口 P-R2：Production 默认没有 Hard sandbox provider

当前默认：

- Unix process group = `Partial`；
- Windows Job Object = `Partial`；
- fs/network/syscall isolation 都没有实现；
- Windows 还有 post-spawn attach race。

而 Production 对 Process runtime 采用 Hard 要求，因此默认内建实现实际上无法在严格 Production profile 中正常启用 Process 插件。

这在安全上是诚实的，但在产品能力上意味着：

> “Process Runtime 已实现”与“Production Process Runtime 可直接使用”不是一回事。

#### 缺口 P-R3：没有 Trust Profile

建议不要强迫所有 Process 插件共享一个“Hard sandbox 或完全禁用”的二元模型。

建议新增：

```text
ProcessTrustMode
├─ TrustedSigned
│   ├─ 强签名
│   ├─ publisher policy
│   ├─ explicit operator approval
│   └─ 不宣称 hostile-code sandbox
│
└─ SandboxedUntrusted
    ├─ OS hard sandbox required
    ├─ filesystem isolation
    ├─ network isolation
    ├─ process-tree containment
    └─ syscall / token restriction
```

这样既不把“签名”冒充“沙箱”，也不会让所有可信企业 sidecar 因跨平台 Hard sandbox 尚未实现而完全不可用。

---

# 7. WASM Runtime：当前最大能力缺口之一

`tauron-wasm` 现在拥有很多“Runtime 周边”：

- module cache；
- instance pool；
- crash tracker；
- memory config；
- ABI fingerprint；
- host function whitelist；
- generation lease。

但 `execute.rs` 自身明确仍是模拟执行，Cargo 依赖中也没有 Wasmtime / Wasmer / Extism 这类真实 engine。

`WasmCallDelivery` 当前实际行为是：

1. 从 Manifest 生成 config；
2. 执行真实 config validation；
3. 检查 crash budget；
4. **明确返回 delivered=false，因为没有实际 WASM runtime**。

这是一种正确的“诚实失败”，但仍然是未完成。

## 7.1 建议：不要继续自建一个半成品 WASM 引擎

建议把 WASM Engine 从 `tauron-wasm` 的 domain contract 中抽象出来：

```text
WasmEngineProvider
├─ ExtismEngine
├─ WasmtimeComponentEngine
└─ CustomEngine
```

### 方案 A：Extism 作为近期默认 provider

优点：

- 已经是成熟的 WebAssembly plugin system；
- 有 host functions；
- 有多个 Host SDK；
- PDK 覆盖 Rust、JS/TS、Go、C#、C、Zig 等；
- 很适合 Tauron “跨语言插件”方向。

### 方案 B：Wasmtime Component Model / WIT

优点：

- 更底层；
- 对 ABI、WASI、resource、component interface 有更强控制；
- 长期更适合形成 Tauron 自己的稳定插件 ABI。

### 推荐组合

```text
Tauron Runtime API
      │
      ├── Extism provider       ← 先形成生产能力
      │
      └── Wasmtime/WIT provider ← 后续高级/原生契约
```

不要让上层 Manifest / Registry / Host API 绑定某一个 engine。

---

# 8. Rust Native Plugin 应重新定义

当前 `PluginType::Rust` 没有动态执行器。

这并不一定是“漏做了一项功能”。

Rust 原生动态库跨编译器版本、依赖版本、panic ABI、allocator、platform loader 的稳定性都不适合作为普通第三方插件 ABI。

建议 V5 明确重定义：

```text
BuiltinNative
= 编译期链接进宿主的可信扩展

Dynamic extension:
- JS/WebView
- Process
- WASM
```

如确实需要 native dynamic plugin，只支持稳定边界：

- C ABI；
- UniFFI/CXX 受控接口；
- 或单独 Process。

不要把“任意 Rust crate 动态加载”作为普通扩展能力。

---

# 9. EventBus / Call / Stream

这是当前比较成熟的一块。

已经具备：

- public/private topic；
- approval；
- revoke；
- revoke 后退订；
- queued frame 作废；
- event/request/state 多通道；
- stream credit；
- ordering issue / observe；
- TS receive-side `EventOrderingWatcher`；
- owner-scoped cleanup。

## 9.1 当前缺口

### 公平调度仍未完成

`FairQueue` 已经被正确删除，因为之前它是“类型存在但没有生产消费者”的假完成状态。

当前已有：

- global limit；
- per-principal limit；
- Stream credit。

但还没有：

- owner-level fair scheduling；
- starvation E2E；
- multi-runtime 一致 admission。

### DecisionToken / PolicyEpoch 尚未贯穿所有 provider

Event revoke 做得已经比较深入，但：

- fs；
- http；
- process；
- secret/provider

并没有全部在 commit/use 边界重新验证 policy token。

建议统一：

```text
Authorize
   ↓
DecisionToken {
  principal,
  capability,
  scope_hash,
  policy_epoch,
  grant_version,
  expires_at
}
   ↓
Provider commit point
   ↓
revalidate(token)
```

防止：

> “检查时允许，但真正产生外部副作用时权限已经被撤销”。

---

# 10. Settings / Recovery

当前 Settings 已经不再是裸 `HashMap`：

- canonical `SettingsStore`；
- schema；
- migration；
- user layer durability；
- rollback image；
- migration failure recovery；
- revision；
- post-commit mirror/watch path；
- plugin namespace guard。

这是正确方向。

## 剩余缺口

1. `getAtLeastRevision()` 类型的读一致性接口尚未形成公共 wire；
2. bulk migrate/adopt 对 watcher 可见性不完全统一；
3. 配置、事件、Registry 各自仍有不同 transaction/fault boundary 风格。

建议后续把“Revisioned Durable State”抽象为内部公共设施，而不是每个 domain 各写一次。

---

# 11. Plugin Install / Package Manager

## 11.1 当前已经完成得比较好的部分

`plugin-install` 是 opt-in feature。

当前 Production 安装已经有：

```text
signed .tpkg
   ↓
verify package
   ↓
trusted time check
   ↓
install preview
   ↓
package digest + permissions
   ↓
InstallReviewToken
   ↓
user approved permissions
   ↓
review token revalidation
   ↓
install
   ↓
per-asset digest verification on service path
```

尤其是：

> preview 后如果包被替换，即便 id/version/permissions 相同，只要内容 digest 变化也拒绝安装。

这是很有价值的供应链安全基础。

## 11.2 仍然缺少完整的 Package Lifecycle

建议把当前“安装命令”升级成真正的 Extension Package Manager：

```text
PackageSource
    ↓
Resolver
    ↓
VerifiedPackage
    ↓
Immutable Generation Store
    ↓
Preflight
    ↓
Install Transaction
    ↓
Activate Generation
    ↓
Health Gate
    ├─ success → commit
    └─ failure → rollback
```

建议目录结构：

```text
plugins/
  com.example.foo/
    generations/
      1.2.0+sha256-xxxx/
      1.3.0+sha256-yyyy/
    active.json
    rollback.json
    state.json
```

禁止直接覆盖当前 active 文件。

---

# 12. Market 与 Distribute 的职责需要重新收敛

当前存在两个容易混淆的概念：

### `host_market_*`

当前仍带 simulated 语义：

- check；
- download；
- install。

### updater / `tauron-distribute`

当前 `host_updater_check` 已经会进入 `tauron-distribute` provider，未配置 endpoint 时明确 Unsupported。

建议 V5 明确：

```text
AppUpdater
= 更新 Tauron 宿主应用本身

ExtensionRegistry
= 查询插件 metadata / version / variant

ExtensionPackageManager
= 下载 / 验签 / 安装 / 更新 / rollback 插件
```

不要再用一个模糊 “market install” 同时覆盖应用更新和插件生命周期。

---

# 13. Local Host / Remote Host

`tauron-host` 已经拥有非常值得保留的基础：

## Local Host

已经有：

- broker lease；
- generation fencing；
- stale takeover；
- platform ownership proof contract；
- Unix/macOS/Windows reference path；
- chaos/e2e tests。

但它还不是官方生产 Host。

## Remote Host

已经有：

- one-time credential；
- audience；
- origin；
- TLS1.3 evidence；
- session；
- resume；
- nonce replay protection；
- sequence；
- rate limit；
- inflight quota；
- per-subject session quota。

这些设计已经比“先开个 WebSocket 再补安全”成熟很多。

## 最大问题

它们目前没有成为统一公开 HostTransport 的第二、第三生产实现。

所以“Tauri-independent core”目前更多是：

> **架构真实，但产品证明不足。**

---

# 14. 当前 CI / 工程治理

当前最新 main CI 的主要 Job 全绿：

- Performance / Size；
- cargo-deny；
- Rust default；
- Rust tauri feature；
- cargo clippy；
- cargo fmt；
- substrate-only example；
- TypeScript；
- Linux x64 target matrix；
- Windows x64 target matrix；
- macOS arm64；
- macOS x64。

目标矩阵已经覆盖：

- substrate；
- ffi；
- extension；
- tauri-desktop；
- process；
- wasm-broker contract；
- remote-tls；
- LocalHost reference。

## 14.1 当前优势

- GitHub Actions 固定 action SHA；
- cargo-deny hard gate；
- Rust/TS 双栈 contract；
- public surface ledger；
- target matrix；
- release evidence inputs；
- lock-across-await check；
- command-surface check；
- docs line reference check；
- version single-source check；
- binary/RSS/performance budget。

## 14.2 仍需补充

当前主 CI 尚未形成完整的：

- cargo-fuzz / libFuzzer；
- coverage threshold；
- Criterion regression gate；
- packaged installer E2E；
- real Process sidecar E2E；
- real WASM E2E；
- SBOM；
- build provenance/attestation；
- reproducible build byte comparison；
- CodeQL/SAST；
- soak/leak trend。

这些应该是后续“Industrial Release Evidence”的组成部分。

---

# 15. 文档系统问题：现在最大的不是真没文档，而是语义漂移

当前仓库的文档自动化已经很强，但主要验证：

- 行号/符号引用仍存在；
- 命令面没有漏；
- 版本号一致；
- public surface ledger 一致；
- target matrix 一致。

它无法自动判定：

> “这个模块现在到底是 Stub、Partial、Production 还是 Legacy？”

因此出现了：

- 代码已经接 `tauron-brand`，部分文档仍写 brand 未接；
- theme 已有真实 registry commands，旧状态表仍写 optional/unwired；
- updater 已真实进入 distribute provider，旧文档仍写 distribute 未接；
- 当前 16 crates / 21 packages，旧 competitive analysis 仍保留历史统计。

## 建议新增 `contracts/module-maturity.json`

例如：

```json
{
  "tauron-wasm": {
    "state": "partial",
    "runtime": "unsupported",
    "productionConsumers": ["tauron-adapter"],
    "tests": ["..."],
    "limitations": ["no real wasm engine"]
  }
}
```

并自动生成：

- architecture status；
- competitive analysis Tauron column；
- README capability matrix；
- release notes maturity section。

这样才能把“语义成熟度”也纳入代码审计。

---

# 16. 竞品对比方法

Tauron 并没有一个完全同构的直接竞品。

更合理的比较方式是按能力层分组：

| 层 | 参考对象 |
|---|---|
| 桌面 Shell / IPC / Security | Tauri 2、Electron |
| Extension Platform | VS Code Extension Host、Eclipse Theia |
| Sandboxed Cross-language Runtime | Extism / Wasmtime |
| Tauron | 希望把以上几层组合成通用桌面 Extension Substrate |

---

# 17. 与 Tauri 2 对比

Tauri 2 本身提供：

- Rust Plugin；
- WebView IPC；
- permission / scope；
- capability；
- window/webview binding；
- desktop + Android/iOS plugin support。

## Tauron 相对优势

### 1. 更上层的 Extension Lifecycle

Tauri plugin 更接近“应用依赖/平台插件”。

Tauron 增加了：

- runtime registry；
- install/uninstall；
- enable/disable；
- plugin state；
- call graph；
- event approval；
- recovery；
- process runtime；
- package trust；
- plugin SDK。

### 2. 更强的运行时主体模型

Tauron 已经明确区分 MainWindow / Plugin principal，并对 self/scoped/privileged 做更高层业务授权。

### 3. 更强的可验证契约

Tauri 是通用桌面框架；Tauron 在自己的域内增加了：

- command surface generator；
- TS/Rust contract；
- public surface ledger；
- Production doctor；
- target matrix。

## Tauron 相对劣势

### 1. 与 Tauri 权限系统存在重复

长期维护两套 permission semantics 成本高。

### 2. 移动端能力明显落后

Tauri 官方插件体系已经把 Android Kotlin / iOS Swift 纳入正式 plugin model，而 Tauron v1 官方边界仍是 Tauri Desktop。

### 3. Tauri 插件本身已有成熟的 generated permissions

Tauron 应复用 Tauri 的 transport access control，而不是自己再实现一套同层命令 ACL。

---

# 18. 与 Electron 对比

Electron 的优势主要在：

- Chromium/Node 生态成熟；
- renderer sandbox；
- context isolation；
- preload/contextBridge；
- 大量成熟桌面应用实践；
- DevTools / extension / Node 生态巨大。

## Tauron 的优势

- Tauri 基础体积通常更轻；
- Rust kernel 的类型安全与资源治理空间更大；
- Tauron 能把插件 capability 明确建模，而不是依赖每个 app 自己设计 preload API；
- Production doctor / permission / runtime lease / package trust 更偏基础设施化。

## Tauron 的不足

Electron 的 renderer sandbox 是长期成熟的 Chromium 安全能力。

而 Tauron 当前 Process Plugin：

- 只有 process tree containment；
- 没有跨平台统一 fs/net/syscall hard sandbox；
- Production 严格模式下因此直接 fail-closed。

所以在“运行不可信第三方 native/process code”这个维度上，Tauron 不能因为使用 Rust/Tauri 就默认认为隔离更强。

---

# 19. 与 VS Code Extension Host 对比

VS Code 的 Extension Host 已经形成：

- local extension host；
- web extension host；
- remote extension host；
- Node / Browser runtime；
- `extensionKind`；
- lazy activation；
- contribution points；
- mature extension lifecycle；
- 大型 Marketplace 生态。

## Tauron 的优势

- 不局限 IDE；
- Rust kernel；
- 更明确的多 Runtime 目标；
- 可以把 WASM / Process / WebView 都作为一等 runtime；
- package trust / policy / recovery 可以比传统 JS extension model 更严格。

## Tauron 的差距

### 1. 缺统一 Runtime Contract

VS Code 的扩展 host model 已经非常统一，而 Tauron 目前每类 Runtime 仍各自演进。

### 2. 缺 Activation Model

建议 Manifest V3 增加：

```yaml
activation:
  - onCommand: foo.open
  - onEvent: workspace.ready
  - onWindow: main
  - onStartupFinished: true
```

避免所有插件在启动时全部激活。

### 3. 缺真正的 Remote Extension Host

Tauron 已有 RemoteHost contract，但还没形成 VS Code 那样被普通用户直接消费的完整 remote runtime。

### 4. 生态差距巨大

Tauron 不应该短期追求 Marketplace 数量，而应该先把 **Conformance + SDK + Package ABI 稳定性** 做成优势。

---

# 20. 与 Eclipse Theia 对比

Theia 支持：

- VS Code extensions；
- compile-time Theia extensions；
- Theia plugins；
- headless plugins；
- frontend/backend 多扩展形态。

## Theia 的优势

- extension mode 已经非常成熟；
- 可复用 VS Code 生态；
- DI-based product composition；
- headless backend extension 模型成熟。

## Tauron 的优势

- 更偏通用客户端，而不是 IDE workbench；
- Rust kernel 更适合本地安全能力；
- Process/WASM runtime 可以提供更强隔离选择；
- 更有机会成为“桌面 app substrate”而不是“IDE framework”。

## Tauron 的不足

- 缺 VS Code compatibility 这种现成生态入口；
- compile-time extension 与 runtime extension 边界没有 Theia 那么清楚；
- 当前 Rust Native type 语义容易混淆 compile-time 和 dynamic runtime。

建议直接借鉴 Theia 的思想：

> **Builtin Extension 与 Runtime Plugin 必须是两个概念。**

---

# 21. 与 Extism 对比

Extism 的核心优势：

- WebAssembly plugin runtime 是真实能力；
- 多 Host SDK；
- 多语言 PDK；
- Host Function；
- WASM 本身隔离；
- 插件语言与宿主语言解耦。

## Tauron 的优势

Extism 不解决：

- window；
- tray；
- desktop lifecycle；
- app recovery；
- plugin UI；
- plugin marketplace lifecycle；
- Tauri integration；
- app update；
- settings；
- notification；
- multi-window principal。

这些正是 Tauron 的上层价值。

## Tauron 的明显差距

在 WASM runtime 这一层，当前 Tauron 没必要与 Extism 重复造轮子。

最合适的关系应该是：

> **Tauron = Extension Platform / Capability Broker**  
> **Extism / Wasmtime = 可插拔 WASM Runtime Provider**

---

# 22. 竞品差异矩阵

| 维度 | Tauron 当前 | Tauri 2 | Electron | VS Code | Theia | Extism |
|---|---|---|---|---|---|---|
| 桌面 Shell | 强 | 强 | 强 | 产品内建 | 产品框架 | 无 |
| Runtime Plugin Lifecycle | 中-强 | 弱 | 需应用自建 | 强 | 强 | 中 |
| JS Runtime | 有 | WebView | Node/Renderer | Extension Host | Plugin Host | 编译 WASM |
| Process Runtime | 部分完成 | 需自建 | Node child 可自建 | Remote/Extension Host | backend | 非重点 |
| WASM Runtime | **未完成** | 非核心 | 需自建 | 非核心 | 非核心 | **强** |
| Capability Policy | 强但复杂 | 强 | 需应用设计 | 强 | 中-强 | Host Functions |
| Package Trust | 较强安装基础 | 生态各自处理 | npm/app 自己处理 | Marketplace | OpenVSX/VSX | 由 Host 管 |
| Runtime Update/Rollback | 未完成 | 非核心 | 应用自建 | 成熟生态 | 成熟生态 | 非重点 |
| Multi-host | contract 有、产品未闭环 | Tauri | Electron | local/web/remote | browser/backend | 多 Host SDK |
| Mobile | 当前不支持 | **正式支持** | 非核心 | 有 Web 场景 | Browser | Runtime 取决 Host |
| Remote Host | contract/reference | 非重点 | 应用自建 | **成熟** | backend | Host 自己实现 |
| CI/Contract Governance | **很强** | 框架自身很强 | 框架自身很强 | 很强 | 很强 | 强 |
| Ecosystem | 早期 | Tauri 生态 | 超大 | **超大** | VS Code/OpenVSX | 多语言 WASM 生态 |

---

# 23. Tauron 当前核心优势

## 23.1 Fail-closed 思维已经进入代码

很多项目把安全写在 README，Tauron 已经把它做成：

- startup gate；
- command gate；
- feature gate；
- production doctor；
- contract test；
- CI hard gate。

这是最值得继续保留的架构文化。

## 23.2 “Unsupported” 被当成正式协议状态

例如：

- 未配置 brand provider；
- 未配置 updater endpoint；
- WASM runtime 缺失；
- runtime delivery 不存在。

都倾向显式 Unsupported，而不是返回“空成功”。

这对于基础设施非常重要。

## 23.3 跨语言契约门禁很有价值

Rust 与 TS 的：

- error codes；
- commands；
- line shapes；
- consumers；
- public ledger；

能够相互校验。

这个思路后续应该扩大到：

- Manifest；
- HostProtocol；
- RuntimeDriver；
- PackageFormat；
- FFI。

## 23.4 `SubstrateState / PluginRuntimeState` 分离正确

这是一个真正有架构价值的类型级边界。

未来 Profiles 也应该继续沿用：

```text
substrate
extension
ffi
tauri-desktop
local-host
remote-host
```

不要重新退回一个 `GlobalState`。

---

# 24. Tauron 当前核心缺点

## P0：架构级

### P0-1 双 canonical 尚未彻底消失

- `tauron-shell` legacy；
- `@tauron/core` legacy/new overlap；
- 两套 Plugin SDK；
- framework adapter 仍走 old core。

### P0-2 Runtime 不统一

JS / Process / WASM 各有自己调用约定、能力事实和生命周期边界。

### P0-3 Universal 的第二 Host 不存在

核心可复用 ≠ 产品可复用。

必须有至少第二个真实 Host 才能证明 Host independence。

### P0-4 Process Production 能力不完整

Transport 已经很多，但 Hard security 与 E2E 还没封口。

### P0-5 WASM 是最大“接口已在，能力未在”的模块

必须真实落地或退出当前稳定能力表。

## P1：产品/扩展平台

### P1-1 无完整 Plugin Update/Rollback

### P1-2 Manifest 尚无稳定演进协议

### P1-3 无 dependency graph / lockfile

### P1-4 Activation Model 不足

### P1-5 Market / Package Manager / App Updater 职责重叠

## P1：工程质量

### P1-6 Adapter 过度集中

约 1.7 万行的 `lib.rs` 不适合继续增长。

### P1-7 canonical kernel 中几个单文件也过大

Registry / EventBus / AuthZ 应按 ownership 拆分。

### P1-8 canonical observability 不够完整

当前 canonical `tauron-host` 并没有形成统一 tracing / metrics / OpenTelemetry 层；反而 legacy `tauron-shell` 还存在 tracing 依赖。

## P2：生态与发布证明

- 真 third-party consumer 不足；
- public conformance kit 不完整；
- packaged E2E 不完整；
- security review 不完整；
- fuzz / soak / leak trend 不完整；
- SBOM/provenance/reproducibility 不完整；
- Mobile 尚未进入 Tauron host model。

---

# 25. 建议的 V5 目标架构

建议 V5 不再以“继续增加 crate”为主，而是重新明确七个 Plane。

```text
┌────────────────────────────────────────────────────────────┐
│ 7. SDK / UI / Tooling Plane                               │
│ TS SDK / Rust SDK / C ABI / .NET / adapters / CLI         │
├────────────────────────────────────────────────────────────┤
│ 6. Host Adapter Plane                                     │
│ TauriHost | LocalHost | RemoteHost | future MobileHost     │
├────────────────────────────────────────────────────────────┤
│ 5. Extension Package Plane                                │
│ Resolver / Trust / Install / Update / Rollback / Store     │
├────────────────────────────────────────────────────────────┤
│ 4. Runtime Plane                                          │
│ JS Driver | Process Driver | WASM Driver                   │
├────────────────────────────────────────────────────────────┤
│ 3. Policy & Capability Plane                              │
│ Principal / Permission / Scope / DecisionToken / Audit     │
├────────────────────────────────────────────────────────────┤
│ 2. Kernel Plane                                           │
│ Registry / Lifecycle / Calls / Event / Streams / Recovery  │
├────────────────────────────────────────────────────────────┤
│ 1. Universal Contract Plane                               │
│ Wire / Protocol Negotiation / Error / Manifest / ABI       │
└────────────────────────────────────────────────────────────┘
```

Tauri 应该只存在于第 6 层。

---

# 26. V5 RuntimeDriver

建议新增真正统一的 Runtime SPI：

```rust
trait RuntimeDriver {
    fn kind(&self) -> RuntimeKind;

    fn capabilities(&self) -> RuntimeCapabilities;

    fn prepare(&self, ctx: PrepareContext)
        -> Result<PreparedRuntime>;

    fn start(&self, prepared: PreparedRuntime)
        -> Result<RuntimeLease>;

    fn invoke(&self, lease: &RuntimeLease, call: RuntimeCall)
        -> Result<DispatchReceipt>;

    fn cancel(&self, lease: &RuntimeLease, call_id: &str)
        -> Result<()>;

    fn health(&self, lease: &RuntimeLease)
        -> RuntimeHealth;

    fn stop(&self, lease: RuntimeLease, reason: StopReason)
        -> Result<StopReport>;
}
```

所有 Driver 必须共享：

- generation；
- lease；
- cancellation；
- timeout；
- admission；
- metrics；
- trace context；
- health；
- shutdown；
- crash budget；
- capability descriptor。

然后：

```text
JsRuntimeDriver
ProcessRuntimeDriver
ExtismRuntimeDriver
```

都跑同一套 Runtime Conformance Kit。

---

# 27. HostTransport 也必须统一

建议 Universal Contract 先固定：

```text
ProtocolInfo
- protocolVersion
- minCompatibleVersion
- codecs
- maxFrameBytes
- capabilities
- hostKind
- runtimeKinds
```

统一 envelope：

```json
{
  "protocolVersion": 2,
  "id": "...",
  "principal": "...",
  "domain": "settings",
  "method": "get",
  "args": {},
  "trace": {},
  "deadlineMs": 1000
}
```

HostAdapter：

```text
TauriHostAdapter
LocalHostAdapter
RemoteHostAdapter
```

全部转换成同一个 kernel command。

### 迁移原则

不要一次删除 85 条现有 Tauri command。

先：

1. 增加 `host_protocol_info`；
2. 增加 Universal envelope；
3. `@tauron/host` 默认走新 transport；
4. 旧命令通过 compatibility adapter 转发；
5. N-1 conformance 稳定之后再减少旧 public surface。

---

# 28. Manifest V3

建议下一版 Manifest 结构包含：

```yaml
manifestVersion: 3

id: com.example.plugin
version: 1.2.0
publisher: example

host:
  api: ">=2.0 <3"

runtime:
  kind: wasm
  entry: plugin.wasm
  protocol: 2
  activation:
    - onCommand: example.run

variants:
  - os: windows
    arch: x86_64
    artifact: ...
    digest: sha256:...

permissions:
  - fs:read

scopes:
  fs:
    allow:
      - "${workspace}/**"

resources:
  memoryMb: 64
  concurrency: 4
  timeoutMs: 5000

contributes:
  commands: ...
  menus: ...
  settings: ...

dependencies:
  required: ...
  optional: ...

integrity:
  assets: ...
```

签名最好放 package envelope / signed metadata 中，而不是让 plugin 自己修改 manifest signature 字段。

---

# 29. Adapter 拆分建议

当前 `tauron-adapter/src/lib.rs` 约 1.7 万行，建议只做**领域拆分，不做过度微文件化**。

目标：

```text
tauron-adapter/src/
  lib.rs                 # re-export / assembly only
  config.rs
  state.rs
  production.rs

  commands/
    capability.rs
    events.rs
    settings.rs
    registry.rs
    runtime.rs
    shell.rs
    i18n.rs
    notifications.rs
    recovery.rs
    updater.rs
    theme.rs
    brand.rs
    filesystem.rs
    network.rs

  runtime/
    js.rs
    process.rs
    wasm.rs

  install/
    preview.rs
    transaction.rs
    assets.rs

  providers/
    dialog.rs
    window.rs
    http.rs
    updater.rs

  tauri/
    caller.rs
    commands.rs
    origin.rs
    state.rs
```

建议约束：

- 普通 domain 文件尽量控制在 300–800 行；
- 复杂 state machine 可独立更大，但必须有单一 owner；
- `lib.rs` 不再承载业务实现。

---

# 30. Kernel 拆分建议

## registry.rs

当前约 3 千行，建议拆为：

```text
registry/
  mod.rs
  plugin_registry.rs
  pending_calls.rs
  runtime_leases.rs
  installation.rs
  visibility.rs
  gc.rs
```

## eventbus.rs

建议拆为：

```text
eventbus/
  mod.rs
  topics.rs
  subscriptions.rs
  approvals.rs
  queues.rs
  ordering.rs
```

## authz.rs

建议拆为：

```text
authz/
  principal.rs
  policy.rs
  guards.rs
  scopes.rs
```

避免新的“另一个巨大 canonical file”。

---

# 31. TS API 收敛

建议最终公共层只保留三个主要概念：

```text
@tauron/host
@tauron/plugin-sdk
@tauron/types
```

### 处理 `@tauron/core`

阶段 1：

- freeze；
- adapter-react/vue/svelte 改依赖 `@tauron/host`；
- `@tauron/core` 内部调用转发到 host client。

阶段 2：

- 变成 compatibility package；
- 标记 deprecated。

阶段 3：

- 下一个 major 删除。

### 两套 Plugin SDK

`@tauron/plugin-sdk` 与 `@tauron/app-plugin-sdk` 应统一契约。

可以保留两个 entrypoint，但共享一个 implementation：

```text
@tauron/plugin-sdk/browser
@tauron/plugin-sdk/runtime
```

不要维持两套生命周期、context 和 contribution 实现。

---

# 32. 可观测性方案

建议新增 canonical observability layer：

```text
tauron-observe
```

或者直接作为 `tauron-host::observe`，避免再次产生孤儿 crate。

统一：

- `tracing`；
- structured span；
- metrics；
- audit；
- diagnostics；
- health snapshot。

每次 plugin call 至少包含：

```text
trace_id
call_id
principal
plugin_id
runtime_kind
generation
queue_wait
execution_time
result
error_code
retry_class
resource_delta
```

核心指标：

- invoke count；
- error rate；
- timeout rate；
- queue depth；
- admission reject；
- event drop；
- stream backpressure；
- process crash；
- restart count；
- package rollback；
- policy deny；
- recovery safe-mode entry。

OpenTelemetry 可以作为可选 exporter，而不是 kernel 硬依赖。

---

# 33. Network / FS / Secret Provider

当前 NetworkPolicy 已有不少真实校验，但 redirect / DNS resolution 的检查还没有完整进入 production sink。

建议所有外部副作用 provider 使用统一接口：

```text
authorize
  ↓
resolve
  ↓
revalidate
  ↓
commit
```

HTTP：

- URL policy；
- DNS resolution；
- private address check；
- redirect per-hop check；
- credential cross-origin stripping；
- response size limit；
- timeout；
- decompression bomb limit。

FS：

- canonical path；
- symlink escape；
- root scope；
- case-fold / reserved name；
- Windows long path；
- atomic write。

Secret：

- 不允许插件直接取得 master secret；
- handle-based read/use；
- policy revalidation；
- audit。

---

# 34. Package Update / Rollback 事务

建议状态机：

```text
Downloaded
  ↓
Verified
  ↓
Reviewed
  ↓
Staged
  ↓
Preflighted
  ↓
Activated
  ↓
Healthy
  └─────────→ Committed

Activated
  ↓ health fail
RolledBack
```

每一步都需要：

- durable journal；
- idempotency key；
- generation；
- exact package digest；
- permission snapshot；
- previous active generation；
- crash recovery。

---

# 35. 第二官方 Host 应提前

如果 Tauron 的长期目标仍然是 Universal Substrate，那么建议把 LocalHost 从后置工作提前。

原因非常简单：

> 在第二 Host 真正跑起来以前，所有“平台无关 kernel”都只能证明代码依赖图独立，不能证明产品协议独立。

建议产物：

```text
tauron-local-host
```

能力：

- Unix Domain Socket；
- Windows Named Pipe；
- peer credential；
- HostProtocol handshake；
- same Principal；
- same RuntimeDriver；
- same PackageManager；
- same Conformance Kit。

然后提供：

- Node reference client；
- .NET reference client；
- Rust reference client。

Qt/Electron 只需要消费 LocalHost，而不需要把 Tauron 核心重新嵌进各自进程。

---

# 36. Mobile 策略

当前不要同时推进 Desktop Runtime 重构和 Mobile Host。

推荐顺序：

1. 先完成 HostTransport；
2. TauriHost + LocalHost 同跑；
3. 稳定 Universal Protocol；
4. 再评估 Tauri Mobile Host。

移动端不能简单复用 Process Plugin 模型，需要单独约束：

- iOS 后台/动态代码限制；
- Android service/process；
- store policy；
- Kotlin/Swift native plugin；
- WASM feasibility；
- filesystem/sandbox 差异。

因此 Mobile 是 Host Profile，不应该污染桌面 kernel。

---

# 37. 改造优先级

## Phase A — Canonical 收敛

**优先级：P0**

- 拆 `tauron-adapter`；
- `tauron-shell` 只留 compatibility facade；
- framework adapters 切到 `@tauron/host`；
- 两套 Plugin SDK 统一 implementation；
- 建立 `module-maturity.json`；
- 删除/隐藏 stale public APIs。

### Exit Gate

- canonical owner 每个 domain 只有一个；
- legacy 层不得新增业务代码；
- adapter 业务不再堆在 `lib.rs`；
- docs capability status 由 machine ledger 生成。

---

## Phase B — Runtime Plane

**优先级：P0**

- `RuntimeDriver`；
- JS driver；
- Process driver；
- real WASM driver；
- Runtime Conformance Kit；
- owner fair queue；
- resource budgets。

### Exit Gate

同一套测试必须对 JS / Process / WASM 跑：

- prepare；
- start；
- invoke；
- cancel；
- timeout；
- crash；
- recovery；
- stop；
- lease expiry；
- resource quota。

---

## Phase C — Process 生产闭环

**优先级：P0**

建立仓内真实 sidecar fixture：

```text
fixtures/process-plugin/
```

支持：

- echo；
- sleep；
- crash；
- malformed frame；
- oversized frame；
- spawn child；
- ignore stdin；
- close stdout；
- duplicate response。

四个平台 target matrix 真启动它。

### Exit Gate

- invoke round trip；
- cancellation；
- stale generation；
- crash restart；
- no orphan child；
- teardown；
- PID reuse simulation；
- Windows Job Object；
- Unix process group；
- trusted/sandboxed profile 分义。

---

## Phase D — Extension Package Manager

**优先级：P0/P1**

- Manifest V3；
- resolver；
- lockfile；
- immutable store；
- update；
- rollback；
- health gate；
- package source protocol。

### Exit Gate

任何中断点重启后均满足：

```text
old generation active
或
new verified generation active
```

不允许“半安装”。

---

## Phase E — Universal Host

**优先级：P0（Universal 目标下）**

- LocalHost product；
- Universal Protocol；
- Node/.NET/Rust client；
- TauriHost 与 LocalHost 同跑 Conformance。

### Exit Gate

同一个 sample app 的核心插件调用测试，在：

```text
TauriHost
LocalHost
```

不修改 domain 代码即可通过。

---

## Phase F — Security Closure

**优先级：P1**

- trusted time production provider；
- HTTP DNS/redirect sink；
- DecisionToken provider revalidation；
- process trust profiles；
- secret provider；
- threat model；
- external security review。

---

## Phase G — Release Evidence

**优先级：P1**

- N-1 compatibility；
- packaged E2E；
- fuzz；
- soak；
- leak trend；
- benchmark regression；
- coverage；
- SBOM；
- provenance；
- signed releases；
- reproducible build。

---

# 38. 测试体系升级

## 38.1 Runtime Conformance

```text
runtime-conformance/
  lifecycle
  invoke
  cancel
  timeout
  backpressure
  crash
  restart
  generation
  teardown
```

## 38.2 Host Conformance

```text
host-conformance/
  principal
  auth
  capabilities
  events
  settings
  recovery
  package
  runtime
```

## 38.3 Package Chaos

随机在这些点 kill host：

```text
verify
unpack
fsync
journal
activate
health-check
rollback
```

重启后自动校验不变量。

## 38.4 Security Property Tests

- path traversal；
- symlink escape；
- permission revoke race；
- nonce replay；
- call replay；
- stale generation；
- package swap；
- redirect-to-private-ip；
- zip bomb；
- frame bomb。

---

# 39. 性能指标应从“单点预算”升级为 SLO

当前已有：

- wire 100k iteration elapsed budget；
- process peak RSS；
- binary size；
- release artifact size。

建议新增：

| 指标 | 建议关注 |
|---|---|
| Host cold start | substrate / full profile 分开 |
| Plugin activate | p50 / p95 / p99 |
| JS invoke | p50 / p95 / p99 |
| Process invoke | p50 / p95 / p99 |
| WASM cold invoke | p50 / p95 / p99 |
| WASM warm invoke | p50 / p95 / p99 |
| Event publish | throughput + p99 |
| Queue | max depth / starvation |
| Memory | base + per plugin |
| Recovery | reconcile duration |
| Install | peak RSS + disk amplification |
| Update | rollback duration |

性能基线必须按 commit 存储趋势，而不只是单次 threshold。

---

# 40. API / Compatibility 策略

建议建立四个明确版本域：

```text
HostProtocolVersion
ManifestVersion
PluginRuntimeABI
SDK SemVer
```

不要用 npm/crate 版本隐式代替协议版本。

## N-1 原则

例如 HostProtocol 2：

- server 必须支持 client 1/2；
- client 2 必须能识别 server 1 并降级；
- capability negotiation 明确列出不可用域；
- unknown field forward compatibility；
- unknown command 不得 panic。

---

# 41. 建议对外能力等级

当前建议继续采用诚实分级：

```text
Stable
Preview
Experimental
Unsupported
```

例如：

| 能力 | 当前建议 |
|---|---|
| Tauri substrate | Stable/接近 Stable |
| Command/AuthZ | Stable |
| EventBus | Stable |
| Settings/Recovery | Stable |
| JS plugin runtime | Stable/Preview，取决正式 API 承诺 |
| Process transport | Preview |
| Process untrusted sandbox | Unsupported |
| WASM runtime | Unsupported |
| LocalHost | Experimental |
| RemoteHost | Experimental |
| plugin install | Preview |
| plugin update/rollback | Unsupported |
| marketplace/catalogue | Experimental/未实现 |
| mobile host | Unsupported |

不要使用一个总的 “industrial-grade” 标签掩盖不同 domain 的成熟度。

---

# 42. 建议的发布门槛

在对外声明“Universal / Industrial Extension Substrate”之前，至少应满足：

1. legacy canonical 清理完成；
2. TauriHost + LocalHost 通过同一 Conformance；
3. JS / Process / WASM Runtime 通过同一 Runtime Conformance；
4. Process 真 sidecar E2E；
5. no-orphan-process gate；
6. real WASM execution；
7. plugin update/rollback；
8. N-1 protocol compatibility；
9. packaged installer E2E；
10. fuzz + soak/leak；
11. SBOM + provenance；
12. external security review；
13. 至少存在多个真实外部 Consumer，而不是仅 examples/fixtures。

---

# 43. 最终推荐的产品定位

## 当前 1.x

建议表述：

> **Tauri-first secure extension substrate**  
> 面向需要插件生命周期、Capability、Process/JS Runtime、Settings、Recovery 和 package trust 的 Tauri 2 桌面客户端。

这和当前真实能力最一致。

## 下一代

当 LocalHost + RuntimeDriver + Package Lifecycle 完成后，定位升级为：

> **Universal Capability & Extension Runtime Substrate**  
> 为 Tauri、独立本地 Host 和其他桌面壳提供统一 Principal、Capability、Extension Runtime、Package Lifecycle 与 Conformance。

---

# 44. 最终架构原则

后续所有新功能建议遵守以下原则：

1. **没有消费者的类型 = 未完成。**
2. **没有 production path 的实现 = reference，不叫 production capability。**
3. **没有真实运行时的 Runtime type = Unsupported。**
4. **安全检查必须位于副作用 commit point 之前。**
5. **权限撤销必须能影响既有 lease/token。**
6. **任何动态资源都必须带 owner + generation + cleanup。**
7. **任何 update/install 都必须事务化。**
8. **任何 capability 都必须 machine-readable。**
9. **任何公开宣称都必须有 conformance evidence。**
10. **Tauri 只是 HostAdapter，不是 Kernel。**
11. **UI Framework 只是 Consumer，不进入安全内核。**
12. **文档成熟度必须由代码事实生成，而不是人工维护。**

---

# 45. 建议下一步实施顺序

建议代码实施顺序严格按以下顺序，不再并行扩新能力：

```text
A. Canonical + Adapter 收敛
        ↓
B. Universal Contract / RuntimeDriver
        ↓
C. 真 Process E2E + Process Trust Profile
        ↓
D. 真 WASM Runtime
        ↓
E. Manifest V3 + Extension Package Manager
        ↓
F. LocalHost 第二官方 Host
        ↓
G. Security Provider Closure
        ↓
H. Release Evidence / Ecosystem
```

如果 A/B 不先完成，后面的 WASM、LocalHost、Marketplace 都会继续复制现有两套协议与状态模型。

---

# 46. 本轮审计发现的几个具体修订建议

## 46.1 立即更新架构状态文档

当前代码已经发生变化，应同步修正：

- `docs/architecture/overview.md`
- `docs/architecture/canonical-owners.md`
- `docs/competitive-analysis/competitive-analysis.md`

特别是：

- `tauron-brand` 当前已有真实 `host_brand_info` 接线；
- `tauron-theme` 已进入真实 theme registry commands；
- `tauron-distribute` 已进入 updater provider；
- crate/package 数量已经变化；
- Process frame path 已经比旧文档成熟。

## 46.2 将现有 gap plan 变成 machine ledger

`v4-industrial-gap-closure-plan.md` 很详细，但状态仍靠人工维护。

建议把 A64–A110 变成：

```text
contracts/v4-capability-ledger.json
```

字段：

```text
id
owner
status
producer
consumer
productionPath
conformance
limitations
evidence
```

Markdown 从这个 JSON 生成。

## 46.3 不再新增 root `host_*` 命令

在 Universal Protocol 完成以前，只允许：

- 修复；
- 补安全 guard；
- 必须的新 handshake。

新增 domain API 尽量先进入 typed `@tauron/host`，避免 85 条继续增长到 100+。

---

# 47. 审计证据索引

本方案重点核对了以下当前代码/文档：

- `Cargo.toml`
- `package.json`
- `pnpm-workspace.yaml`
- `README.md`
- `CHANGELOG.md`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `contracts/public-surface-ledger.json`
- `contracts/target-matrix.json`
- `contracts/performance-budgets.json`
- `crates/tauron-host/src/*`
- `crates/tauron-adapter/src/lib.rs`
- `crates/tauron-adapter/src/tauri.rs`
- `crates/tauron-adapter/src/process_delivery.rs`
- `crates/tauron-adapter/src/wasm_delivery.rs`
- `crates/tauron-proc/src/spawner.rs`
- `crates/tauron-wasm/src/execute.rs`
- `crates/tauron-shell/src/dispatch.rs`
- `docs/api/command-surface.md`
- `docs/architecture/canonical-owners.md`
- `docs/architecture/v4-industrial-gap-closure-plan.md`
- `docs/integration/support-boundary.md`
- `docs/competitive-analysis/competitive-analysis.md`

---

# 48. 外部对比参考

以下均为本轮核对时使用的官方资料：

1. Tauri 2 — Plugin Development  
   https://v2.tauri.app/develop/plugins/

2. Tauri 2 — Capabilities / Security  
   https://v2.tauri.app/security/capabilities/  
   https://v2.tauri.app/security/

3. Electron — Security / Context Isolation / Process Sandboxing  
   https://www.electronjs.org/docs/latest/tutorial/security  
   https://www.electronjs.org/docs/latest/tutorial/context-isolation  
   https://www.electronjs.org/docs/latest/tutorial/sandbox/

4. Visual Studio Code — Extension Host  
   https://code.visualstudio.com/api/advanced-topics/extension-host

5. Visual Studio Code — Contribution Points / Activation Events  
   https://code.visualstudio.com/api/references/contribution-points  
   https://code.visualstudio.com/api/references/activation-events

6. Eclipse Theia — Extensions and Plugins  
   https://theia-ide.org/docs/extensions/

7. Extism — Plug-ins / Host Functions / PDK  
   https://extism.org/docs/concepts/plug-in/  
   https://extism.org/docs/concepts/host-functions/  
   https://extism.org/docs/concepts/pdk/

---

# 49. 最终判断

Tauron 当前真正值得保留并继续放大的不是“功能数量”，而是以下四点：

1. **Fail-closed security posture**；
2. **machine-verifiable contracts**；
3. **platform-neutral canonical kernel**；
4. **owner/generation/lifecycle 思维**。

未来最大风险则是：

> 为了追求“Universal / Industrial / 多 Runtime / 多客户端”的完整名义，继续在尚未收敛的双栈上横向增加功能。

因此 V5 最重要的目标不是再增加 20 个模块，而是做到：

> **One Kernel, One Policy Model, One Host Protocol, One Runtime Contract, One Package Lifecycle, Multiple Adapters.**

当这六个“一”真正成立以后，Tauron 才会从“很强的 Tauri 插件化基础设施”升级成“可被多种客户端可靠复用的通用应用底座”。
