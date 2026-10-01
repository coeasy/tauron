# tauron 全量架构重评估与重构方案（1.0）

> **状态**：重评估已完成；**W2 / W3 / W6 / W7 与 P1-10 已实施完毕，W1-b / W10 部分实施**，其余待做。
> 进度表见 §0.2。**不得据此宣称可发布。**
> **目标版本**：tauron 1.0（不再保留历史兼容负担——见 §12）
> **口径**：全部数字与结论为 **2026-09-26 本机实测**（命令附在每节）；文档与代码冲突处记文档漂移。
> **判据**：**「入口可达性」**，不是「函数存在」。本仓反复出现的缺陷类型是
> 「**有类型、有接口、有测试、没有入口**」；其次是「**命令登记了但最后一跳没送出去**」。
> **关联**：[overview.md](./overview.md)、[app-layer-wire.md](./app-layer-wire.md)、
> [canonical-owners.md](./canonical-owners.md)、[capability-closure-plan.md](./capability-closure-plan.md)（0.4，前置）、
> [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md)（0.3）

**本文与 0.4 方案的关系**：0.4 方案（A1–A8）**只落地了 A1–A3**（轮 20–22），
A4–A8 与轮 23–29 **未开工**；本文先做**整体重评估**（不假设 0.4 的判断仍然成立），
再给出**全量实现**方案——把 0.4 未做的部分、本轮新发现的缺陷、以及此前被列为
「明确不做」的事项（如进程内 WASM 运行时）一并纳入。

---

## 0.2 实施进度（2026-09-26 本轮实做）

| 工作流 | 状态 | 已落地的证据 |
|---|:--:|---|
| **W2 端到端可运行** | ✅ **完成** | `ui-primitives/src/wc.ts` 补 4 条副作用导入 → 示例构建产物注册组件数 **1 → 10**（含 `oc-plugin-manager`）；新增两道门禁：`ui-primitives` 的「源码定义的全部标签必须已注册」（22 tests）、`tauron-ui` 的「`@tauron/ui/wc` 必须注册壳组件」（7 tests） |
| **W7 运行时硬化** | ✅ **完成** | spawner：`SinkTable` 单锁消除 TOCTOU 泄漏、写帧改两级锁（不再全局阻塞）、`MAX_FRAME_BYTES` 单帧上限、EOF 主动 `try_wait` 回收 + 触发 `on_eof`；`process_delivery` 非幂等结算失败留痕；eventbus 锁序澄清。proc **17 → 22** tests |
| **W6 安装默认可达 + 签名强化** | ✅ **完成** | `tauron-adapter` 的 `default = ["plugin-install"]`；签名载荷升级 **v2**（元数据 + `path`/`size`/`hash` 全进签名）+ 有效期/时钟偏移校验 + 路径控制字符拒绝；两侧共用同一载荷定义（Rust `signing_payload` ↔ TS `signPayload`）。market **63 → 73** tests；adapter 默认特性下 **214 → 221** tests（端到端安装链路 fixture 已切 v2） |
| **W3 扩展点闭环** | ✅ **完成** | ① `ShellController` 接 `oc-command-select`（`contributesList` 解析归属 → `callPlugin` → `callTakeResult` 真投递）；② `UNWIRED_EVENTS` 显式登记表 + 门禁（替代「注释放行」）；③ **新增 `host_contributes_reconcile`（self 档）+ `E_CONTRIBUTES_DRIFT`**：比对 manifest 声明与 activate 期注册，分叉报错并点名缺哪条；SDK `createPlugin` 激活后真消费它（留痕不阻断）；命令面 17 → 18（插件面），adapter 命令面 59 → 60。host **283 → 284**、adapter **214 → 221**、host TS **367 → 368** tests；新增 wire-gate「0.4-W3 贡献对账闭环」 |
| **W1-b 单线收敛（TS 侧）** | ⚠️ **部分** | 已落：**删除死编排器 `bootstrap()`**（P1-1 的载体：它 import `@tauron/core` 三个**运行时类**，而本包 `sideEffects:false` → 真实构建里这些类不存在，一用即崩）；`@tauron/host` 的 `package.json` / `pnpm-lock.yaml` 移除 `@tauron/core` 依赖 → **活线不再依赖死线**；P1-5 的「TS `maxPlugins: 32` vs Rust `max_plugins: 8`」第二事实源随之消失，并加门禁「TS 不得出现硬编码 `maxPlugins:` 字面量」+「host 全目录不得 import `@tauron/core`」。**未落**：P1-2 两套 IPC 后端合表（两套是**两层协议**——框架层 `plugin_*` vs 应用层 `host_*`，合并需先定框架层存废，与 W1-a 同题）；P1-10 已单独完成（见下） |
| **P1-10 档位表生产自检** | ✅ **完成** | `authz_table_selfcheck()`（`OnceLock` 缓存、可断言）+ 在 `SubstrateState::with_adapter_config` 装配期调用，失败即 panic（构建缺陷不得带病运行）；新增 wire-gate 门禁「必须在**非测试**区域调用」。adapter +1 test |
| W1-a 单线收敛（Rust 侧） | ⬜ 待做 | 需先定框架层（`plugin_invoke` 三命令）的存废：`PluginDispatcher::dispatch` 是**同步**签名，而真实投递是**异步**（`host_plugin_call` + 取件），「复活」需设计有界阻塞桥或改协议语义——**这是设计决策，不是编码量** |
| W4 平台五域 Provider | ✅ **完成（R9）** | menu（3）/ tray（3）/ fs（6）/ http（1）/ updater（2）共 18 条命令进底座命令面，**当时** 57 条（现为 61，见 README 命令面口径）；menu/tray 在 `tauri` feature 下真实现，fs 走 `std::fs`（允许根内），updater 接 `tauron-distribute`，http 诚实降级为可注入 `HttpSink` |
| W5 插件形态四通（Native / Wasm） | ⬜ 待做 | — |
| W8 横切组件打通 | ⬜ 待做 | — |
| W9 内存传输统一 | ⬜ 待做 | — |
| W10 门禁与文档派生 | ⚠️ **部分** | 已落：命令面计数（59→60）在 7 份文档中同步；新增/改造 5 条门禁（贡献对账、档位表生产调用、host 不依赖 core、无 `maxPlugins` 字面量、UI 包静态引入全目录扫）。**未落**：形态表/能力清单由代码符号**自动推导**（当前仍是「改代码 + 改文档 + 门禁比对」） |

> 本轮只做「能在本环境验证到绿」的部分；未做的项**不做任何「已完成」表述**。
> 每项完成的判据都是**可复现的命令输出**，不是代码里"看起来有了"。
>
> **本轮实测绿**：`cargo check --workspace --all-targets` 0 警告；`cargo test --workspace --locked --lib --tests`
> 15 个测试二进制全绿（adapter 221 / host 284 / market 73 / proc 22）；`pnpm -r typecheck` 全绿；
> `eslint` 0 error；wire-gate 全绿（**计数以实跑为准**：2026-09-26 该快照为 142，
> 2026-09-27 发布收口复核为 **126**——该文件的用例数随门禁增删与用例合并而变）。

---

## 0. 方法：本轮重测的纪律

1. **不引用任何未经本轮实测的数字**。0.3/0.4 文档里的结论一律当作「待复核假设」。
   本轮实测已推翻两条旧结论（见 §2.4：`tauron-acl` / `tauron-market` 已不再是孤儿）。
2. **两条产品线分别评估**，不混在一张表里打分。
3. **门禁会假红也会假绿**：解析类辅助函数按字面串匹配会被 rustfmt 折行打穿（假红），
   解析到空数组若不断言就是假绿。凡结论只由门禁支撑者，本轮一律再用 grep 独立复核。
4. **诚实边界不计为缺陷**（`simulated: true` / `UnsupportedBody` / 「诚实边界」注释），
   但**「宣称可用」与「实际不可达」之间的落差一律计为缺陷**——包括文档宣称。

---

## 1. 项目是什么：功能全景（本轮实测）

**一句话**：Tauri 2 之上的**插件化桌面客户端基础设施**——不是应用，是一套给接入方
装配「带插件生态的桌面客户端」的地基。**15 个 Rust crate + 20 个 npm 包**。

### 1.1 对外承诺两件事，实测判定

| 承诺 | 落地形态 | 本轮实测 |
|---|---|:--|
| **任意宿主底座** | 与 UI 无关的宿主能力面（窗口 / 剪贴板 / 深链 / 对话框 / i18n / 通知 / 设置 / 崩溃恢复 / 事件总线 / 流式帧） | ⚠️ 实为「任意 **Tauri** 宿主底座」；且**底座能力面缺 5 个域**（fs / http / updater / menu / tray） |
| **多插件框架** | 插件可安装、可启停、可调用、可订阅、可贡献 UI、可回收 | ⚠️ **调用**这一跳已于轮 20 打通（A1）、**安装已默认可达**（W6，`default = ["plugin-install"]`，仍需配置 `plugin_install_dir`）；但**四种形态只有 2 种有执行器**、**贡献的 UI 填充仍由接入方喂数** |

### 1.2 功能域清单（真通 / 降级 / 未通）

| 域 | 关键命令 | 状态 | 本轮证据 |
|---|---|:--:|---|
| 窗口管理 | `host_window_*`（8 条） | ✅ 真通 | 真铸窗，label 恒 `plugin-<id>` |
| 生命周期 | `host_lifecycle_report` / `host_registry_admin` | ✅ 真通 | 10 态 × 18 事件状态机，宿主单一写入 |
| 事件总线 | `host_events_publish/subscribe/unsubscribe/drain` | ✅ 真通 | 三通道 + 每订阅者队列；**取件泵只在 `app-plugin-sdk`**（legacy SDK 无泵，订阅即死） |
| 流式帧 | `host_stream_open/write/close` | ✅ 真通 | seq 由宿主铸，终帧后句柄失效 |
| 崩溃恢复 | `host_recover_*` | ✅ 真通 | 持久化 + 安全模式 + 试验预算 |
| i18n | `host_i18n_*`（6 条） | ✅ 真通（持久化未接） | 引擎已接线；`to_json`/`from_json`/`remove_resource_bundle` **零外部调用者** |
| 通知 | `host_notify` / `host_notifications_*` | ✅ 真通（系统气泡降级） | 缓冲 + dispatchLog |
| 设置 | `host_settings_*` | ⚠️ 读写真通，**变更通知链整体孤儿** | `store.rs:111-155/325-340` 的 `ChangeEvent`/`Watcher`/`watch`/`broadcast`/`drain` 零外部调用者 |
| 资源配额 | `host_resource_stats` | ✅ 真通 | 4 类 per-plugin 配额 |
| **插件调用（跨主体）** | `host_call_plugin/result/take` | ✅ **轮 20 真通** | `call_delivery.rs` + `process_delivery.rs`；Js / Process 双通路生产装配 |
| **插件安装** | `host_registry_install(_preview)` | ✅ **默认可达（W6）** | `default = ["plugin-install"]`（`crates/tauron-adapter/Cargo.toml:21`）；`plugin_install_dir` 默认 `None`，未配置时**如实不可用**（`lib.rs:226-228`） |
| **进程插件通信** | `host_runtime_spawn/health` | ⚠️ 帧回路已通，**无真 sidecar 端到端证据** | 管道 + 读线程已接线；心跳监控**未实现**（诚实边界） |
| **扩展点 UI** | `host_contributes_register/list/reconcile` | 🟡 **对账已闭合（W3）** | `host_contributes_reconcile` + `E_CONTRIBUTES_DRIFT` 已在（`authz.rs:189`/`error.rs:97`/`tauri.rs:1421`）；`oc-command-select` 已由 `ShellController` 接管（`shell-controller.ts:203`）。剩余：命令面板**填充**仍由接入方喂数 |
| 对话框 / 剪贴板 / 品牌 / 商城 / 更新 | `host_dialog_*` / `host_clipboard_*` / `host_brand_info` / `host_market_*` | ⚠️ 诚实降级 | `UnsupportedBody` / `simulated: true` / `fallback: in-process-buffer` |
| **menu / tray / fs / http / updater** | — | ❌ **全仓 0 命中** | `grep host_fs_\|host_http_\|host_menu_\|host_tray_\|host_updater crates/tauron-adapter/src/tauri.rs` → 空 |

### 1.3 命令面（三方对账后的事实）

| 集合 | 条数 | 定义处 |
|---|---:|---|
| 底座 `tauron_substrate_handler!` | **61** | `crates/tauron-adapter/src/tauri.rs`（R9 五域补齐后 39→57；1.1 审批 3 条 + 生产就绪自检 1 条 → 61） |
| 插件运行时 `tauron_plugin_handler!` | **83**（底座 61 + 运行时 22；`plugin-install` 另 2 条 gated，**已进默认特性** → 默认 **85**） | 同上 |
| TS `FRAMEWORK_COMMANDS` / `OPTIONAL_FRAMEWORK_COMMANDS` | **85**（含 OPTIONAL 两条 install）/ **2** | `packages/tauron-host/src/tauri-backend.ts` |
| `authz::COMMANDS`（插件面档位表） | **19**（Self_ 17 + ScopedRead 2） | `crates/tauron-host/src/authz.rs` |
| `authz::ADMIN_COMMANDS`（主窗特权） | **4** | 同上 `:240-268` |

> 三方一致性由 `@tauron/contract-tests` 的 wire-gate 锁定（2026-09-27 复核：**126/126 绿**；本包合计 147 条）。
> **教训（已入纪律）**：不要用临时 python 正则去数命令面——窗口截断会产生假警报；
> 一律以 wire-gate 的解析器为准。

---

## 2. 架构：真实结构

### 2.1 三层，中间那层是死的

```
┌─ ① 框架层（对外协议门面）────────────────────────────┐
│  @tauron/types · core · plugin-sdk · adapter-react|vue|svelte  │
│  dual-world · shell-matrix · cli · market                      │
│  Rust: tauron-shell（plugin_invoke / cancel / emit 三命令）     │
│  状态：🔴 链路死——PluginDispatcher 生产实现仍为零（§5-P0-2）    │
└────────────────────────────────────────────────────────┘
┌─ ② 应用层（真正活着的那条线）────────────────────────┐
│  @tauron/host · framework · ui · ui-primitives · app-cli       │
│  app-plugin-sdk · app-contract-kit · shell-events              │
│  Rust: tauron-host（引擎）+ tauron-adapter（host_* 78）        │
└────────────────────────────────────────────────────────┘
┌─ ③ 平台层 ────────────────────────────────────────────┐
│  Tauri v2（唯一真实传输实现）                                  │
└────────────────────────────────────────────────────────┘
```

### 2.2 Rust crate 依赖实测（2026-09-26）

```
tauron-adapter ──→ tauron-host, tauron-i18n, tauron-notify, tauron-recovery,
                   tauron-settings, tauron-proc, tauron-acl, tauron-market
tauron-settings ─→ tauron-schema
tauron-acl ──────→ tauron-host
tauron-market ───→ tauron-host
零依赖者（孤儿）：tauron-brand / tauron-theme / tauron-wasm / tauron-distribute / tauron-shell
```

| crate | 规模 | 状态 | 判据 |
|---|--:|:--:|---|
| `tauron-brand` | 1368 行 | **孤儿** | `grep tauron_brand::` 全仓 0 命中 |
| `tauron-theme` | 630 行 | **孤儿** | 同上 |
| `tauron-wasm` | 1629 行 | **孤儿 + 无引擎** | `Cargo.toml` 无 `wasmtime`/`extism`/`wasmi`/`quickjs` |
| `tauron-distribute` | 1529 行 | **孤儿（CI 侧）** | 0 命中；属运维组件，可接受但要登记 |
| `tauron-shell` | 2380 行 | **孤儿 + legacy 冻结** | 示例 `Cargo.toml` 依赖它，但 `main.rs:273` 唯一引用是**注释** |

> **本轮推翻的旧结论**：0.4 方案 §2.2 把 `tauron-acl` / `tauron-market` 列为
> 「迁移中/孤儿」——**已过期**，两者现已在 `tauron-adapter/Cargo.toml` 依赖表内
> （`acl` 用于授权、`market` 用于安装验签）。这正是「残差表每次用前必须重测」的又一例证。

### 2.3 TS 包可达性实测（生产入口 = `examples/minimal-app/src/main.ts`）

| 状态 | 包 |
|---|---|
| ✅ 示例可达 | `@tauron/host` `@tauron/ui` `@tauron/ui-primitives` `@tauron/app-plugin-sdk` `@tauron/plugin-sdk` `@tauron/types` `@tauron/shell-events` |
| ⚠️ 仅被其它包消费 | `@tauron/market`（被 `app-cli/pack.ts`）、`@tauron/app-cli`/`@tauron/cli`（bin 工具，正常） |
| ❌ 仅本包测试可达 | `adapter-react` `adapter-vue` `adapter-svelte` `dual-world` `shell-matrix` `framework` `app-contract-kit` `plugin-context-contract` `contract-tests` |
| ❌ 运行时不可达（`sideEffects:false` + 无 import） | `@tauron/core` 的 `bootstrap.ts` 路径（`bootstrap.ts:32` 真运行时使用 core 三个类，但 `bootstrap` 自身只被自己的测试引用） |

### 2.4 主体链路：12 跳实测判定

定义：用户从打开客户端到用一个插件完成一次交互所必须经过的跳。

```
 L1  宿主装配    tauron_adapter::tauri::init()          ✅  main.rs
 L2  主窗启动    ShellClient + host_capabilities        ✅  能力采用链已接线（轮 22）
 L3  插件安装    host_registry_install                  ⚠️  feature-gated + install_dir 默认 None
 L4  插件启用    host_registry_admin(enable)            ✅
 L5  插件开窗    host_window_create                     ✅  label 恒 plugin-<id>
 L6  插件自举    app-plugin-sdk createPlugin            ✅  轮 22 已换主推 SDK
 L7  生命周期    host_lifecycle_report(ATTACH)          ✅
 L8  调用投递    host_call_plugin/result/take           ✅  轮 20 打通（Js + Process）
 L9  流式帧      host_stream_open/write/close           ✅
 L10 事件        publish/subscribe/drain                ✅  app-sdk 有泵；legacy 无泵（P2）
 L11 卸载回收    host_registry_admin(uninstall/purge)   ✅
 L12 崩溃恢复    host_recover_*                         ✅
 —   扩展点 UI   contributes → 命令面板                 ❌  无 reconcile、无驱动
```

**结论：12 跳中 10 跳真通、1 跳半通（L3）、1 跳外挂未通（扩展点 UI）。**
相比 0.4 方案实测的「8 通 4 断」，**调用投递（L8）与自举（L6）已修复**；
剩下的问题**不在链路本身，而在「最后一公里」**——见 §5-P0-1：
**唯一的可运行 app 里，插件管理 UI 根本没被注册**。

---

## 3. 实现细节：值得继承的硬机制（不得回退）

1. **跨语言线格式强制 camelCase**：跨 IPC 的 Rust 结构体一律
   `#[serde(rename_all = "camelCase", deny_unknown_fields)]`，由 wire-gate 扫源码锁定。
2. **两套错误码、零交集、只追加**：框架层 `SC-xxxx`、应用层 `E_*`；
   新码只能追加到枚举末尾（TS `HOST_ERROR_CODES` 按声明顺序比对）。
3. **身份从 webview label 解析，不信任入参**：self 档命令的 pluginId 只从
   `plugin-<id>` 解析，调用方传入一律忽略。
4. **底座类型上触达不到注册表**：`SubstrateState` 无 registry 字段，编译期隔离。
5. **`cmd_*` 零直接 Tauri 调用**：平台调用一律经 `trait *Sink` / `*Provider`。
6. **事件总线锁序**：`stats → topics → subs → topic_subscribers → approvals → queues`。
7. **诚实降级形状**：`UnsupportedBody{supported,reason,fallback}` / `DegradedValue` /
   `simulated` 字段——**成功形状不得用于表达「没做」**。这是本项目的核心竞争力，不得回退。
8. **pending 表只在满时 GC**：稳态不驱逐，保证过期条目返回 `E_CALL_TIMEOUT`（可重试）
   而非 `E_CALL_NOT_FOUND`（不可重试）。
9. **投递与簿记分离**（轮 20 新增）：`CallDelivery::deliver` 只接收 `&PendingCall`，
   不写 pending 表；投递失败不改 TTL 语义。
10. **per-plugin 配额 4 类**：pending 100 / stream 32 + 256 全局 / 订阅 256 + 4096 全局 /
    通知 512 全局 + 64 每插件。

---

## 4. 回答两个问题

### 4.1 核心功能是否全部实现？——**没有**

| 功能域 | 是否全实现 | 缺口 |
|---|:--:|---|
| 窗口 / 生命周期 / 事件 / 流式 / 恢复 / 通知 / 配额 | ✅ | — |
| 插件调用（含跨主体） | ✅ | Process 侧缺真 sidecar 端到端证据 |
| 插件安装 | ✅ **默认可达**（W6） | feature `plugin-install` 已进默认；`plugin_install_dir` 未配置时如实不可用。签名 payload 已升级 v2 覆盖元数据（§5-P0-8 已修） |
| 插件形态（4 种） | ⚠️ **2/4** | Js 靠开窗、Process 能起能聊；**Rust Native 无执行器、Wasm 无引擎** |
| 扩展点（contributes） | 🟡 **对账已闭合**（W3） | `host_contributes_reconcile` + `E_CONTRIBUTES_DRIFT` 已在；`oc-command-select` 已由 `ShellController` 接管。剩余：命令面板**填充**仍由接入方喂数 |
| 平台域 | ❌ **缺 5 域** | fs / http / updater / menu / tray |
| 设置 | ⚠️ | 读写通，变更通知链无出口 |
| 框架层（对外协议） | ❌ | `plugin_invoke` 从未被任何宿主注册 → 第三方集成恒拿 SC-9001 |
| UI 层 | ✅ **示例已可跑**（W2） | 示例构建产物注册组件数 1 → 10（含 `oc-plugin-manager`），两道门禁锁定（§5-P0-1 已修） |
| 横切组件（brand / theme / distribute / wasm） | ❌ | 4 个 crate 零入口 |

### 4.2 主体链路是否全部贯通？——**链路本身基本贯通，但「最后一公里」断了**

- **12 跳中 10 跳真通**（L8 调用投递已于轮 20 修复，这是 0.4 的核心成果）。
- 剩余缺口（**已比基线缩小**）：
  1. ~~**L3 安装**默认不可达~~ → **已修**（W6：`plugin-install` 进默认特性；仅剩 `plugin_install_dir` 需接入方配置）；
  2. ~~**扩展点 UI 完全未驱动**~~ → **对账已闭合**（W3：`oc-command-select` 已接线；其余 5 条 `oc-*` 已显式登记进 `UNWIRED_EVENTS`）；
  3. ~~**示例 app 的插件管理区不工作**~~ → **已修**（W2：`oc-plugin-manager` 等组件已注册）。

---

## 5. 不合理清单（全部带 `path:line`）

> **口径提醒**：本节是 **2026-09-26 重评估的基线快照**（施工前状态）。带 ✅ 标记的行
> **已闭环**（P0-1←W2、P0-3←W6、P0-6←W3、P0-7←W7、P0-8←W6；P0-2 仅部分）；
> **未加标记的行仍是未修状态**。判断某项是否已闭环，**以 §0.2 进度表为准**。

### P0（阻塞「核心功能已实现」这个判断）

| # | 问题 | 证据 | 为什么严重 |
|---|---|---|---|
| **P0-1** ✅ 已修（W2） | **唯一的可运行 app 里，插件管理 UI 完全惰性**：示例只 `import '@tauron/ui/wc'`，该入口只注册 `oc-toast`；`oc-plugin-manager` 等 6 个组件定义在 `wc-shell.ts`，仅经 `ui-primitives/index.ts` 暴露，示例从不 import | `examples/minimal-app/src/main.ts:14`；`packages/tauron-ui/src/wc.ts:5`；`packages/tauron-ui-primitives/src/wc.ts:195`（仅 `oc-toast`）；`wc-shell.ts:554-559`；构建产物 `dist/assets/main-*.js` 中 `customElements.define` **只有 `"oc-toast"`** | 「命令登记了但最后一跳没送出去」的教科书案例：`main.ts:152` 拿到的 `<oc-plugin-manager>` 永不 upgrade，`:162-185` 注册的 `oc-plugin-install/toggle/uninstall` 监听永不触发。**这是「插件管理器」这个功能在示例里的唯一落点** |
| **P0-2** ⚠️ 部分（W1-a 待做） | **框架层整条产品线不可用却按可用文档发布**：`PluginDispatcher` 生产实现为零 → `plugin_invoke` 恒返回 `SC-9001` | `crates/tauron-shell/src/dispatch.rs:278`（唯一 `impl` 是测试 `EchoDispatcher`）；示例 `main.rs:273` 唯一引用是**注释**；`examples/minimal-app/src-tauri/Cargo.toml` 却依赖 `tauron-shell` | 第三方按 README 集成必然拿到 SC-9001——对外承诺与实现的直接冲突 |
| **P0-3** | ~~**安装链路默认不可达**~~ → **已部分闭环**（1.0-W6）：feature `plugin-install` **已进默认特性**（`crates/tauron-adapter/Cargo.toml:21`）；剩余缺口是 `plugin_install_dir` 默认 `None`（未配置时安装仍明确不可用，属**诚实不可用**而非静默失败） | `crates/tauron-adapter/Cargo.toml:21`；`lib.rs:226-228` | 「多插件框架」的「装」这件事默认可达，但需接入方配置安装目录 |
| **P0-4** ✅ 已修（R9） | **底座缺 5 个域** → **已补齐**：menu（3）/ tray（3）/ fs（6）/ http（1）/ updater（2）共 18 条命令进底座命令面（39→57）；menu/tray 在 `tauri` feature 下真实现，fs 走 `std::fs`（允许根内），updater 接 `tauron-distribute`，http 诚实降级为可注入 `HttpSink` | `crates/tauron-adapter/src/tauri.rs` 宏体 |
| **P0-5** | **四种插件形态只有 2 种有执行器** | `lib.rs:1464-1469`（`PluginType → DeliveryKind`）；`lib.rs:1431-1446`（`default_deliveries` **只注册 Js + Process**） | `Rust → Native`、`Wasm → Wasm` 未注册 → 落 `UnwiredDelivery` → `E_PLUGIN_TYPE_NO_RUNTIME`。对外文档写「4+1 形态」，代码里三变体无执行器 |
| **P0-6** ✅ 已修（W3） | **contributes 不驱动 UI、无对账**：`host_contributes_reconcile` 与 `E_CONTRIBUTES_DRIFT` 全仓 0 命中 | `grep` 于 `crates/` + `packages/` | 扩展点声明了但没有消费者；`host_contributes_register` 只是 Vec 增删 |
| **P0-7** ✅ 已修（W7） | **spawner 三处真实缺陷**：① `register_frame_sink` 的 `closed` 检查与 `sinks.insert` 是**两把锁非原子** → 与读线程 EOF 的 `sinks.remove` + `closed.insert` 交错时 sink 永不回收（反复崩溃→重启无界累积）；② `write_frame` 持**全局** `stdin_writers` 锁做阻塞 `write_all` → 一个不读 stdin 的 sidecar 会阻塞**所有**进程的帧投递（无超时）；③ 读线程用 `BufReader::lines()` **无行长上限** | `crates/tauron-proc/src/spawner.rs:319-325` vs `:234-238`；`:300-306`；`:216-238` | ①是我在轮 21 引入 `closed` 集合时的修复不完整（TOCTOU 仍在）；②③是可被恶意/故障 sidecar 触发的可用性缺陷 |
| **P0-8** ✅ 已修（W6） | ~~**安装签名 payload 不覆盖元数据**~~：已升级为 **v2 载荷**——元数据 + `path`/`size`/`hash` 全进签名，两侧共用同一载荷定义（Rust `signing_payload` ↔ TS `signPayload`） | `crates/tauron-market/src/package_signature.rs`；§0.2 的 W6 行 | 原缺口（`issued_at` 可改写绕过有效期、`kid` 不经签名保护）已闭合 |

### P1（架构层面不合理）

> **口径提醒**：基线快照。带 ✅ 标记的行已闭环（P1-1 / P1-5←W1-b、P1-8←W3、
> P1-10←P1-10 工作流、P1-11←文档更正）；**未加标记的行仍是未修状态**。

| # | 问题 | 证据 |
|---|---|---|
| **P1-1** ✅ 已修（W1-b） | ~~**活线依赖死线**~~：`bootstrap.ts` 已删除（它 import `@tauron/core` 三个运行时类，而 host 声明 `sideEffects:false`）；`@tauron/host` 的依赖已移除 `@tauron/core`，并加门禁「host 全目录不得 import `@tauron/core`」 | §0.2 的 W1-b 行 |
| **P1-2** | **两套 IPC 后端 + 两套命令名表并存**：`core/src/tauri-backend.ts`（`plugin_*` 前缀，无生产消费者）vs `host/src/tauri-backend.ts`（示例实际使用，前缀 `''`） | `packages/tauron-core/src/tauri-backend.ts:33/101`；`packages/tauron-host/src/tauri-backend.ts:174` |
| **P1-3** | **MemoryTransport 无任何 host 命令族**：只有通用命令注册表，`host_stream_*` / `host_call_*` / `host_events_drain` 一律 `throw new Error('command not found: '+cmd)`；**无取件泵** | `packages/tauron-host/src/memory-transport.ts:20-41`（全文件 86 行）；对比 `MockBackend` 有真实流式内核（`backend.ts:212-272`） |
| **P1-4** | **`oc-*` 事件派发无监听者**：`oc-command-select` **已于 W3 接线**（`ShellController`）；其余 5 条（`oc-tray-item`、`oc-shortcut-change`、`oc-theme-change`、`oc-toast-action`、`oc-updater-dismiss`）**已显式登记进 `UNWIRED_EVENTS`** + 门禁（`unwired-events.test.ts`），不再是「写句注释就放行」 | 派发：`ui-primitives/src/wc-shell.ts:111/234/337/400`、`theme-picker.ts:341`、`wc.ts:161`；接线：`shell-controller.ts:92-154`；登记：`shell-events/src/index.ts` 的 `UNWIRED_EVENTS` |
| **P1-5** ✅ 已消除（W1-b） | ~~**两份注册表无桥接、上限各自硬编码且矛盾**~~：载体 `bootstrap.ts` 已删除（TS 侧 `maxPlugins: 32` 随之消失）；新增门禁「TS 不得出现硬编码 `maxPlugins:` 字面量」 | §0.2 的 W1-b 行 |
| **P1-6** | **设置变更通知链整体孤儿**：`ChangeEvent` / `Watcher` / `watch` / `unwatch` / `broadcast` / `drain` / `subscriber_count` 零外部调用者 | `crates/tauron-settings/src/store.rs:111-155/325-340` |
| **P1-7** | **4 个孤儿 crate + tauron-shell**（规模合计 7536 行） | §2.2 |
| **P1-8** ✅ 已修（W3） | ~~**门禁漏洞**：wire-gate 曾允许「接线 **或** 注释声明接入方域」→ 写句注释即放行~~：`UNWIRED_EVENTS` 显式登记表已落地（`packages/tauron-shell-events/src/index.ts`），由 `unwired-events.test.ts` 门禁锁定（2026-09-27 第 3 轮审计再加「事件名解析覆盖全部键」用例，堵住解析漏项假绿） | `packages/tauron-host/src/unwired-events.test.ts`；`shell-events/src/index.ts` |
| **P1-9** | **同义反复测试**：整文件只有 `expect(typeof x).toBe('function')` / `toBeDefined()` | `adapter-react/src/use-invoke.test.ts:5-7`、`use-event.test.ts:5-7`、`use-plugin-id.test.ts:5-11`；`adapter-vue` / `adapter-svelte` 同类；`tauron-ui/src/plugin-manager.test.ts:413-502`（数十条 `expect(designTokens.x).toBeDefined()`） |
| **P1-10** ✅ 已修（P1-10 工作流） | ~~**`validate_command_registry` 生产零调用**~~：已加 `authz_table_selfcheck()`（`OnceLock` 缓存）并在 `SubstrateState::with_adapter_config` 装配期调用（失败即 panic），新增 wire-gate 门禁「必须在非测试区域调用」 | `crates/tauron-host/src/authz.rs`；§0.2 的 P1-10 行 |
| **P1-11** ✅ 已修 | ~~**文档漂移**：`docs/architecture/README.md` 仍写「唯一的前瞻计划」是 0.3 方案、仍写「4+1 插件形态」「双世界隔离（QuickJS-WASM）」~~：已更正——文档地图指向 1.0 方案为「唯一的前瞻计划」，核心原则第 3/5 条改为「sandbox 是 fail-closed 模拟」+「以 `PluginType` 枚举为准、不存在 B+ 混合模式」 | `docs/architecture/README.md` 文档地图 + 核心架构原则 3/5 |

### P2（一致性、体验与收尾）

> **口径提醒**：同样为**基线快照**。**P2-1 / P2-2 / P2-3 / P2-4 已由 W7 修复**
> （`process_delivery` 非幂等结算失败留痕、eventbus 锁序澄清、EOF 主动 `try_wait` 回收
> + 触发 `on_eof`）；**P2-5 的 `ShellController` 已不再是死导出**（1.0-W3 起
> `examples/minimal-app/src/main.ts` 有生产实例化）。其余行仍为未修状态。

| # | 问题 | 证据 |
|---|---|---|
| P2-1 | `process_delivery.rs:134` 用 `let _ =` 吞掉 `settle_call` 全部错误（含伪造 `callId` 的 `E_CALL_NOT_FOUND`），非幂等失败无留痕 | `crates/tauron-adapter/src/process_delivery.rs:134` |
| P2-2 | `eventbus.rs:406-408` `subscribe` 先取 `approvals` 再取 `stats`，与声明锁序相反（**非嵌套**，不构成死锁，但违反自定纪律） | `crates/tauron-host/src/eventbus.rs:406-408` |
| P2-3 | `CommandSpawner::tracked` / `ProcessFrameSink::on_eof` 生产零调用（`on_eof` 缺省空实现，EOF 实际走 `closed` 集合） | `crates/tauron-proc/src/spawner.rs:111/167` |
| P2-4 | `children` 表只在 `is_alive`/`kill` 时清理；健康检查是惰性轮询 → 不轮询即无界累积 `Child` 句柄 | `crates/tauron-proc/src/spawner.rs:148`、`lib.rs:2951` |
| P2-5 | `@tauron/host` 大量死导出（除本包测试外无消费者）：`FrameSink`、`SIDECAR_ABI_CONTRACT`、`WindowState`、`LazyPluginLoader`、`DeepLinkClient`、`toHostRpc`、`LIFECYCLE_*`、`validateClientConfig` 族、`capabilityMatrix` 族、`GRANT_SET_*` 族、`translate_at_boundary` 族（**注**：`bootstrap` 已于 W1-b 删除；`ShellController` 已于 W3 由示例生产实例化，不再是死导出） | `packages/tauron-host/src/index.ts` |
| P2-6 | 各 crate 孤儿 public API（示例）：`tauron-market/api.rs:243/467/472`（整个 API 客户端）、`tauron-market/lib.rs:87-163`（`cmp_version`/`is_downgrade`/`is_monotonic`/`validate_zip_constants`）、`tauron-acl` 的 `build_approval_rows`/`load_unverified`/`to_capability`/`diff`、`tauron-host/config.rs:256-294`（模板族）、`tauron-notify/lib.rs:332-491`、`tauron-i18n/lib.rs:212/349/359`、`tauron-recovery/lib.rs:139-163/536` | 见对应文件行号 |
| P2-7 | `plugin sign` 产出 `algorithm: 'sha256-digest'`（诚实标注但非真签名）；`cli` 的 `dev`/`test`/`pack`/`publish` 返回 `success:false` | `capability-closure-plan.md` §4.2 记载，本轮未复测 |
| P2-8 | 3 个 UI adapter 零消费者（仅模板字符串提及） | `packages/tauron-cli/src/scaffold.ts:178/228` |

---

## 6. 全量重构方案：W 系列（Workstream）

> 命名说明：`W` = Workstream，与 0.3 的 `S/M/X`、0.4 的 `A` 不冲突。
> 每条给：**问题 / 设计 / 不变量 / 门禁 / 验证 / 量级**。
> **总原则**：**不再保留历史兼容负担**（用户决策）——允许删除 legacy 层、合并重复实现、
> 变更内部线格式；但 §3 的 10 条硬不变量与**对外协议语义**（`plugin_invoke` 三命令的
> 存在性与信封形状）保持，因为它们是「框架层」这个产品承诺的本体。

---

### W1. 单线架构收敛（解 P0-2 / P1-1 / P1-2 / P1-5 / P1-10）

**问题**：三条重复线并存——Rust 侧 `tauron-shell` 的 eventbus/registry/dispatch 与
`tauron-host` 同名模块重复；TS 侧 `@tauron/core` 的 `PluginRegistry/ConfigManager/EventBus`
与 `@tauron/host` 重复；两套 IPC 后端 + 两套命令名表。且活线（host）依赖死线（core）。

**设计**：

1. **canonical 唯一化**：`tauron-host`（Rust）/ `@tauron/host`（TS）为**唯一实现**。
   `tauron-shell` 只保留**协议门面**（三命令的签名与信封翻译），其
   `EventBus` / `PluginRegistry` / `HostState` 三套 legacy 引擎**删除**，
   `PluginDispatcher` 改为**委托** `tauron-host` 的引擎（原 0.4-A5 阶段 2/3）。
2. **解 P1-1**：把 `PluginRegistry` / `ConfigManager` / `EventBus` 从 `@tauron/core`
   迁到 `@tauron/host` 内部（或新建零依赖包 `@tauron/kernel`），
   `bootstrap.ts` 不再 import `@tauron/core` 运行时类。
3. **统一 IPC 后端**：删除 `@tauron/core/src/tauri-backend.ts`；
   `plugin_*` 与 `host_*` 两套命令名表由**一个** `COMMAND_SURFACE` 常量导出。
4. **注册表上限同源**：TS 不再硬编码 `maxPlugins: 32`，改为读 `host_capabilities`
   报告的 `registryMax`（Rust 侧已有 `max_plugins`）。
5. **档位表门禁进生产**：`validate_command_registry` 在 `AdapterState` 构造时调用一次
   （或由 `#[cfg(debug_assertions)]` 调用），使 P1-10 的「仅测试调用」不再成立。

**不变量**：`plugin_invoke` / `plugin_cancel` / `plugin_emit` 的**命令名与信封线格式不变**；
两套错误码仍不合并，穿越点走 `translate_at_boundary`。

**门禁**：
- 「`tauron-shell` 不得再出现 `struct EventBus` / `struct PluginRegistry` 的**定义**」
  （正则扫 `crates/tauron-shell/src/*.rs`，只允许 `use` 与门面）。
- 「`@tauron/host` 不得 import `@tauron/core` 的运行时类」（现有门禁，扩展到
  `bootstrap.ts` 与全部非测试文件）。
- 「TS 不得出现字面量 `maxPlugins:`」（防第二事实源回归）。

**验证**：示例 app 同时注册两套 handler，`plugin_invoke` 真能调到插件，不再返回 SC-9001。

**量级：大（2 轮）**。

---

### W2. 端到端可运行（解 P0-1）——**最高优先级**

**问题**：唯一的可运行 app 里，插件管理 UI 从未注册；仓库**没有任何「插件管理器可用」
的运行证据**。所有「已完成」宣称都缺少可运行证据。

**设计**：

1. **修 UI 注册**：示例改为 `import '@tauron/ui'`（或 `@tauron/ui-primitives`）**索引入口**，
   使 `wc-shell.ts:554-559` 的 6 个组件真正 `customElements.define`。
2. **补 `@tauron/ui/wc` 的语义**：该入口要么**只注册 toast 并在文档写明**
   （那示例就该用别的入口），要么改为**转出全部组件**。二选一，不允许含糊
   ——这正是 P0-1 的根因（入口名暗示「全部」，实际只有 1 个）。
3. **e2e 冒烟脚本**（可 CI 化）：启动 Tauri 应用 → 断言 `<oc-plugin-manager>` 已 upgrade
   → 触发 install/toggle/uninstall → 断言状态变化。无 GUI 环境时用
   `@tauron/host` 的 `MemoryTransport` + 真实 `ShellClient` 做**链路级**冒烟（见 W9）。
4. **插件管理器从哑组件升级为容器**：接上 `contributes` 列表与 `host_registry_list`
   （与 W3 协同）。

**不变量**：示例 app 的每个 UI 区块都必须有「运行时真的渲染了」的证据，不接受「构建通过」代替。

**门禁**：
- 「`examples/minimal-app` 的构建产物中 `customElements.define` 的组件数 ==
  `ui-primitives` 导出的组件数」（**直接钉死 P0-1 这类缺陷**）。
- 「示例必须存在一条 `oc-plugin-manager` 的 e2e 断言」。

**验证**：`dist` 产物里出现全部 7 个组件；e2e 冒烟脚本绿。

**量级：中（1 轮）**。

---

### W3. 扩展点闭环（解 P0-6 / P1-4）

**设计**：

1. **补 `host_contributes_reconcile`**（self 档）：比对 manifest 声明与 activate 期注册，
   分叉返回 `E_CONTRIBUTES_DRIFT`（**追加到 `ErrorCode` 枚举末尾**，遵守只追加约定）。
   触发点：安装完成、启用、`host_registry_list`（只读比对，不改状态）。
2. **驱动 UI**：`ShellController` 接上 `oc-command-select` → 查 contributes →
   派发到 `host_call_plugin`（经 W1 的投递通路）。`oc-plugin-manager` 从哑组件升级为容器。
3. **修门禁漏洞（P1-8）**：`wire-gate` 的「接线 **或** 注释声明接入方域」改为
   「必须有 `addEventListener` 的真实消费方，**或** 在 `UNWIRED_EVENTS` 常量里显式登记
   且该常量被测试断言长度」（当前「写句注释就放行」等于门禁失效）。
4. **`oc-toast-action` 落地**：`toast` 的 action 回调在示例里接一次（或登记为未接线）。

**不变量**：新增命令必须进 `authz::COMMANDS` 并同步 TS 镜像表（`capabilities.ts`）。

**门禁**：「每个 `dispatchEvent(new CustomEvent('oc-*'))` 的 `oc-*` 名必须出现在
真实监听者或 `UNWIRED_EVENTS` 中」（正反双向断言）。

**验证**：命令面板真能调到插件；drift 可检出（造一个「声明了但没注册」的 manifest）。

**量级：中（1 轮）**。

---

### W4. 平台五域 Provider（解 P0-4 / P1-11）

**设计**：引入 `trait FsProvider` / `HttpProvider` / `UpdaterProvider` / `MenuProvider` /
`TrayProvider`，沿用既有 `ProviderResult<T>` + `native_supported()` 模式
（`lib.rs:354/729/790` 已有该模式，dialog/clipboard/deep-link 就是这么做的）。
缺省 = `native_supported() == false` → 返回 `UnsupportedBody`。优先级 **fs > http > updater > menu > tray**。

同时补 `crates/tauron-adapter/permissions/`（**由 `authz::COMMANDS` 生成，不手写**）
与三档 capability 模板——这解掉「已登记的部署配置缺口」。

**不变量**：新命令一律走 `*Provider` trait，`cmd_*` 内零直接 Tauri 调用（§3-5）。

**门禁**：
- 「五域命令存在性与 `native_supported()` 一致」；
- 「`permissions/` 文件与 `authz::COMMANDS` 逐条对应」（生成器 + 校验，防手写漂移）；
- 「未配置 provider 时命令返回 `UnsupportedBody` 而非裸成功」。

**验证**：fs/http 在示例里真能读写一个文件 / 发一次请求；未配置域返回有类型 Unsupported。

**量级：大（2 轮，按域拆）**。

---

### W5. 插件形态四通（解 P0-5）

| 形态 | 目标 | 动作 |
|---|---|---|
| **Js** | **契约化**（承认「宿主开窗 + 插件自举」为正式模型） | 文档改写 + 隔离声明（webview 边界 + 逐命令身份判定），不再宣称进程内沙箱 |
| **Process** | **真通** | 帧回路已接线（轮 20）；本轮补 ① W7 的三处硬化 ② 一个**真实 sidecar 二进制**（Rust 或 Node 均可）作为集成测试夹具，跑通「真收真回」 |
| **Native（Rust）** | **编译期静态注册** | 宿主构建期 `register_native(id, impl)`；清单校验 `entry`/`abi`；运行期直接调用，无 IPC/进程/webview；未注册 → `E_PLUGIN_TYPE_NO_RUNTIME` |
| **Wasm** | **真通（feature-gated）** | `tauron-wasm` 补 wasm 引擎依赖（`wasmtime` 优先），**默认关闭**；未启用时维持 `E_PLUGIN_TYPE_NO_RUNTIME` |

**不变量**：每个 `PluginType` 变体必须有 {执行器} 或 {诚实失败码 + 文档标注}，
**不允许**「清单合法但调即死」的第三态；wasm 引擎依赖不得进入默认构建。

**门禁**：
- 「遍历 `PluginType` 四变体，每个必须有执行器分支或显式 `NO_RUNTIME` 返回」；
- 「对外文档的形态表必须与枚举变体集合一致」（删掉「B+ 混合模式」这类代码里不存在的宣称）；
- 「`cargo tree --no-default-features` 不含 wasm 引擎」。

**验证**：Native 插件在示例里被真调用；Wasm 在开 feature 后真执行一个函数；默认构建不含引擎。

**量级：大（2 轮）**。

---

### W6. 安装链路默认可达 + 签名强化（解 P0-3 / P0-8）

**设计**：
1. **feature 策略反转**：`plugin-install` 进 `default`（安装是「多插件框架」的动词，
   不该默认缺席）；保留「未配置 `plugin_install_dir` 时明确不可用」的诚实语义。
2. **签名 payload 覆盖元数据**：把 `algorithm` / `kid` / `issued_at` / `packageId` / `version`
   一并纳入签名 payload（规范化序列化，字段顺序固定），并加**过期校验**。
3. **安装失败路径保持「无残留」**（既有负向测试保持）。

**不变量**：签名 payload 的字段集合与顺序必须由**单一函数**生成，验签与签名共用。

**门禁**：「篡改 `issued_at` / `kid` 后验签必须失败」（负向测试）；
「TS 的 install 可用性标注 == Rust 的真实可用性」。

**验证**：默认构建下 `supports('host_registry_install') === true`（配置了目录时）；
篡改元数据后验签红。

**量级：中（1 轮）**。

---

### W7. 运行时硬化（解 P0-7 / P2-1 / P2-2 / P2-3 / P2-4）

**设计**：
1. **spawner 竞态**：`closed` 与 `sinks` 合并为**单锁**结构（`Mutex<ProcRegistry>` 内含
   `HashMap<u32, Sink>` + `HashSet<u32> closed>`），消除 TOCTOU；或至少在同一锁内完成
   「检查 + 插入」。
2. **写帧不再全局阻塞**：`stdin_writers` 改为 `DashMap`/`Mutex<HashMap<u32, Mutex<ChildStdin>>>`
   两级锁，写帧只锁**目标进程**的 stdin；加写超时（`SO_SNDTIMEO` 不适用于管道，
   改为独立写线程 + 有界队列）。
3. **行长上限**：`lines()` 换成手动 `read_until` + 长度检查，超限即断连并记日志。
4. **children 回收**：改为读线程 EOF 时**主动**从 `children` 移除（而非惰性轮询）。
5. `process_delivery` 的 `settle_call` 失败不再静默：非幂等失败记日志 + 计数。
6. `eventbus` 锁序与声明一致（`subscribe` 先 `stats` 后 `approvals`）。

**不变量**：管道/线程/句柄的生命周期必须与子进程生命周期**绑定**；任何「崩溃→重启」
循环下资源占用必须有界。

**门禁**：「`register_frame_sink` 的检查与插入在同一锁内」（源码级断言）；
「读线程必须从 `children` 移除自己」；「`write_frame` 不得持全局锁做阻塞 IO」。

**验证**：崩溃→重启 1000 次的压力测试后 `sinks` / `children` 长度归零。

**量级：中（1 轮）**。

---

### W8. 横切组件全部打通（解 P1-7）——**用户要求「全部打通」**

| crate | 规模 | 打通方式 |
|---|--:|---|
| `tauron-theme` | 630 行 | 接 `host_theme_*` 命令（3–5 条：`set_theme` / `list_themes` / `export` / `import`）+ `@tauron/ui` 的 `oc-theme-picker` 消费（顺带解 P1-4 的 `oc-theme-change` 孤儿） |
| `tauron-brand` | 1368 行 | 接 `host_brand_info`（当前是 `UnsupportedBody` 桩）：白标信息由 `tauron-brand` 提供，含 logo/名称/主题变量；与 W4 的 provider 模式一致 |
| `tauron-wasm` | 1629 行 | 由 W5 的 Wasm 形态打通（补引擎依赖） |
| `tauron-distribute` | 1529 行 | 接线到 `release.yml` 的发布流程（灰度/分发策略），或明确登记为「CI 侧组件」并从能力宣称中移出 |
| `tauron-shell` | 2380 行 | 由 W1 收敛（保留门面、删除 legacy 引擎） |

**另需**：`tauron-settings` 变更通知链（P1-6）接出口——`host_settings_changed` 事件或
`host_events_publish` 保留 topic；`tauron-i18n` 的 `to_json`/`from_json` 接持久化。

**不变量**：任何被宣称「已接线」的 crate 必须有**非测试的生产调用点**（门禁可判）。

**门禁**：「`crates/*/Cargo.toml` 中每个 crate 至少被一个非自身 crate 依赖，
或在 `canonical-owners.md` 的机器可读表里登记为 `standalone`/`ci-only`」。

**验证**：`grep tauron_theme:: crates/tauron-adapter` 非空；`grep tauron_brand::` 非空。

**量级：大（2 轮）**。

---

### W9. 内存传输与测试基座（解 P1-3 / P1-9 / P2-5）

**设计**：
1. **MemoryTransport 补真实内核**：与 `MockBackend` 的流式内核（`backend.ts:212-272`）
   **统一**——一套内存实现，覆盖全部 host 命令族（含 `host_stream_*` / `host_call_*` /
   `host_events_drain`）+ 取件泵。使「只取底座」这一档（incremental-adoption 的档 1）
   真正可跑。
2. **删同义反复测试**，改为行为断言（至少断言「调用后状态变化」）。
3. **`@tauron/host` 死导出收口**：要么被示例/W2 的 e2e 消费，要么移入
   `@tauron/host/testing`，要么删除——不留第三态。

**不变量**：`MemoryTransport` 与真实 `TauriBackend` 必须**共享同一份命令面契约**
（由 wire-gate 对两侧断言），防止内存实现悄悄少实现一个命令族。

**门禁**：「`MemoryTransport` 必须实现 `FRAMEWORK_COMMANDS` 全集，或对每条未实现命令
显式登记 `unimplemented`」（解析到 0 条即失败，防假绿）。

**验证**：用 `MemoryTransport` 跑通档 1 的全部能力域（0.3 的 S1 验收条件，本轮补齐）。

**量级：中（1 轮）**。

---

### W10. 门禁与文档由代码推导（解 P1-11 / P2-6）

1. **门禁解析健壮性**：wire-gate 的 `parse()` 类辅助一律「容忍换行与注释」+
   「解析到空即失败」断言。
2. **文档派生**：对外文档的**形态表**与**能力清单**由代码符号推导（`PluginType` 枚举、
   `FRAMEWORK_COMMANDS`、`authz::COMMANDS`）或显式登记 + 测试断言，不再手写。
3. **孤儿 public API 收口**：§5-P2-6 列出的孤儿 API 逐个处置（接线 / 移入 testing /
   删除 / 登记为公共 SDK 面）。
4. **更新 `docs/architecture/README.md`**：修正「唯一的前瞻计划」（0.4 已存在）、
   「4+1 插件形态」（代码里不存在）、「双世界隔离（QuickJS-WASM）」（fail-closed 模拟）。

**量级：中（1 轮）**。

---

## 7. 轮次编排（轮 30 起）

依赖顺序：**先让示例真跑（W2）**→ 再收敛结构（W1）→ 再补能力（W3/W4/W5）→
最后硬化与收尾（W6–W10）。理由：W2 是「一切宣称的前提」，W1 是其余各条的公共地基。

| 轮 | 内容 | 完成判据 |
|---|---|---|
| **轮 30** | **W2 端到端可运行**（修 P0-1） | 示例 dist 注册全部 7 个组件；插件管理器可交互；e2e 冒烟绿 |
| **轮 31** | **W7 运行时硬化**（P0-7 三处 + P2 四项） | 崩溃→重启压力测试后资源归零；三处门禁生效 |
| **轮 32** | **W6 安装默认可达 + 签名强化** | 默认构建可装；篡改元数据验签红 |
| **轮 33** | **W1-a 单线收敛：Rust 侧**（shell 委托 host，删 legacy 引擎） | `plugin_invoke` 不再恒 SC-9001；shell 无 legacy 定义 |
| **轮 34** | **W1-b 单线收敛：TS 侧**（解 P1-1/P1-2/P1-5/P1-10） | host 不 import core 运行时类；单一 IPC 后端；注册表上限同源 |
| **轮 35** | **W3 扩展点闭环** | drift 可检出；命令面板真调到插件；孤儿事件门禁生效 |
| **轮 36** | **W4-a 平台域 fs + http** | 两域可用或明确 Unsupported；permissions 自动生成 |
| **轮 37** | **W4-b 平台域 updater + menu + tray** | 三域有 provider 与诚实缺省 |
| **轮 38** | **W5-a Native 静态注册** | `PluginType::Rust` 有执行器 |
| **轮 39** | **W5-b Wasm 引擎接入（feature-gated）** | Wasm 插件真执行；默认构建不含引擎 |
| **轮 40** | **W8 横切全部打通**（theme / brand / settings 通知 / i18n 持久化） | 各 crate 有非测试生产调用点 |
| **轮 41** | **W9 内存传输统一 + W10 门禁与文档派生 + 全量诚实性对账** | 档 1 可跑；文档数字由代码推导；§5 清单全部有处置结论 |

> **每轮门槛**（沿用 R1–R8 / 0.4）：
> `cargo check --workspace --all-targets` 0 警告；
> `cargo test --workspace --locked --lib --tests` 全绿 **且**
> `--features tauron-adapter/plugin-install` 全绿；
> `pnpm -r test` **0 红**；wire-gate 全绿。**每轮结束都是可发布态**。

---

## 8. 验收标准（1.0 发布条件）

1. **可运行证据**：示例 app 的插件管理器可交互，且有 e2e 断言（不是「构建通过」）。
2. **单线架构**：`tauron-shell` 无 legacy 引擎定义；`@tauron/host` 不 import `@tauron/core`
   运行时类；只有一套 IPC 后端与一套命令名表。
3. **四形态名实相符**：`PluginType` 四变体各有执行器或显式 `NO_RUNTIME` + 文档标注。
4. **平台域完整**：fs / http / updater / menu / tray 五域各有 provider；
   未配置时返回有类型 `Unsupported`；`permissions/` 由 `authz` 生成。
5. **扩展点闭环**：contributes 可对账（drift 可检出），命令面板由 contributes 驱动，
   孤儿事件门禁生效（不再「写句注释就放行」）。
6. **安装可用**：默认构建可安装；签名覆盖元数据且有过期校验。
7. **运行时无泄漏**：崩溃→重启循环下 `sinks`/`children` 有界；写帧不阻塞全局。
8. **横切全接线**：`crates/*` 每个 crate 要么有非测试生产调用点，要么登记为 `standalone`/`ci-only`。
9. **门禁与文档同源**：形态表与能力清单由代码推导；解析类辅助有「空即失败」保护。
10. **诚实披露**：§5 每条都有处置结论；无「有类型、有接口、无入口」的第三态被描述为可用。

---

## 9. 风险与不变量

### 9.1 硬不变量（不得回退）

§3 的 10 条，全部继续有效。新增：

11. **能力表只能由运行时真相推导**，不得硬编码（解 P1-5 的机制化）。
12. **任何被宣称「已接线」的 crate / 包必须有非测试的生产调用点**。
13. **示例 app 是唯一可运行证据的载体**——它的每个 UI 区块都必须有「运行时真渲染了」的断言。

### 9.2 风险

| 风险 | 缓解 |
|---|---|
| W1 删除 legacy 引擎触发隐藏耦合（P1-1） | 先解 P1-1 再删；门禁钉死「host 不 import core 运行时类」 |
| W5-b 引入 wasm 引擎破坏「零依赖」承诺 | 严格 feature-gated + `cargo tree --no-default-features` 门禁 |
| W7 加写线程引入新的竞态 | 有界队列 + 超时；压力测试覆盖 |
| W2 的 e2e 在无 GUI 环境不可跑 | 双轨：GUI 冒烟（本地/CI 有显示时）+ `MemoryTransport` 链路级冒烟（W9） |
| 轮次长，中途需发布 | 每轮结束都是可发布态（门槛绿 + 文档同步） |
| 本方案自己制造新的「有接口无入口」 | 每条 W 项都配**入口可达性门禁**；轮末做负向突变验证 |

### 9.3 明确不做

- **不合并两套错误码**（`SC-xxxx` / `E_*`）。分层是有意设计。
- **不做进程内 JS 沙箱运行时**。Js 型隔离来自 webview 边界 + 身份判定。
- **不做进程内内存/CPU 配额**。改为记录与暴露指标并在文档写明。
- **不改写 `plugin_invoke` 三命令的对外协议语义**（W1 删的是实现，不是协议）。

---

## 10. 关联文档（本方案落地后需同步）

| 文档 | 需同步内容 |
|---|---|
| [overview.md](./overview.md) | 三层结构 → 单线结构；接线状态表按 W8 更新；命令面数字 |
| [app-layer-wire.md](./app-layer-wire.md) | 五域 provider 线格式、contributes reconcile、install 可用性语义 |
| [canonical-owners.md](./canonical-owners.md) | crate 归置表按 W1/W5/W8 更新；新增 `standalone`/`ci-only` 登记 |
| [capability-closure-plan.md](./capability-closure-plan.md) | 顶部加横幅：A4–A8 与轮 23–29 由本文接管（**不得再宣称 0.4 已完成**） |
| [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md) | §2 残差表按本轮实测重写（acl/market 已非孤儿） |
| [../api/plugin-development-guide.md](../api/plugin-development-guide.md) | 形态表按 W5 重写；新增五域命令；示例换入口 |
| [../architecture/README.md](./README.md) | 修正「唯一的前瞻计划」「4+1 插件形态」「双世界隔离」三处漂移 |
| `README.md` | 形态宣称、能力清单按 §5 处置结论更新 |

---

## 11. 本轮实测基线（供后续轮次对照）

| 指标 | 实测值（**2026-09-27 发布收口复核**） | 命令 |
|---|---|:--|
| Rust 库测试（默认特性，含 `plugin-install`） | host **290** / adapter **224** / proc **46** / settings **99** / schema **131** / market **74** / types **67**（15 suite 合计 **1264**） | `cargo test --workspace --locked --lib --tests` |
| wire-gate | **126 / 126**（本包合计 **147** 条 / 2 文件） | `pnpm --filter @tauron/contract-tests test` |
| 命令面 | 底座 **57** / 插件 **78**（= 57 + 运行时 21）；`plugin-install` 2 条**已进默认特性**，默认装配共 **80** | wire-gate 解析器 |
| 档位表 | 插件面 **18**（Self_ 16 + ScopedRead 2）+ 特权 **4** | `crates/tauron-host/src/authz.rs` |
| 孤儿 crate | brand / theme / wasm / distribute / shell（**7536 行**） | 依赖图 |
| 示例可运行组件 | **10**（W2 已修：`ui-primitives/wc` 补 4 条副作用导入，含 `oc-plugin-manager`） | `grep customElements.define dist/assets/*.js` |

> **环境限制（非项目缺陷）**：本机 `cargo test --doc` 报
> `Failed to spawn rustc: Os error 231`（管道耗尽）+ 安全中心拦 `target/` 目录，
> 故 doctest 以 CI 为准；`rimraf dist` 被沙箱批量删除保护挡住，
> TS 构建改为 `npx tsc -p tsconfig.build.json` 直跑。

---

## 12. 诚实性更正记录

- **`08156f9` 的提交信息不准确**：该提交写「A1–A8 全量落地 / A4–A8 按方案落地」，
  但本轮实测确认 **只有 A1–A3（轮 20–22）落地**，A4–A8 与轮 23–29 未开工。
  按用户决策「按最新版本计划全部重构实现，不需要保持历史兼容」，
  **不改写已公开的提交历史**，以本文档为准确口径；
  后续实现以本文 §6/§7 为准。
- **`capability-closure-plan.md` §8 的轮次表**是准确的（轮 23–29 无 ✅），
  该文档本身没有夸大；夸大发生在提交信息里。
- **`docs/architecture/README.md`** 的三处漂移（唯一前瞻计划 / 4+1 形态 / 双世界隔离）
  属**文档层面**的夸大，由 W10 收口。
