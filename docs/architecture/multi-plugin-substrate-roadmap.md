# tauron 0.3 优化改进方案：多插件框架 × 任意宿主底座

> **状态**：分轮实施中；尚未达到 0.3 验收条件，不得据此宣称可发布。
> **目标版本**：tauron 0.3
> **前置**：底座重构 R1–R8（轮 7–12 **已完成并收口**）。原始方案文档已清理，
> 结论沉淀在 [overview.md](./overview.md) 的「架构演进」与
> [canonical-owners.md](./canonical-owners.md)。
> **关联文档**：[overview.md](./overview.md)、[app-layer-wire.md](./app-layer-wire.md)、
> [canonical-owners.md](./canonical-owners.md)、[渐进接入指南](../integration/incremental-adoption.md)

**本方案解决什么**：R1–R8 把「底座」从一个 god object 里**析取**出来了——这件事已完成。
但析取出来 ≠ 兑现了两个产品承诺。本方案处理的是**析取之后的兑现问题**：

- 「任意宿主底座」今天实际仍是「任意 **Tauri** 宿主底座」——传输层绑死 Tauri，底座能力面缺四个域。
- 「多插件框架」今天实际是「**单插件运行时**框架」——没有安装入口、没有 per-plugin 配额、扩展点不驱动 UI。

本文所有「现状」均为 **2026-09-24 实测**（路径 + 符号名），不是文档推断。凡文档与代码冲突处以代码为准，并在 §2 标出。

---

## 0. 基线：轮 12 收口时站在这里

| 指标 | 实测值 | 出处 |
|---|---|---|
| Rust 测试 | **1221 / 0 failed**（15 crate） | 轮 12 收口 |
| TS 测试 | **1571 / 0 failed**（97 文件，21 包） | 轮 12 收口 |
| 跨语言门禁 wire-gate | **125 / 125**（当轮计数口径，见下方口径注） | `packages/tauron-contract-tests/src/wire-gate.test.ts` |
| 命令面（本方案新增 `host_capabilities` 后） | **59 = 底座 39 + 插件运行时 20**（0.4 实测复核，含 0.4-A1 三命令；`plugin-install` feature 另注册 2 条） | `tauron_substrate_handler!` / `tauron_plugin_handler!` |
| 能力表 | 16（13 插件面 + 3 特权） | `authz::COMMANDS` / `capabilities.ts` |
| 底座独立装配 | ✅ 有编译证据 + 功能证据 | `substrate-only` feature + `substrate_only_host_is_functionally_complete` |

> **数字口径**：上表是**轮 12 收口时**的快照，作为本方案的比较基准，不随后续改动刷新。
> 当前实测（2026-09-25，三轮全链路审计之后）：TS **1626**（`pnpm -r test` 实跑，98 文件）；
> Rust 执行结果 **1251 passed / 0 failed**（`cargo test --workspace --locked --lib --tests`，
> 15 suite），源码 `#[test]` 声明数 **1284**——**声明数不是执行结果**，feature 门控
> （`tauron-adapter` / `tauron-shell` 的 `tauri` feature）另计，真实执行结果以 CI 为准；
> wire-gate **110**（`vitest run src/wire-gate.test.ts` 实跑；基线表的 125 是更早的计数
> 口径，那个口径下同一文件曾按包含 `contract.test.ts` 的整包计）。
> 本节只陈述口径，不再回头改基线表的数字。

### 当前执行快照（2026-09-26）

本表记录实现状态，不替代下文各项验收条件；只有“完成”且满足对应门禁才可关闭路线项。

| 项 | 状态 | 当前证据 / 剩余工作 |
|---|---|---|
| S3 桩命令 `Unsupported` | 部分完成 | `UnsupportedBody` 与部分命令降级已接线；全域降级分支仍需逐条核对并完成门禁。 |
| S4 孤儿归置 | 部分完成 | `canonical-owners.md` 已登记七个 crate；ACL 与 market 有可选安装 feature 的接线，但宿主生产配置与其余孤儿 crate 归置仍缺。 |
| S6 能力协商 | 部分完成 | `host_capabilities` 返回当前 handler 命令集与不支持域；provider 命令和运行时装配尚未实现。 |
| S1 传输层 | 部分完成 | TS `MemoryTransport` 可驱动 `ShellClient`；Rust `ChannelSink` 与 Tauri 传输仍未抽成可替换帧接口。 |
| M2 资源配额 | 基本完成 | 通知 / pending / 流 / 订阅均有插件与全局上限；通知溢出优先裁剪占用最多插件的最旧条目。需继续核实所有配额门禁与统计字段。 |
| M8 可观测性 | 部分完成 | 主窗 `host_resource_stats` 暴露占用和配额，通知淘汰数可见；调用量、失败量、平均时长、pending 峰值和队列深度未实现。 |
| M5 SDK 收敛 | 部分完成 | `PluginJsRuntime` 已从 `@tauron/host` 公共入口移除；legacy / 主推 SDK 入口定位与全链路收敛尚未完成。 |
| M7 更新 | 部分完成 | `simulated` 不会在 UI 冒充可用更新；真实 HTTP 下载、验签、替换与回滚未实现。 |
| M1 安装链路 | 部分完成 | CLI `.tpkg` 签名协议、Rust 验签/manifest 校验、feature-gated 安装、权限审批 UI、原子目录落盘已接线；本轮增加受限只读 Tauri 插件资源协议、安装后启用/开窗、卸载事务回滚和路径越界测试。仍缺生产密钥安全存储/注入（示例仅提供环境变量入口且默认无密钥）、插件页面真实 SDK 调用/响应的端到端证据、卸载失败后暂存物清理保证。 |
| S2 / S5 / M3 / M4 / M6 / X1 | 未完成 | Provider 命令、权限文件生成、各插件形态执行/拒绝契约、contributes 对账和 UI 派发、跨插件双向声明、完整诚实性门禁仍需实现。 |

截至本快照，未修改版本号、未提交、未创建发布 tag。发布条件以 §5 全部验收项为准。

**已达成的硬不变量**（不得回退，本方案所有改动都必须保持）：

1. 底座命令在类型上触达不到注册表（`SubstrateState` 无 registry 字段）。
2. `Plugin Runtime → Substrate` 单向依赖，反向不成立。
3. `@tauron/ui-primitives` 不依赖 `@tauron/host`；`@tauron/host` 不静态引入 UI 包。
4. 全部 `cmd_*` 函数体零直接 Tauri 调用（平台调用一律经 `trait *Sink`）。
5. 身份判定在代码里（不只是 ACL 配置里），且判定点在任何副作用之前。
6. 两套错误码不合并，但穿越边界必经 `translate_at_boundary`。

---

## 1. 成熟度记分卡：两条产品线分别打分

打分口径：**0 = 不存在 / 30 = 有接口无实现或有实现无入口 / 60 = 主链路可用但有硬缺口 /
85 = 生产可用 / 95 = 生产可用且有对账与可观测**。

### 1.1 任意宿主底座线

| 维度 | 现状 | 目标 | 判据（实测） |
|---|:--:|:--:|---|
| 底座可独立装配 | **90** | 95 | 已有编译 + 功能双证据 |
| 平台无关性（核心逻辑） | **70** | 85 | `cmd_*` 零直调 Tauri ✅；但 `tauri.rs` 的 50 个 `pub fn host_*` 普遍注入 `WebviewWindow`，`ChannelSink` 硬编码 `tauri::Wry`（`tauri.rs:288-302`） |
| **传输可替换性** | **30** | 90 | `Backend` 只有 `TauriBackend` + `MockBackend` 两个实现；`channel()` 语义绑死 Tauri `Channel`；`principal()` 依赖 webview label 前缀 |
| 底座命令面完整性 | **55** | 85 | `host_menu_*` / `host_tray_*` / `host_fs_*` / `host_http_*` / `host_updater_*` **全仓 0 命中** |
| 底座能力诚实度 | **50** | 95 | 10 条命令是桩，其中 dialog 三命令恒返回「取消 / false / Ok(())」而非「不支持」 |
| 部署可配置性 | **40** | 90 | `crates/tauron-adapter/permissions/` 不存在；仅示例有一份 capabilities |
| 孤儿 crate 处置 | **15** | 80 | 7 个 crate 零消费者：acl / brand / theme / market / distribute / wasm / shell |

### 1.2 多插件框架线

| 维度 | 现状 | 目标 | 判据（实测） |
|---|:--:|:--:|---|
| **插件安装链路** | **20** | 90 | 命令面**无 install**（`RegistryAdminOp` 只有 Disable/Enable/Uninstall/Purge）；`Registry::install` 的调用点全在测试里；market 三命令恒 `simulated:true` |
| **per-plugin 资源隔离** | **25** | 85 | 事件队列 1000 是「每订阅者×每通道」但无 per-plugin 总量；通知环**全局共享 256**；流句柄表**无上限**；pending **全局 1000**；无内存/CPU 配额 |
| 插件形态覆盖 | **35** | 75 | Process ✅（tauron-proc）；Wasm ❌（`E_PLUGIN_TYPE_NO_RUNTIME`）；**Js ❌ 无执行器**（`entry.js` 只做非空校验，零消费点）；**Rust ❌ 完全无**（`PluginType::Rust` 仅出现在 manifest 校验与一个测试里） |
| 扩展点 contributes | **40** | 85 | 能 register / list，但**无对账**（`cmd_contributes_reconcile` 与 `E_CONTRIBUTES_DRIFT` 只存在于方案文档）；命令面板**没有任何代码把 contributes 灌进去** |
| 插件 SDK 一致性 | **55** | 90 | 契约包两端都实现了，但**开发者实际面对四套体验**（见 §2.2-M5） |
| 生命周期闭合 | **75** | 95 | 卸载/禁用/崩溃恢复闭合（卸载回收 6 类旁路状态）；安装/更新缺 |
| 多插件 UI | **45** | 80 | `PluginManagerStore` 启停/卸载真调命令 ✅；无安装入口；`oc-plugin-manager` 是哑组件 |
| 市场 / 更新链 | **10** | 60 | 全桩，不下载/不验签/不换文件，只改进程内 `updateState` 字符串 |

> **两条线的共同诊断**：R1–R8 解决的是「结构」问题（谁能依赖谁、状态怎么拆），本方案要解决的
> 是**「通路上有类型、有接口、没有入口」**这一类问题——它比"没实现"更隐蔽，因为编译过、测试过、
> 文档还写着，但线上永远走不到。轮 9–12 已经抓出 8 处这类断链，本方案预判还有同类存在，
> 因此每项改进都必须配「入口可达性」判据，而不只是「函数存在」。

---

## 2. 残差清单（实测，逐条带证据）

> ⚠️ **本节是 2026-09-24 口径的快照，已被 2026-09-26 全仓重测部分推翻**（轮 19 复核）。
> 逐条复核结论：
>
> | 条目 | 本节原判 | 2026-09-26 实测 |
> |---|---|---|
> | M-1 无 install 入口 | 🔴 | **已实现**：`host_registry_install` / `_preview` 存在（`tauri.rs:2389-2392`），但挂在 `#[cfg(feature = "plugin-install")]` 下且 `default = []` → **默认构建不可达**；TS 侧能力表曾无条件列出（误报已注册），已由 0.4-A2 修 |
> | M-2 无任何 per-plugin 配额 | 🔴 | **已接线 4 类**：pending 100/插件（`registry.rs:33`）、stream 32/插件（`stream.rs:113`）、订阅 256/插件（`eventbus.rs:52`）、通知 64/插件（`tauron-notify/src/lib.rs:161`） |
> | S-6 无能力协商 | 🟠 | **已实现** `host_capabilities`（`lib.rs:382`），但 `families` / `unsupported` 域名仍硬编码（`lib.rs:396-415`） |
> | S-3 桩命令谎报 | 🔴 | **已修**：dialog / clipboard / brand / market 全部返回有类型的 `UnsupportedBody` / `simulated`，无裸成功 |
> | S-4 剪贴板 | 🟠 | 保持（进程内回退，已诚实标注 `fallback: in-process-buffer`） |
> | S-1 传输层 | 🔴 | `MemoryTransport` 已有但**不完整**（无流式内核、无取件泵）→ S1 验收条件实际未达成 |
> | S-2 命令面缺 5 域 | 🔴 | **未变**：menu / tray / fs / http / updater 仍全仓 0 命中 |
> | S-7 / S-8 / M-3~M-10 | — | **未变** |
>
> **新发现的断链不在本节**（调用投递不存在、框架层链路死、进程插件不能通信等），
> 统一登记在 [capability-closure-plan.md](./capability-closure-plan.md) §6。
> 本节保留作为历史口径，不再作为选型依据。

### 2.1 任意宿主底座线

| # | 残差 | 证据 | 档 |
|---|---|---|---|
| S-1 | **传输层绑死 Tauri**：`Backend` 只有 Tauri + Mock 两实现；`channel()` 返回 Tauri `Channel`（`tauri-backend.ts:169-174`）；Rust 侧 `ChannelSink` 硬编码 `tauri::Wry` | `packages/tauron-host/src/backend.ts:57-90`、`tauri.rs:288-302` | 🔴 |
| S-2 | **命令面缺 5 域**：menu / tray / fs / http / updater 全仓 0 命中 | 全仓 grep `host_menu\|host_tray\|host_fs_\|host_http\|host_updater` | 🔴 |
| S-3 | **桩命令对外谎报**：`host_dialog_open/save` 恒 `Ok(None)`（=用户取消）、`host_dialog_confirm` 恒 `Ok(false)`、`host_dialog_message` 恒 `Ok(())` | `lib.rs:3848/3857/3886/3869` | 🔴 |
| S-4 | **剪贴板不是系统剪贴板**：`host_clipboard_write/read` 读写 `shell_ext.clipboard` 进程内字段 | `lib.rs:3543/3556` | 🟠 |
| S-5 | **`tauron-acl` 是孤儿且零 `use`**：权限授予/审批无生产调用点；`EventBus::approve` 调用点全在单测 | `crates/tauron-adapter/Cargo.toml` 无 acl；`eventbus.rs:844/1123/1232` | 🟠 |
| S-6 | **无能力协商**：前端无法问「这个底座支持什么」，只能调了才知道（`command not found` 或未定义行为） | 命令面无 `host_capabilities` | 🟠 |
| S-7 | **`permissions/` 缺失**：插件形态注册下 `plugin:tauron\|*` 在启用能力检查的宿主上无权限 | `crates/tauron-adapter/` 只有 `src/` | 🟠 |
| S-8 | **7 个孤儿 crate 无归置决策**：acl / brand / theme / market / distribute / wasm / shell | `crates/*/Cargo.toml` 依赖实测 | 🟡 |
| S-9 | `AutoUpdateClient.relaunch()` 调 `host_window_quit` 而非 `host_window_relaunch`——与轮 11 修的「重启必须对账」相矛盾 | `auto-update-client.ts:213-215` | 🟠 |

### 2.2 多插件框架线

| # | 残差 | 证据 | 档 |
|---|---|---|---|
| M-1 | ~~**无 install 入口**~~ → **已实现但默认不可达**（2026-09-26 改判）：`host_registry_install` / `_preview` 已存在并注册，但挂 `#[cfg(feature = "plugin-install")]` 且 `default = []`；TS 侧能力表原无条件列出（误报已注册），已由 0.4-A2 改为运行期 `host_capabilities` 开门 | `tauri.rs:2389-2392`、`Cargo.toml:12`；修正在 `capability-closure-plan.md` A2 | 🟠 |
| M-2 | ~~**无任何 per-plugin 配额**~~ → **已接线 4 类**（2026-09-26 改判）：pending 100/插件、stream 32/插件 + 256 全局、订阅 256/插件 + 4096 全局、通知 64/插件 + 512 全局。**剩余缺口**：通知环仍是全局单实例（逐插件只是裁剪）、无内存/CPU 配额 | `registry.rs:33`、`stream.rs:113/122`、`eventbus.rs:49/52`、`tauron-notify/src/lib.rs:129/161` | 🟡 |
| M-3 | **Js 型插件无执行器**：`entry.js` 零消费点；真实路径是「宿主开窗 + 插件自举」，但文档没这么写 | `manifest.rs:344-350` | 🔴 |
| M-4 | **Rust 型插件完全无落点**：`PluginType::Rust` 仅出现在 manifest 校验与 1 个测试；插件开发指南的形态表**缺 Rust 整行** | `manifest.rs:335/392/732`、`registry.rs:1874`；`docs/api/plugin-development-guide.md:24-29` | 🔴 |
| M-5 | **四套插件开发体验并存**：legacy `PluginContext`(iframe) / `createContractContext`(沙箱，缺取件泵) / app-plugin-sdk(webview，有泵) / `PluginJsRuntime`(无生产调用点) | `plugin-context.ts:103`、`contract-context.ts:111`、`context.ts:47`、`plugin-js.ts` | 🟠 |
| M-6 | **contributes 不对账、不驱动 UI**：无 reconcile；`oc-command-select` 被 `ShellController` 明确不接（"执行属接入方域"） | `wire-gate.test.ts:1240/1265`、`shell-events/index.ts:50-54` | 🟠 |
| M-7 | **无 plugin-to-plugin 协作**：`cmd_plugin_call` 用 `resolve_self_identity` → 插件只能调自己；跨插件调用只会命中拒绝测试 | `lib.rs:1424`、`eventbus.rs:770` | 🟡 |
| M-8 | **市场/更新链全桩**：只改进程内 `updateState` 字符串，不下载/不验签/不换文件 | `lib.rs:3729/3752/3776` | 🟠 |
| M-9 | **两份注册表无桥接**：`@tauron/core` 的 `PluginRegistry`（`maxPlugins: 32`）与 Rust `tauron-host::Registry`（`max: 8`）互不感知 | `registry.ts:49`、`bootstrap.ts:148/207` | 🟡 |
| M-10 | **无多插件可观测**：无 per-plugin 调用数/耗时/错误/资源占用指标 | 命令面无 metrics | 🟡 |

---

## 3. 改进方案

编号规则：**S = 底座线，M = 多插件线，X = 横切**。
每条给：**问题 / 设计 / 不变量 / 门禁 / 验证 / 影响 / 量级**。

### 3.1 底座线

---

#### S1. 传输层抽象 `HostTransport`（解 S-1，底座线最高优先级）

**问题**：`Backend` 接口本身是干净的（6 个字段），但它只有 Tauri 一个真实现。三个地方把语义钉死在 Tauri 上：
① `channel()` 返回 Tauri `Channel`；② Rust 侧 `ChannelSink` 硬编码 `tauri::Wry`；
③ `principal()` 从 webview label 前缀解析。于是「任意宿主底座」实际是「任意 Tauri 宿主底座」。

**设计**：把传输契约从 Tauri 语义降到**帧语义**——

```ts
// 传输层契约：Tauri / Electron / WebSocket / postMessage / 测试 均可实现
export interface HostTransport {
  request<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  openChannel(): ChannelPort;              // 只承诺 onmessage(id, payload) + close()
  subscribe(event: string, h: (p: unknown) => void): Unlisten;
  identity(): Principal;                   // 传输自报，宿主校验
  readonly capabilities: readonly string[]; // 见 S6
}
```

Rust 侧配套：把 `ChannelSink` 的 `tauri::Wry` 换成 `trait FrameSink`（`send(frame) / close()`），
`ChannelSink` 只是它的 Tauri 实现。wire 层的 `WebviewWindow` 注入收敛为一个
`CallerSource` trait（`caller() -> Caller`），Tauri 实现读 label，其他实现按传输自报。

**不变量**：
- `cmd_*` 函数体继续零 Tauri 调用（已有门禁保持）。
- wire 层（tauri.rs）允许出现 Tauri 类型，但**只允许**出现在 `impl Trait for TauriXxx` 块内；
  命令**签名**里不得出现 `WebviewWindow`（改注入 `CallerSource`）。
- `Principal` 的来源可替换，但**校验权仍在宿主侧**——传输自报的身份必须经已有的
  `resolve_principal` 校验，不得直接采信。

**门禁**：
- 「`tauri.rs` 的 `pub fn host_*` 签名不得出现 `WebviewWindow`」
- 「`tauri::Wry` 只允许出现在 `ChannelSink` 实现块内」
- 「非 Tauri 传输必须存在一个可编译的替代实现」（新增 `MemoryTransport` 参考实现 + 编译期断言）

**验证**：用 `MemoryTransport`（进程内直通，无 Tauri）跑通档 1 底座 39 条命令的等价路径；
`HostClient` 在 Tauri 与 Memory 两种传输下行为逐项相等。

**影响**：Electron / 纯 Web / 移动 WebView / Node CLI 宿主只需实现 6 个方法即可接入整个底座，
这是「任意宿主底座」这个承诺的兑现点。**量级：大（1 轮）**。

---

#### S2. 能力 Provider 模式补齐四域（解 S-2，含 menu / tray / fs / http / updater）

**问题**：menu / tray / fs / http / updater 五个域全缺。直接补会撞上两个约束：
① 每个域在 Tauri 下都要一个 `tauri-plugin-*`（新增依赖）；
② 不同宿主的能力面天然不同（Web 宿主没有托盘）。

**设计**：不直接补命令，而先补**模型**——引入 `CapabilityProvider`：

```rust
// 宿主按能力注册 provider，tauron 只做编排 + 线协议 + 授权，不自带平台实现
pub trait MenuProvider: Send + Sync {
    fn set(&self, items: &[MenuItem]) -> HostResult<()>;
    fn on_select(&self, id: &str) -> HostResult<()>;
    fn native_supported(&self) -> bool;     // 沿用 R8 的诚实约定
}
```

- 每个域一组命令（`host_menu_set` / `host_menu_on_select` …），**命令属于底座集合**，
  未注册 provider 时返回 `Unsupported`（见 S3 的统一形状），而不是恒成功。
- Tauri 实现放在 feature-gated 的 binding 层（允许新增 `tauri-plugin-*` 依赖，见 §4 约束变更）。
- 优先级排序：**fs > http > updater > menu > tray**。
  fs/http 是插件刚需（几乎所有插件都要读写与联网），且 Tauri 有官方插件；
  menu/tray 是宿主形态相关，可延后。

**不变量**：provider 缺省 = 该域 `native_supported() == false`，命令返回 `Unsupported`，
绝不返回「看起来成功」的空形状（这是 R8 已经确立的约定，推广到全部新域）。

**门禁**：
- 「新增域命令必须成对注册且经 `Unsupported` 分支」
- 「provider 不得在核心 `cmd_*` 里被直接构造，必须来自 `SubstrateState` 注入」
- 「能力表必须同步新增条目」（否则 `@tauron/app-cli` 脚手架的 capabilities.json 会漏，轮 11 踩过）

**验证**：注入记录型 provider → 命令到达 provider；不注入 → 返回 `Unsupported { reason }` 且前端可读。

**影响**：底座能力面从 55 → 85；插件第一次能读写文件与发网络请求。**量级：大（2 轮，按域拆）**。

---

#### S3. 桩命令统一为 `Unsupported` 契约（解 S-3/S-4，诚实性）

**问题**：`host_dialog_*` 三命令恒返回「取消 / false / Ok(())」——**这不是降级，这是谎报**：
调用方无法区分「用户取消了」与「平台根本没弹框」。`host_clipboard_*` 同理（写的是进程内字段，
换个 webview 就读不到，却返回成功）。

**设计**：定义统一的线形状并强制所有降级路径使用它：

```rust
pub struct UnsupportedBody {
    pub supported: bool,        // 恒 false
    pub reason: String,         // 必须非空，说明缺什么
    pub fallback: Option<String>, // 已做的降级动作（如 "in-process-buffer"）
}
```

适用范围：dialog / deep_link / clipboard / brand / market / relaunch（缺省 sink）/ 所有 S2 新域。
同时把「是否 simulated」从注释与文档提升为**类型字段**（R8 对 market 已经这么做了，推广之）。

**不变量**：`Ok(...)` 的成功形状**不得**用于表达"没做"。判定口径：命令返回成功形状时，
其副作用必须真实发生（写进了剪贴板 / 弹了框 / 换了文件）。

**门禁**：
- 「每条命令的返回值若含 `simulated`/`supported` 字段，必须为有类型结构体，不得 `json!` 裸构造」
- 「全仓禁止 `Ok(None)` / `Ok(false)` / `Ok(())` 出现在无 provider 的降级分支」（正则 + 白名单）
- 「剪贴板命令必须声明是否落到系统剪贴板」

**验证**：逐条桩命令的返回形状测试；去掉 provider 后前端能读出 `reason`。

**影响**：消除一类最坏的缺陷——调用方按"成功"继续往下走。**量级：中（0.5 轮）**。

---

#### S4. 孤儿 crate 归置决策（解 S-8）

**问题**：7 个 crate 零消费者。它们各自有完整实现与测试，但接入方无法从外部判断
「能不能用」。孤儿 crate 是**文档承诺与实际能力差**的主要来源。

**设计**：对每个 crate 给出**三选一**的显式决策，并写进 `canonical-owners.md` 的决策表：

| crate | 决策 | 理由 / 动作 |
|---|---|---|
| `tauron-acl` | **接线**（本轮） | 权限授予是 S-M1 安装链路的前置；无它，插件权限只能靠部署配置 |
| `tauron-brand` | **接线（读端）** | `host_brand_info` 恒 `{}` 是桩；品牌配置 crate 已完整，读端接线成本低 |
| `tauron-market` | **接线（取包）** | M-1 安装链路需要真实取包与验签；zip 净化/撤销清单已就绪 |
| `tauron-wasm` | **标注为可选组件 + 诚实失败** | 不引入 wasm 运行时（重依赖），保留配置校验；执行路径继续 `E_PLUGIN_TYPE_NO_RUNTIME` |
| `tauron-theme` | **标注为可选组件** | 前端主题可由 `@tauron/ui-primitives` 的 design tokens 承担，crate 侧等待消费者 |
| `tauron-distribute` | **标注为可选组件（CI 侧）** | 灰度策略与崩溃率停发属 CI 运维，不进客户端运行时；明确定位为「CI 工具链组件」 |
| `tauron-shell` | **阶段 2/3 迁移**（既有计划） | legacy 冻结中；按 `canonical-owners.md` 的三阶段走，不新增功能 |

**不变量**：每个 crate 的状态必须是 {已接线 / 可选组件 / 迁移中} 之一，
**不允许**存在状态未定的 crate。状态写进机器可读表，门禁校验。

**门禁**：「每个 crate 必须在归置表中有条目，且条目状态与 Cargo 依赖表一致」
（例：状态 = 已接线 但 `tauron-adapter/Cargo.toml` 无依赖 → 红）。

**验证**：归置表与依赖方向可交叉复核（同 `canonical-owners.md` 的做法）。

**影响**：接入方第一次能按表选型而不是靠猜。**量级：小（0.5 轮，不含接线工作量）**。

---

#### S5. 部署面补齐（解 S-7）

**问题**：`permissions/` 缺失导致插件形态注册在启用 ACL 的宿主上不可用。轮 12 把它登记为
"无法验证故不写"——这个判断是对的，但**缺口本身没消失**。

**设计**：
1. 补 `crates/tauron-adapter/permissions/` 的**默认权限集**（与 `authz::COMMANDS` 同源生成，
   不由手写，避免权限表与命令表漂移）。
2. 随仓提供**三档 capability 模板**（档 1 / 档 2 / 档 3，对应渐进接入指南），
   宿主复制即用。
3. 在渐进接入指南里补一节「启用 ACL 检查时的注意事项」，并明确
   `windows: ["main", "plugin-*"]` 为何不能省。
4. 诚实标注：权限文件的**运行期效果仍需真实宿主构建验证**，本仓只提供编译期与
   `tauri-build` 生成物层面的证据（同轮 12 对示例 capabilities 的做法）。

**不变量**：权限条目必须**生成**而非手写；生成源 = `authz::COMMANDS` + 能力表（单一真相）。

**门禁**：「`permissions/` 的权限集合 == `authz::COMMANDS`」（防漂移，与 wire-gate 同机制）。

**验证**：`tauri-build` 生成物包含这些权限；示例在插件形态下 `cargo check` 通过。

**影响**：插件形态注册从"理论上可配"变成"随仓可配"。**量级：中（0.5 轮）**。

---

#### S6. 底座能力协商 `host_capabilities`（解 S-6）

**问题**：底座宿主能力面不同（有的没托盘、有的没 fs），前端却无从得知，只能调了才知道。
这是「任意宿主底座」在**运行时**层面的缺失——S1 解决编译期可替换，S6 解决运行期可协商。

**设计**：新增一条底座命令 `host_capabilities`，返回：

```rust
pub struct CapabilitiesBody {
    pub families: Vec<String>,       // 已注册的命令族（shell/ipc/i18n/notify/settings/recovery/...）
    pub commands: Vec<String>,       // 实际可达命令全集
    pub unsupported: Vec<UnsupportedDomain>, // { domain, reason } —— 复用 S3 的形状
    pub pluginRuntime: bool,         // 是否装配了插件运行时（档 1 vs 档 3）
}
```

TS 侧 `Backend.capabilities()` 已有字段，改为**启动时拉取一次并缓存**，
`HostClient` 提供 `supports(cmd)` 谓词；UI 组件按能力决定是否渲染（无托盘则不显示托盘菜单项）。

**不变量**：返回值必须由**实际注册的 handler 集合**与**已注入的 provider 集合**推导，
不得硬编码清单（否则又是一份会漂移的真相）。

**门禁**：「`host_capabilities` 的 `commands` 集合 == 两个 handler 宏的并集（按当前装配形态）」。

**验证**：档 1 装配返回 39 条且 `pluginRuntime: false`；档 3 返回 55 条且 `true`；
注入/不注入 provider 时 `unsupported` 相应变化。

**影响**：底座宿主第一次能"自述能力"，UI 可按底座形态自适应。**量级：小（0.5 轮）**。

---

### 3.2 多插件框架线

---

#### M1. 插件安装链路闭环（解 M-1，多插件线最高优先级）

**问题**：这是多插件框架最大的洞——**插件装不进来**。现状：
`Registry::install` 存在但调用点全在测试里；命令面的 `RegistryAdminOp` 只有
Disable/Enable/Uninstall/Purge；market 三命令恒 `simulated:true`，只改进程内字符串；
UI 无安装入口。也就是说今天的插件只能"生而有之"（宿主预置），用户装不了新插件。

**设计**：把安装做成一条显式流水线，每步都有可验证产物：

```
取包 → 验签 → 清单校验 → 权限授予（ACL）→ 落盘 → 注册表 install → 首次启用（受恢复引擎管辖）
```

1. **取包**：`tauron-market` 补真实取包客户端（本地 `.tpkg` 先行，HTTP 次之）。
   zip 净化（路径穿越/解压炸弹/条目数/压缩比）**已实现**，直接复用。
2. **验签**：接入 `@tauron/market` 已有的 **Ed25519**（TS 侧真实现）+ `tauron-acl` 的 HMAC 授予签名。
   验签失败 = 安装中止，**不留半安装状态**。
3. **权限授予**：`tauron-acl` 接线（S4）——manifest 声明的权限 → 审批行 → 授予集落盘 →
   可物化为 Tauri Capability（acl crate 的 `to_capability` 已有）。
   权限**扩张**才需重审（acl 的 diff 语义已实现，直接复用）。
4. **安装命令**：`host_registry_install`（privileged，仅主窗）+ `host_registry_install_progress`
   （走 S1 的流式通道回传阶段进度）。
5. **原子性**：落盘用 tmp + rename；任一步失败则整体回滚，注册表不留条目。

**不变量**：
- 安装**必须**经验签与权限授予两步，缺一不可（配置可关闭验签但必须显式声明，且 `simulated` 字段随之为真）。
- 半安装状态**不允许存在**：要么注册表有条目且文件完整，要么什么都没有。
- 安装进来的插件首次启用必须走恢复引擎管辖（避免一个坏插件每次启动都崩）。

**门禁**：
- 「`host_registry_install` 必须存在且属特权档」
- 「安装路径必须调用 `tauron-acl` 的授予检查」（锁住 S4 的接线，防回退成孤儿）
- 「`tauron-market` 必须被适配层 import」（同 P0-2 激活 tauron-proc 的做法）
- 「安装失败后注册表不得残留条目」（负向测试）

**验证**：本地 `.tpkg` → 安装 → 出现在 `registryList` → 启用 → 调用 → 卸载 → 清理干净；
篡改包体 → 验签失败且无残留；权限扩张 → 触发重审。

**影响**：多插件框架从"预置插件宿主"变成"可安装插件的框架"。**量级：大（2 轮）**。

---

#### M2. per-plugin 资源配额（解 M-2）

**问题**：今天所有上限都是全局的，且多数根本没上限。后果是**一个插件能拖垮整个宿主**：
刷通知挤掉别人的条目（环形 256 全局共享）、开无限流句柄、塞满 pending call 让所有插件调用超时。
多插件框架的底线是"邻居隔离"，这条现在是空的。

**设计**：双层配额——**per-plugin 上限 + 全局上限**，两者都配才有效。

| 资源 | 现状 | 目标 |
|---|---|---|
| 通知环 | 全局 256 | per-plugin 64 + 全局 512（全局满时按 LRU 淘汰**最吵的**插件，而非最旧的条目） |
| pending call | 全局 1000 | per-plugin 100 + 全局 2000 |
| 流句柄 | **无上限** | per-plugin 32 + 全局 256 |
| 事件队列 | 每订阅者×每通道 1000 | 保持，但新增 per-plugin 订阅数上限 256 |
| 订阅表 | 全局 4096 | 保持，新增 per-plugin 配额 |
| 内存/CPU | 无 | **不实现进程内配额**（做不到），改为记录 + 暴露指标（M8），并在文档写明 |

**不变量**：
- 配额超限**必须**返回明确的错误码（复用既有 `E_*`，不新增），且**不得**静默丢弃。
- 淘汰策略必须可观测（计入 stats），否则"为什么我的通知不见了"无从排查。
- 宿主（主窗）不受 per-plugin 配额限制——它是特权主体。

**门禁**：「每类资源必须同时存在 per-plugin 与全局两个常量，且被测试引用」
（防只加常量不接线，这类"有常量无逻辑"正是本仓库反复出现的问题）。

**验证**：插件 A 刷满自己的通知配额 → A 超限报错，宿主与插件 B 的通知不受影响；
反向对照：去掉 per-plugin 判定 → B 的条目被挤掉（证明判定真的在工作）。

**影响**：多插件场景从"能跑"到"能共跑"。**量级：中（1 轮）**。

---

#### M3. 插件形态：Rust 落点 + Js 诚实定义（解 M-3/M-4）

**问题**：四形态里只有 Process 有执行器。Js 实际是"宿主开窗 + 插件自举"（这条路是**真**的、
且身份判定完整），但文档写成"QuickJS-WASM 沙箱"，代码里 `entry.js` 零消费点；
Rust 型**完全无落点**，插件开发指南的形态表连这行都没有。

**设计**：

- **Rust 型**：定义为**编译期静态注册**——宿主在构建时把插件实现注册进
  `PluginRuntimeState`（`register_native(id, impl)`），清单层校验 entry/abi，
  运行期直接调用，无 IPC、无进程、无 webview。这是最高效的形态，且不需要新依赖。
  未注册的 Rust 型插件 → `E_PLUGIN_TYPE_NO_RUNTIME`（与 wasm 同一诚实失败码）。
- **Js 型**：**承认现状并写进契约**——"宿主开窗 + 插件自举"就是 Js 型的正式模型，
  隔离来自 **webview 边界 + 逐命令身份判定**，不是进程内沙箱。
  同时删除 `entry.js` 的"校验但不消费"这种半吊子状态：要么消费它（作为自举脚本的声明），
  要么从清单必填里去掉。
- **Wasm**：维持 `E_PLUGIN_TYPE_NO_RUNTIME`，并在指南写明"引入运行时属可选组件（S4）"。
- **B+ 混合**：README 与竞品文档里的 "B+ 混合模式" 在代码里**不存在对应变体**
  （`PluginType` 只有四变体）→ 从所有对外文档删除或明确标为规划。

**不变量**：每个 `PluginType` 变体必须有 {执行器} 或 {诚实失败码 + 文档标注}，二选一，
不允许"清单合法但调即死"的第三态。

**门禁**：「`PluginType` 的每个变体必须有执行器分支或显式 `E_PLUGIN_TYPE_NO_RUNTIME` 返回」（遍历枚举）
+「对外文档的形态表必须与 `PluginType` 变体集合一致」。

**验证**：四种类型各跑一次 spawn/call，断言实际行为与文档表格逐行一致。

**影响**：消除 README「4+1 形态」与代码「1.5 形态」的落差。**量级：中（1 轮）**。

---

#### M4. contributes 对账 + 驱动 UI（解 M-6，含方案遗留 A4）

**问题**：contributes 能注册能列出，但 ① 不与 manifest 声明对账（A4 从方案轮 10 起就未做），
② 不驱动任何 UI——`oc-command-select` 被 `ShellController` 明确标为"执行属接入方域"，
结果命令面板是个空壳。

**设计**：
1. **对账**：新增 `host_contributes_reconcile`（self 档），比对 manifest 声明与 activate 期注册，
   分叉返回 `E_CONTRIBUTES_DRIFT`（新增错误码，追加到枚举末尾——遵守既有约定）。
   对账触发点：安装完成时、启用时、`registry_list` 时（只读比对，不改状态）。
2. **驱动 UI**：`ShellController` 接上 `oc-command-select` → 查 contributes 注册表 →
   派发到对应插件的 `host_plugin_call`（命令类）或宿主动作（宿主类）。
   `oc-plugin-manager` 从哑组件升级为可装载 contributes 列表的容器。
3. **宿主 contributes**：允许宿主自己注册 contributes（`host_contributes_register` 的
   主窗分支），使"宿主内置命令"与"插件命令"在命令面板里同构。

**不变量**：contributes 的**声明**（manifest，安装期 ACL 依据）与**注册**（activate 期）
必须可比对；比对结果只报不分叉（不自动修复，修复属插件作者职责）。

**门禁**：「`cmd_contributes_reconcile` 必须存在且被两个 handler 宏之一注册」
+「`oc-command-select` 必须有归属」（复用轮 7 建的"监听方不得多于派发方"门禁）。

**验证**：声明 3 条注册 2 条 → 检出 drift 且不影响已注册的 2 条；命令面板选中 → 真的调到插件。

**影响**：扩展点第一次闭环（声明 → 注册 → 对账 → 呈现 → 执行）。**量级：中（1 轮）**。

---

#### M5. 插件 SDK 收敛为两套（解 M-5/M-9）

**问题**：开发者实际面对**四套**体验：legacy iframe 版、`createContractContext` 沙箱版
（缺取件泵——订阅即死，这是实打实的功能缺陷）、app-plugin-sdk webview 版（有泵）、
`PluginJsRuntime`（仓内零生产调用点）。另有两份无桥接的注册表（core 的 32 vs host 的 8）。

**设计**：
- **主推一套**：app-plugin-sdk（webview + 内置取件泵）为默认开发者体验，文档只教这一套。
- **保留一套**：`plugin-sdk` legacy，标注为 **deprecated / 仅供存量插件**，
  并**补上取件泵**（否则它是个"订阅即死"的陷阱）或明确标注"不支持订阅，仅支持一次性调用"。
- **删除**：`PluginJsRuntime`（零调用点，属孤儿逻辑，与 S4 的孤儿处置同一原则）。
- **注册表桥接**：`@tauron/core` 的 `PluginRegistry`（前端视图缓存，`maxPlugins: 32`）
  明确定义为**前端镜像**，其上界必须 ≥ Rust 侧上界（8），且启动时从
  `host_registry_list` 同步；两边上界的因果关系写进注释（镜像不得比真源更严格，
  否则前端会拒绝宿主允许的插件）。

**不变量**：任何对外导出的插件开发入口都必须在文档里有明确定位
（{主推 / deprecated / 内部}）；零调用点的导出一律删除或标注。

**门禁**：「`@tauron/host` 不得导出零调用点的插件运行时类型」
+「前端注册表上界 ≥ Rust 上界」（数值断言，防两份表各自漂移）。

**验证**：同一份插件定义在主推 SDK 下完整走通（注册/调用/订阅/取件/设置页）；
legacy SDK 的行为与文档标注一致。

**影响**：开发者第一公里不再有四个入口。**量级：中（1 轮）**。

---

#### M6. 跨插件协作模型（解 M-7）

**问题**：插件只能调自己。但真实插件生态需要协作（一个插件提供格式化服务，另一个调用）。
当前既不支持，也没写"不支持"。

**设计**：明确**支持，但必须显式**——
- 插件在 manifest 声明 `dependencies: [pluginId]`，安装期校验依赖存在。
- 新增 `host_call_plugin(target, method, args)`（self 档，但**目标不是自己**）：
  ① 校验 target 在调用方声明的依赖里；② 校验 target 声明了该 method 为 public contributes；
  ③ 计入调用方的配额（M2）；④ 走同一 pending/超时/取消机制。
- 未声明依赖的跨插件调用 → `E_AUTH_DENIED`（复用既有码）。
- 事件侧：跨插件订阅私有 topic 需 `public: true` **或**运行期审批（`EventBus::approve` 接线，S5 的 acl 同一条线）。

**不变量**：跨插件调用**必须**双向可声明（调用方声明依赖 + 被调方声明公开），
缺任何一侧都拒绝。不提供"匿名跨插件调用"。

**门禁**：「`host_call_plugin` 必须校验 manifest 依赖声明」（负向测试：去掉声明 → 拒绝）。

**验证**：A 声明依赖 B → 调用成功；去掉声明 → `E_AUTH_DENIED`；B 未公开 method → 拒绝。

**影响**：插件生态从"孤岛集合"变成"可协作生态"。**量级：中（1 轮，可与 M4 合并）**。

---

#### M7. 市场与更新链去桩（解 M-8）

**问题**：`host_market_check/download/install` 恒 `simulated:true`，只改进程内字符串。
更新链是"看起来有一条链，实际没有"。

**设计**：分两步，**先诚实、后真实**——
1. **诚实**（低成本，立即做）：`simulated` 字段已类型化（轮 11/12 完成），
   补上 `AutoUpdateClient` 对 `simulated` 的处理——`simulated: true` 时**不得**向用户展示
   "有新版本可更新"，而应展示"未接入更新源"。
   同时修 S-9：`relaunch()` 改调 `host_window_relaunch`（先对账、后重启）。
2. **真实**：`tauron-market` 补 HTTP 客户端 + Ed25519 验签 + 原子替换 + 回滚。
   与 M1 的安装流水线**共用**取包与验签两段代码（同一条链的两个入口：市场安装 / 手动安装）。

**不变量**：`simulated: true` 时 UI 不得呈现为可用的更新；`simulated: false` 路径必须有真实网络与验签调用。

**门禁**：「`AutoUpdateClient` 必须在 `simulated` 为真时不进入下载/安装分支」
+「market 三命令返回必须带 `simulated`」（已有，保持）。

**验证**：`simulated` 真 → UI 走"未接入"分支；接入 mock endpoint → 走真实分支并验证签名。

**影响**：更新链从"假可用"变成"可用或明确不可用"。**量级：中（与 M1 合并 1 轮）**。

---

#### M8. 多插件可观测性（解 M-10）

**问题**：没有 per-plugin 指标。多插件场景下"哪个插件慢/哪个在报错/哪个占满了队列"
完全靠猜——M2 的配额如果不可观测，超限就只是"莫名其妙失败"。

**设计**：只读暴露每个插件的 pending 调用 / 流句柄 / 订阅 / 通知占用及其配额，
并记录通知容量淘汰数。当前由主窗专属 `host_resource_stats` 返回全局和逐插件快照；
调用失败率、平均耗时和 pending 峰值仍是后续指标，不得描述为已实现。`oc-plugin-manager`
可消费该快照展示关键指标。

**不变量**：指标**只读**，不得影响判定逻辑（判定用真实状态，不用计数器的派生值）。

**门禁**：「新增的每项配额必须有对应的可观测字段」（与 M2 配套，防"限了但看不见"）。

**验证**：插件连打 10 次失败调用 → stats 反映 10 次失败且不影响其他插件。

**影响**：多插件运维从黑盒变灰盒。**量级：小（0.5 轮，依赖 M2）**。

---

### 3.3 横切

#### X1. 诚实性门禁扩展（防本方案自己制造新的落差）

- 对外文档（README / 插件开发指南 / 竞品文档）的**每条能力宣称**必须有对应代码符号，
  由门禁正则校验；无符号的必须标 ⚪ 规划。
- 全仓禁止 `Ok(None)` / `Ok(false)` / `Ok(())` / `success: true` 出现在未实现分支（S3  generalization）。
- 新增命令必须同时出现在：handler 宏、能力表、wire-gate 清单、`FRAMEWORK_COMMANDS`，
  四处缺一即红（轮 11 已建"无孤儿命令"门禁，扩展到能力表与 TS 常量表）。

#### X2. 约束变更声明（相对 R1–R8 的 §7）

R1–R8 的硬约束是「**无新 crate / 无新 npm 依赖**」。本方案**部分放宽**，但收紧边界：

| 层 | 约束 |
|---|---|
| 核心逻辑层（`tauron-host` 的 engine、`tauron-adapter` 的 `cmd_*`） | **仍然禁止**新增依赖 |
| binding 层（feature-gated，如 `tauri` feature 下的 `tauri-plugin-dialog`） | 允许新增平台依赖，但**必须**走 `trait *Provider` 抽象，且缺省降级实现不得引入该依赖 |
| 可选组件（wasm 运行时、市场 HTTP 客户端） | 允许新增，但**必须**为 feature-gated 且默认关闭 |

新增 crate 只允许出现在「可选组件」档，且不得进核心依赖路径。

---

## 4. 执行编排

依赖顺序原则：**先让底座可被替换（S1），再让插件能被装进来（M1），然后才是隔离与体验**。
理由是 M1 的安装流水线要用 S1 的流式通道回传进度，M2 的配额要在 M1 之后才有意义
（没有安装就没有"多插件"可隔离）。

| 轮 | 内容 | 完成判据 |
|---|---|---|
| **轮 13** | S3 桩命令 `Unsupported` 契约 + S4 孤儿归置决策 + S6 能力协商 | 10 条桩命令返回有类型的 `Unsupported`；7 个 crate 状态入表；`host_capabilities` 返回实测命令集 |
| **轮 14** | **S1 传输层抽象**（底座线核心） | 存在非 Tauri 传输实现且跑通档 1 等价路径；`host_*` 签名无 `WebviewWindow` |
| **轮 15** | **M1 安装链路**（多插件线核心）+ M7 更新链去桩 | 本地 `.tpkg` 安装→启用→调用→卸载全通；验签失败无残留；market `simulated` 处理正确 |
| **轮 16** | **M2 per-plugin 配额** + M8 可观测 | 邻居隔离测试（A 超限不影响 B）；每项配额有可观测字段 |
| **轮 17** | M3 形态落点 + M4 contributes 对账与驱动 UI + M6 跨插件调用 | 四种类型行为与文档逐行一致；命令面板真能调到插件；跨插件调用需双向声明 |
| **轮 18** | S2 能力 Provider（fs → http → updater → menu → tray）+ S5 部署面 + M5 SDK 收敛 + X1 门禁 | fs/http 两域可用或明确 `Unsupported`；permissions/ 生成自 authz；SDK 收敛为两套 |

> **轮 14 与轮 15 可并行**（不同产品线、不同 crate），但两者都依赖轮 13 的 S3/S4
> （S4 把 `tauron-acl` 与 `tauron-market` 的接线决策定下来，M1 才有着落）。
>
> **每轮的门槛**（沿用 R1–R8）：`cargo check --workspace --all-targets` 0 警告；
> `cargo check/test -p tauron-adapter --features tauri` 全绿（feature 门控必须与主 check 并列）；
> `pnpm verify`（= build → typecheck → test，顺序不可换）；wire-gate 全绿。
> **每轮结束都是可发布态**，不留下半完成状态。

---

## 5. 验收标准（0.3 发布条件）

1. **任意宿主底座**：存在一个**非 Tauri** 的 `HostTransport` 实现（进程内 `MemoryTransport`），
   在其上跑通档 1 底座全部能力域（shell / settings / i18n / notify / recovery / 事件总线），
   且 `cmd_*` 与 wire 层均无 Tauri 类型泄漏。
2. **底座能力面**：fs 与 http 两域可用或返回有类型的 `Unsupported`；
   `host_capabilities` 返回值与实际注册命令集一致；无一条命令在降级时返回成功形状。
3. **多插件可安装**：本地 `.tpkg` 走完「取包 → 验签 → 权限授予 → 落盘 → 注册 → 启用 → 调用 → 卸载」，
   每步有可验证产物，任一步失败无残留。
4. **邻居隔离**：per-plugin 配额覆盖通知环 / pending / 流句柄 / 订阅数；
   A 插件耗尽自己的配额不影响宿主与 B 插件；每项配额可观测。
5. **插件形态诚实**：`PluginType` 四个变体每个都有执行器或 `E_PLUGIN_TYPE_NO_RUNTIME` + 文档标注；
   对外文档形态表与枚举一致（无"4+1"这类代码里不存在的宣称）。
6. **扩展点闭环**：contributes 声明 ↔ 注册可对账（drift 可检出）；
   命令面板/设置页由 contributes 驱动；跨插件调用需双向声明。
7. **SDK 收敛**：对外只有「主推」与「deprecated」两套插件开发入口；零调用点的导出已删除或标注。
8. **部署可配**：`permissions/` 由 `authz::COMMANDS` 生成；三档 capability 模板随仓；
   启用 ACL 的宿主在插件形态下可用（有编译期证据，运行期证据如实标注）。
9. **诚实披露**：所有未兑现能力在文档有兑现度标记；无"有类型、有接口、无入口"的第三态被
   描述为可用；本方案引入的每项能力都有入口可达性判据。
10. **全量 battery 绿**（同 §4 门槛），wire-gate 覆盖本方案 S/M/X 各项不变量。

---

## 6. 风险与回退

| 风险 | 缓解 |
|---|---|
| S1 传输抽象引入间接层，流式性能退化 | `request` 路径零成本抽象（泛型内联）；帧通道走传输原生能力，不做二次拷贝 |
| S2 引入 `tauri-plugin-*` 破坏"零依赖"承诺 | 严格限定在 feature-gated binding 层；缺省降级实现不引入依赖（沿用 R8 的 `native_supported` 约定） |
| M1 安装引入解压/验签/文件替换的攻击面 | 复用 `tauron-market` 已有的净化常量（路径穿越/解压炸弹/条目数/压缩比）；验签失败即中止；原子替换 + 回滚 |
| M2 配额设错导致正常插件被误限 | 配额可配置且缺省宽松；超限返回明确错误码而非静默丢弃；每项配额有可观测字段 |
| M6 跨插件调用打开新的越权面 | 双向声明（依赖 + 公开）才放行；复用既有 `E_AUTH_DENIED`；走同一 pending/超时/取消机制 |
| 轮次长，中途需发布 | 每轮结束都是可发布态（battery 绿 + 文档同步）；无半完成状态披露给接入方 |
| 本方案自己制造"有常量无逻辑"的新断链 | 每项不变量都配入口可达性门禁（不只是"函数存在"）；轮末做负向突变验证 |

---

## 7. 明确不做（边界声明）

- **不统一两套错误码**（`SC-xxxx` / `E_*`）。分层是有意设计，沿用 R2-c 的翻译边界约定。
- **不实现进程内 JS/WASM 沙箱运行时**。Js 型的隔离来自 webview 边界 + 身份判定；
  WASM 继续 `E_PLUGIN_TYPE_NO_RUNTIME`。需要运行时的接入方按 S4 的"可选组件"路径自行引入。
- **不做进程内内存/CPU 配额**。做不到可靠实现，改为记录与暴露指标（M8）+ 文档写明。
- **不删除 `tauron-shell`**。它是框架层协议门面且被 `@tauron/core` 与三个 adapter 消费，
  按 `canonical-owners.md` 的三阶段迁移走。
- **不改写已收口轮次的结论**。R1–R8 的六条硬不变量记在
  [overview.md](./overview.md)「架构演进 → R1–R8 析取出的底座边界」，只增不改。

---

## 8. 关联文档（本方案落地后需同步）

| 文档 | 需同步内容 |
|---|---|
| [overview.md](./overview.md) | 装配矩阵加入「传输层」维度；接线状态表按 S4 归置表更新 |
| [app-layer-wire.md](./app-layer-wire.md) | 新增 install / capabilities / menu / tray / fs / http 域的线格式条目 |
| [canonical-owners.md](./canonical-owners.md) | 新增 crate 归置状态表（S4） |
| [渐进接入指南](../integration/incremental-adoption.md) | 新增「非 Tauri 宿主」一节；档 1/2/3 补 capability 模板与 ACL 注意 |
| `docs/api/plugin-development-guide.md` | 形态表按 M3 重写（补 Rust 行、删 B+ 行、改 Js 描述）；安装流程按 M1 |
| `docs/competitive-analysis/*` | 头条特性兑现度按 M1–M8 重打 |
