# canonical 归属表（R2-a 决策记录）

> 本文件是**机器可读的决策记录**：`wire-gate` 会读它并断言代码与表一致
> （见 `packages/tauron-contract-tests/src/wire-gate.test.ts` 的「R2-a」门禁）。
> 改这张表 = 改架构决策，必须同时改代码并说明理由。
>
> ⚠️ **本文件在轮 12 曾被一次错误的批量替换毁掉全文反引号**（PowerShell 数组展平
> 把 `` ` `` 替换成了 `t`，且该文件当时**未入 git**，无备份可回滚）。已按残文逐行
> 重建并复核了其中的实测事实；记录此事是因为"文件看起来没问题"本身就是一种
> 需要证据的断言。**教训**：文档也应当入版本控制，机器改文只走 `edit`/`write`。

## 为什么需要这张表（方案 §3 的 B1）

重构前仓库里有**两套**同名核心类型，且从未写明哪套是准的：

| 概念 | 框架层（`tauron-shell`） | 应用层（`tauron-host`） |
| --- | --- | --- |
| 事件总线 | `EventBus`（300 行，9 个公开方法） | `EventBus`（1324 行，19 个公开方法） |
| 注册表 | `PluginRegistry`（298 行，10 个公开方法） | `Registry`（1826 行，32 个公开方法） |
| 宿主态 | `HostState`（533 行 `dispatch.rs` 内） | `SubstrateState` / `PluginRuntimeState`（R1b 拆分） |

方案 R2 原文的决策是「框架层（tauron-shell）作为 canonical」。**实测推翻了该决策的
依据**，因此本表记录的是反向决策（依据见下）。这不是忽略方案，而是执行方案自己
要求的「先量化、再决定」——方案 R2 的门禁原文也允许反向：「`tauron-shell` 不得定义
`PluginRegistry`/`EventBus`（**必须引用 host 的，或反之按 canonical 决策**）」。

## 决策：canonical = `tauron-host`

| 概念 | canonical 归属 | 非 canonical 侧状态 |
| --- | --- | --- |
| 事件总线 | `crates/tauron-host/src/eventbus.rs` 的 `EventBus` | `crates/tauron-shell/src/eventbus.rs`：**legacy 冻结**（不得再加功能） |
| 注册表 | `crates/tauron-host/src/registry.rs` 的 `Registry` | `crates/tauron-shell/src/registry.rs` 的 `PluginRegistry`：**legacy 冻结** |
| 宿主态 | `crates/tauron-host` 的 `SubstrateState`（底座）+ `PluginRuntimeState`（插件运行时） | `crates/tauron-shell/src/dispatch.rs` 的 `HostState`：**legacy 冻结** |

## 依据（全部为实测，可复核）

| 维度 | `tauron-host` | `tauron-shell` |
| --- | --- | --- |
| 消费者 | `tauron-acl`、`tauron-adapter`（唯一被依赖的两条） | **零个 crate 依赖它**（`grep tauron-shell crates/*/Cargo.toml` 只有它自己） |
| 能力面 | 19 + 32 个公开方法：主题审批、可靠 drain 队列、订阅组、在途调用、TTL GC、**流式句柄表**、身份绑定、safemode 过滤 | 9 + 10 个公开方法：emit/subscribe 与 register/transition |
| 验证密度（轮 12 末实测） | `tauron-host` **269** 测试 + `tauron-adapter` **193**（lib，无 tauri）/ **222 + 1**（`--features tauri`）+ `tauron-shell` **71** | 71 测试（`--features tauri`，含 ic 命令层） |
| 与 R5/R7 的关系 | R5 的流式句柄表、R7 的持久化对账都在此 | 无流式、无 drain、无审批模型 |
| 生态面（方案给 shell 的理由） | 应用壳 + `@tauron/host` + 契约门禁 | `plugin_invoke` 协议被 `@tauron/core` + 3 个框架 adapter 使用 |

**结论**：把 3000+ 行已硬化、已被依赖的实现删掉、改成依赖 1100 行零消费者的实现，
会同时降低能力与验证密度——这不是"去重"，是回退。因此 canonical 定为
`tauron-host`；`tauron-shell` 保留其**协议门面**角色，但其并行引擎冻结为 legacy。

### 轮 10 复核（P0-2 之后）

依赖方向重新实测（`crates/*/Cargo.toml` 内 `tauron-*` 依赖行）：

| crate | 依赖的 tauron crate |
| --- | --- |
| `tauron-acl` | `tauron-host` |
| `tauron-adapter` | `tauron-host`、`tauron-i18n`、`tauron-notify`、`tauron-recovery`、`tauron-proc` |
| `tauron-settings` | `tauron-schema` |

`tauron-shell` **仍然零消费者**（决策依据未变）；P0-2 让 `tauron-proc` 获得第一个
消费者（`tauron-adapter`）——顺带证明本表的用法：**归属看依赖，不看文件大小**。

### 轮 11 复核（R7 持久化对称之后）

依赖方向重新实测（`crates/*/Cargo.toml` 内 `tauron-*` 依赖行）：

| crate | 依赖的 tauron crate |
| --- | --- |
| `tauron-acl` | `tauron-host` |
| `tauron-adapter` | `tauron-host`、`tauron-i18n`、`tauron-notify`、`tauron-recovery`、`tauron-proc`、**`tauron-settings`** |
| `tauron-settings` | `tauron-schema` |

R7 让 **`tauron-settings` 获得第一个消费者**（`tauron-adapter`）——在此之前它是"有实现、
有测试、无消费者"的孤儿 crate，`host_settings_*` 命令直接操作裸 `HashMap`。这条再次印证
归属判据：`tauron-settings` 是设置语义的 canonical 所有者，而"谁被谁依赖"才是归属证据；
顺带把 `tauron-schema` 也拉进了传递闭包（设置 schema 校验）。`tauron-shell` **依旧零消费者**，
本表决策未变。

### 轮 12 复核（R8 + 身份收口之后）

依赖方向**未变**（轮 12 只加测试、示例 feature、文档与 CLI 诚实性修正，未动 crate 依赖）：

| crate | 依赖的 tauron crate |
| --- | --- |
| `tauron-acl` | `tauron-host` |
| `tauron-adapter` | `tauron-host`、`tauron-i18n`、`tauron-notify`、`tauron-recovery`、`tauron-settings`、`tauron-proc` |
| `tauron-settings` | `tauron-schema` |

`tauron-shell` **依旧零消费者**。轮 12 新增的客户端侧契约（`--features substrate-only`
示例形态、capability 文件）都在**消费**侧，不改变归属：`SubstrateState` 就是底座析取的
产物，`tauron_substrate_handler![]` 是它的命令面，两者都在 `tauron-adapter` 里。

## 本轮的**真实**改动与**未做**（诚实披露）

已做（可验证）：

1. 决策成文 + 机器可读归属表（本文件）；
2. `tauron-shell` 的三个 legacy 类型加上「非 canonical / 冻结」文档标记，指向本表；
3. 门禁冻结：legacy 侧方法集**指纹**（9 / 10 / `HostState`）一旦增长即失败；
   canonical 侧方法集只允许增长（≥ 记录值）；两个 crate **不得**互相依赖
   （避免"半合并"状态让归属再次含糊）。

**未做**：把 shell 的 `EventBus`/`PluginRegistry`/`HostState` 从代码中**删除**、
或反向让 host 引用 shell —— 两者都需要**协议迁移**（`plugin_invoke` 信封 +
`@tauron/core` 与 3 个框架 adapter 的调用点），属独立的迁移轮次，而不是本轮的
"删 1100 行"。方案给 R2 排了一轮，但实测的改动面（两套引擎的语义模型不同：
审批式主题 + drain 队列 vs emit 直投）不支持一轮内安全完成。迁移路线：

- **阶段 1（本轮）**：定 canonical + 冻结 legacy + 门禁（已完成）。
- **阶段 2**：`tauron-shell` 的 `plugin_invoke` 命令层改为**委托** `tauron-host`
  的引擎（`PluginDispatcher` 只做信封 ↔ 宿主调用的翻译）；其 71 个测试随之改写为
  委托后的行为断言。
- **阶段 3**：删除 shell 的 `EventBus`/`PluginRegistry`/`HostState` 实现与其
  11 + 11 + 15 个旧测试；`@tauron/core` 的协议调用点合并到 `@tauron/host` 的
  HostRpc（R5 已就位）。

阶段 2/3 未在本轮假装完成。
