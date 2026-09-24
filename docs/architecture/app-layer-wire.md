# 应用层线格式协议（`@tauron/host` ↔ `tauron-adapter`）

> 本文是应用层 `host_*` 命令族 TS↔Rust 线格式的**规范**。该契约由两道门禁锁定：
> Rust `tauron-adapter::tauri::wire_tests`（TS 调用点 JSON 字面量逐字反序列化）
> 与 `@tauron/contract-tests` 形状门禁（解析两侧源码，逐调用点比对参数形状）。
> 历史教训：只比对命令名的门禁发现不了"名对得上、形状对不上"的断链——
> 本文档与门禁共同防止其复发。

## 1. 注册形态（互斥）

| 形态 | 用法 | 命令面 | 插件名 |
| --- | --- | --- | --- |
| root 注册（全量） | `state_init()` + `tauri::generate_context` 外的 `tauron_generate_handler![]`（= `tauron_plugin_handler![]`） | 54 条 | 无（裸命令） |
| **底座-only root 注册** | 自己 `manage(SubstrateState)` + `tauron_substrate_handler![]` | 38 条（不含插件运行时 16 条） | 无（裸命令） |
| 插件注册（需 capability/ACL） | `init()` / `init_with_adapter_config(cfg)` | 54 条 | `tauron` |

> ⚠️ **三种形态都不是"零配置"**（轮 12 改判，此前本表把 root 形态写成"零配置"是错的）：
> Tauri v2 的规则是**不匹配任何 capability 的 webview 完全没有 IPC 访问**（原文见
> `tauri-build` 生成的 `gen/schemas/desktop-schema.json` 的 `Capability` 说明）。
> 所以任何宿主都必须至少有一份 capability 文件，且 `windows` 要覆盖它实际用到的窗口 label。
> 示例已随仓提供 `examples/minimal-app/src-tauri/capabilities/default.json`
> （`core:default` + `windows: ["main", "plugin-*"]`；`plugin-*` 对应 `host_window_create`
> 铸出的 `plugin-<插件 id>` 面板窗，漏掉它插件界面一片空白）。

两种形态**互斥**（重复 `manage::<CommandState>` 会 panic）；壳层 `tauron-shell`
/ `tauron-shell-state` 与应用层插件名不同，可叠加注册。

> ⚠️ **插件注册形态的部署期缺口（照实登记，未修）**：`crates/tauron-adapter` 目前
> **没有 `permissions/` 目录**（也没有权限条目定义）。Tauri v2 在**启用能力检查**的宿主里
> 强制 ACL，因此走 `plugin:tauron|host_*` 路由的调用在这种宿主上会因**缺权限条目**被拒
> ——这与命令实现无关，是**部署配置**缺口。本仓示例应用走的是 root 注册（裸命令名，
> 不依赖插件 ACL），所以开发态看不出来。同一条也适用于 R8 新增的
> `host_window_relaunch` / `host_window_create` 与第三方插件的装配器宏路由
> （`plugin:<name>|<cmd>`，示例里的 `plugin:my-plugin|my_stats` 同理）。
>
> **轮 12 结论（为何仍然"未修"，而不是忘了）**：修它需要在 crate 内加 `build.rs` +
> `tauri-build` 依赖 + `permissions/*.toml`，而**权限条目的运行期效果在本仓无法验证**
> （只有真实启用能力检查的宿主构建才能证）。按"不写不可验证的配置"原则，宁可在文档里
> 如实登记缺口，也不塞一份看着像真的权限文件——那会制造"配了就是通的"错觉。
> 需要走插件路由的宿主请自行定义权限条目，并注意 root 形态与插件形态**互斥**。

**状态拆分（R1b）**：`CommandState` 已**不再是** 12 字段的 god object，而是
`PluginRuntimeState` 的别名；`PluginRuntimeState` 通过 `Deref` 暴露
`SubstrateState`（底座）字段。宿主要装什么就注册什么：

```rust
// 多插件宿主（全量命令）
tauri::Builder::default()
    .plugin(tauron_adapter::tauri::init())        // 或 state_init() + tauron_generate_handler![]
// 底座-only 宿主（不跑插件运行时：一分插件状态都不建）
tauri::Builder::default()
    .setup(|app| {
        app.manage(SubstrateState::with_adapter_config(&AdapterConfig::default()));
        Ok(())
    })
    .invoke_handler(tauron_adapter::tauron_substrate_handler![])
```

命令的 `State` bound 与命令族**一一对应**（wire-gate 锁死）：底座命令绑
`State<SubstrateState>`（编译期触达不到 registry），插件命令绑
`State<PluginRuntimeState>`。

**命令族是编译期可选集合（R1a）**：不跑插件运行时的宿主（harness 壳、单窗口工具
壳）用 `tauron_substrate_handler![]`——**未注册即不可达**，前端调用 `host_registry_*`
/ `host_plugin_call` / `host_contributes_*` 等会得到 `command not found`。两组集合
都由 wire-gate 锁死：全量集合 == 全部 `#[tauri::command]` 定义；底座集合 ⊂ 全量且
**不含任何插件域命令**；两组都经同一个 origin 门。

> `host_recover_trial_enable` 归**插件域**：它要读注册表里插件的当前状态并补发
> `TrialEnable` 事件（R1b 收窄签名时发现，R1a 首版曾误归底座）。

> 为什么是「两组集合」而不是「N 个可嵌套的族宏」：`tauri::generate_handler!` 是
> proc macro，输入按**字面 path 列表**解析——实测传入 `family!()` 会得到
> `error: expected ','`。族的展开结果无法拼进同一个 handler，因此族的边界只能落在
> **整份注册列表**这一级（编译期二选一），而不是运行时过滤。

### TS 侧传输面（R5）

```ts
export interface HostRpc {
  request<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  stream(callId: string, onFrame: (frame: StreamFrame) => void): Unlisten;
  event(topic: string, payload: unknown): Promise<void>;
  subscribe(topic: string, handler: (payload: unknown) => void): Promise<Unlisten>;
}
```

- `toHostRpc(client)` 从 `HostClient` 适配；换传输（移动 gateway、测试 mock）
  只需实现 `Backend`（更低层）或 `HostRpc`（更高层）两者之一。
- **`channel` 的线值**：前端传的是**真实 Tauri `Channel` 对象**（`Channel.toJSON()`
  → `__CHANNEL__:<id>`），Rust 侧用 `JavaScriptChannelId` 解析。类型层面
  `ChannelPort` 只声明真实 `Channel` 拥有的成员（`onmessage` / `id`），因此
  `tauri-backend` 不再需要 `as unknown as` 掩盖差异。
  `Option<tauri::ipc::Channel<T>>` 作为命令参数**编译不过**（Tauri 只为裸
  `Channel<T>` 实现 `CommandArg`），故 Rust 侧声明 `Option<String>` 自行构造。
- **SDK 边界**：`PluginContext` 只消费 `PluginRpc` 契约（事件域 4 原语 +
  `contributesRegister`），由编译期断言 `HostClientSatisfiesPluginRpc` 与
  wire-gate 双向锁定；SDK 源码不出现 `invoke` / `@tauri-apps` / `TauriBackend`。
  `ctx.host` 仍暴露完整 `HostClient`（插件作者需要 `pluginCall` 等面）——
  把它收窄为纯 `HostRpc` 会砍掉插件能力，属 R6 的独立设计项。

**装配（R4-D2）**：`init_with_adapter_config(AdapterConfig { .. })` /
`state_init_with_adapter_config(..)` 是唯一能把宿主配置交给适配器的入口——含
`origin_allowlist`（origin 允许清单，空=不启用）、`registry`、`required_plugins`、
`recovery_data_dir`。缺省入口 `init()` / `state_init()` 等价于传
`AdapterConfig::default()`；历史入口 `init_with_config(RegistryConfig)` 保留，
且已改为委托到同一装配点（此前它自建 `CommandState::with_config`，**绕过了恢复
持久化装配**，崩溃检测在该入口静默失活）。

## 2. 参数规则

- TS 侧 camelCase 键由 Tauri 映射到 Rust snake_case 形参；**缺失必填（非 `Option`）
  参数在运行时以 missing required key 失败**；多余键被忽略。
- 结构体参数（`req`/`evt`/`op`/`sub`）一律 `#[serde(rename_all = "camelCase")]`，
  字段与 TS 调用点逐字对应（见 `tauri.rs` 的 `Host*` 结构体）。
- 身份（插件 id）一律由宿主从 webview label 解析（`plugin-<id>` 前缀），
  **不信任前端入参**（§2.1 / ADR-17）。

## 3. 关键命令线格式

| 命令 | TS 调用（`HostClient`） | Rust 形参 | 返回 |
| --- | --- | --- | --- |
| `host_plugin_call` | `{ req: { callId, method, kind, argsJson? }, channel? }` | `req: HostPluginCallReq` + 顶层 `channel: Option<String>` | `PendingCall`（**宿主铸造权威 callId**） |
| `host_call_end` | `{ req: { callId, ok?, errorCode?, seq? } }` | `req: HostCallEndReq` | `PendingCall` |
| `host_cancel` | `{ callId }` | `call_id` | `()` |
| `host_lifecycle_report` | `{ evt: { event, reason? } }` | `evt: HostLifecycleEvt` | `TransitionOutcome` |
| `host_events_publish` | `{ evt: { topic, payload } }` | `evt: HostEventPublish` | `PublishResult` |
| `host_events_subscribe` | `{ sub: [{ topic }, …] }` | `sub: Vec<HostEventSelector>` | `HostSubscription { token, selectors }` |
| `host_events_unsubscribe` | `{ token }` | `token`（兼容分组 token） | `()` |
| `host_events_drain` | `{ kind }`（`event`/`request`/`state`） | `kind` | `EventFrame[]`（`{topic,seq,payload}`） |
| `host_registry_list` | `{ scope }`（`visible` 默认） | `scope: Option<String>` | `PluginSummary[]` |
| `host_registry_admin` | `{ op: { op, id } }`（D15 四值） | `op: HostAdminOp` | `TransitionOutcome` |

语义要点：

- **调用 id**：`req.callId` 仅作调用方关联；权威 `callId` 由宿主铸造
  （`PendingCall.callId`），取消与终帧确认必须使用返回值里的 id。
- **返回形状**（Rust → TS 消费，与入参同受门禁锁定）：
  - `host_events_drain` 返回**事件总线帧** `EventFrame`（`{topic,seq,payload}`），
    不是流式帧 `StreamFrame`（后者经 Channel 承载，R5 起为**唯一**帧词表：
    `{seq, kind: data|end|error, argsJson?, argsRaw?}`，旧的 `CallFrame`
    `response/progress/cancel` 词表已删除——两套词表两边都「有类型」却对不上）；
  - `PluginSummary` 线形 `{id,name,version,state,pluginType,publishedTopics,
    disabledBySafemode}`（camelCase；safemode 角标供 `<oc-plugin-manager>`）；
  - `ContributeEntry` 线形 `{pluginId,kind,id,label}`（camelCase，双向）；
  - `host_recover_boot` 手拼 JSON 的键必须 camelCase
    （`phaseName`/`counter.consecutiveFailures`…，对齐 TS `RecoveryBootResult`）；
  - 更新源/品牌桩必须返回可消费形状（`{available:false}` / `{}`），
    返回 Null 会让前端对 null 取属性而崩溃。
- **生命周期上报的是事件不是状态**：`event` 取 `LIFECYCLE_EVENTS`
  （Rust `Event` 的 SCREAMING_SNAKE_CASE 线名，如 `ATTACH`）；状态由宿主单一写入
  （§4.3），上报状态线名（如 `RUNNING`）会被反序列化确定性拒绝。
- **订阅分组**：单选择器直接透传核心 token；多选择器整体记入分组
  （token 形如 `grp:…`），`host_events_unsubscribe` 对分组 token 整体退订，
  不留悬挂订阅（§8-3）。
- **注册表可见性**：`scope` 只是语义提示；可见集合由宿主按调用方身份与事件总线
  **真实订阅**推导（`EventBus::subscribed_topics_of`），scoped-read 无法自报订阅
  扩大可见范围。`host_registry_list_all` 才是全量列表（仅主窗）。
- **管理操作**：D15 定稿四值 `disable` / `enable` / `uninstall` / `purge`
  （kebab-case）；不存在 `trial_enable` 操作名——试验性放行走独立命令
  `host_recover_trial_enable`（受恢复引擎的试验预算约束，1 次试验失败即回落）。
  用户 `enable` 会一并清掉 `disabledBySafemode`（显式启用是权威放行）；若恢复
  引擎仍判定该插件应禁用，阶段对账会在下一拍重新 `SafemodeEnter`，安全模式
  权威不因此旁路。
- **对话框 / 更新源 / 剪贴板当前的可观测语义**（原生后端未接入，属**契约**）：
  - `host_dialog_open` / `host_dialog_save` **恒返回 `null`**（等同"用户取消"）；
  - `host_dialog_message` 不弹窗、直接 `Ok`；
  - `host_dialog_confirm` **恒返回 `false`**（fail-closed：绝不为真，故不得当作
    真实用户确认）；
  - `host_clipboard_read` / `host_clipboard_write` 是**进程内字符串**，
    不碰系统剪贴板（`read` 只能读回本 App 写过的东西）；
  - `filters`/`defaultPath`/`confirmLabel`/`cancelLabel`/`endpoints`/`pubkey`
    属线格式一部分（Rust 侧已接受），在原生后端落地前无效果。
  接入方式：宿主加 `tauri-plugin-dialog` / `tauri-plugin-clipboard-manager` 后在
  适配器命令里转调；窗口命令（`host_window_*`）已是真实原生调用，可作参照。
  TS 侧 JSDoc（`packages/tauron-host/src/dialog-client.ts` 文件头）与本节同源。
- **`kind` 是闭集**：只接受 `unary` / `stream`，表外值返回 `E_AUTH_DENIED`
  （宿主不据 `kind` 改变行为，但必须校验——否则 `streaming` 这类拼写错误
  会无声通过与 `unary` 无异）。缺省时视为未指定，仍被接受。
- **`argsRaw` 是帧的字段，不是调用参数的字段**：调用参数按 §4.8 R6 不得夹带
  二进制（宿主线格式**没有**对应字段），前端单独传 `argsRaw` 抛 `TypeError`
  （**静默丢弃会以 `null` 参数执行**）；与 `argsJson` 同时给出时以 `argsJson`
  为准。**二进制载荷的出口是流式帧**（下一条，R5 已接线）。
- **流式帧已真实派发（R5 / P0-1 落地，废止旧「收下但不派发」说明）**：
  `host_plugin_call` 带 `channel` 时,该通道被**登记为该调用的帧载体**；
  帧由 C/D 后端经三命令写入 → 经通道送达前端 `onFrame`：

  | 命令 | TS 调用（`HostClient`） | Rust 形参 | 返回 |
  | --- | --- | --- | --- |
  | `host_stream_open` | `{ req: { callId }, channel? }` | `req: HostStreamOpenReq` + `channel: Option<String>` | `{ streamId, callId }` |
  | `host_stream_write` | `{ req: { streamId, argsJson?, argsRaw? } }` | `req: HostStreamWriteReq` | `StreamFrame` |
  | `host_stream_close` | `{ req: { streamId, kind } }`（`end`/`error`） | `req: HostStreamCloseReq` | `StreamFrame`（终帧） |

  语义与不变量：
  - **`seq` 由宿主铸**（从 1 起、跨 `kind` 连续、终帧也占号）：前端自报序号就能
    伪造乱序/重复，接收方的去重假设随即失效；
  - **终帧后句柄失效**（再写/再关都是 `E_CALL_NOT_FOUND`）；
  - **跨插件写被拒**（`E_AUTH_DENIED`）：句柄是 UUID，但不靠「猜不到」兜底；
  - **句柄表与 pending-call 表同锁域**（`Registry` 内，锁序 `pending → streams`），
    因此 `call_end` / `call_cancel` / 窗口关闭 / TTL GC 都会**补发终帧**——
    handler 忘了关流时接收方也不会永远等下去（`end` / `canceled` / `call_timeout`
    / `subscriber_closed` 四种终帧原因可区分）；
  - 三命令**成组注册**（属插件运行时域，底座-only 宿主不注册）：只挂一两条会让
    某条流永远开不了或永远终结不了。
- **`host_i18n_t` / 恢复引擎已真实接线**（原 §3 桩条目作废）：
  - i18n：`host_i18n_t` / `host_i18n_t_params` / `host_i18n_set_locale` /
    `host_i18n_load` / `host_i18n_stats` / `host_i18n_cleanup_plugin` 全部落到
    `tauron-i18n` 引擎。**回退链全部落空时返回 key 本身**（不是空串）并计入
    `missingTotal`——前端可用 `text === key` 检出缺失文案；回退命中**不**计缺失。
    bundle 来源仍由应用侧通过 `host_i18n_load` 提供（宿主不内置文案）。
  - 启动恢复（§4.14）：`host_recover_boot` 只读；驱动信号是
    `host_recover_report(outcome, pluginId?)`——**应用每轮启动成功必须上报一次**，
    否则崩溃检测会把未上报视为一次崩溃，连续两次进安全模式（方向安全：一次
    `success` 即自愈）。持久化在 Tauri 宿主入口（`init`/`state_init`）自动打开
    （`app_config_dir`），`persistence.enabled=false` 表示本轮计数只在进程内有效。
    阶段判定落到插件侧由 `reconcile_recovery_phase` 补发
    `SafemodeEnter`/`SafemodeExit`，前端读 `PluginSummary.disabledBySafemode`。
    安全模式内用 `host_recover_trial_enable` 逐个试启（1 次试验失败即回落禁用）。
    轮 11（R7）起 `host_recover_boot` 等返回值新增 **`lastContext`**：最近 N 条
    崩溃诊断（N = 8，`RecoveryEngine::CONTEXT_CAPACITY`），每条含
    `{ ts, phase, failureKind, pluginId, trial, consecutiveFailures, safemodeFailures, pluginState }`。
    它回答的是"上次到底发生了什么"——旧实现只有标记（知道崩过），没有上下文
    （不知道为什么）。关键语义：`failureKind` 由 `(phase, trial)` **派生**、不参与判定；
    上下文**不消耗**任何插件的试验失败预算（否则"安全模式里崩过一次"会永久禁掉它的
    试启用机会）；`record_boot_success` **不清空**上下文（计数器自愈 ≠ 诊断证据消失），
    前端用 `ts` 判断新鲜度；上下文与标记**同一次原子写**落盘，写失败只进
    `persistence.lastError`，读失败只让上下文为空（不把整份标记判成 Corrupt）。
    ⚠️ 已知代价：**应用降级**会多计一次失败——标记文件带版本号，新版写的标记
    被旧版读到即判损坏（`LoadSource::Corrupt`），按「上次未干净结束」计一次。
    方向保守（多计一次失败，不会漏计崩溃），一次成功上报即自愈；升级不受影响。
- **通知中心读写成对**：`host_notify(pluginId, title, body)` 只写
  （`tauron-notify` 环形缓冲，容量 256）；读取端是
  `host_notifications_list(limit?)` → `{ unread, total, items[], dispatchLog[] }`
  （`items` 时间倒序、`limit` 缺省 50；`unread` 恒为全量计数，角标用）与
  `host_notifications_read(id?)` → `{ marked }`（`id` 缺省 = 全部已读）。
  条目键为 camelCase（`pluginId`）。插件卸载时宿主自动回收该插件的通知
  （未读计数不得被死条目永久膨胀）。通知中心属主窗（`ShellClient`）。
  轮 11（R7）起 `host_notify` 真正推系统：挂载 `DispatchSink` 后走
  `tauron_notify::dispatch`，**顺序固定为 push → send → log**——先入缓冲再推系统，
  反过来（send 失败即不入缓冲）等于"系统通知失败就丢通知"。失败（`Ok(false)` 或 `Err`）
  一律降级为 `Degraded`，**通知仍在缓冲里**，`host_notify` 绝不因此报错。
  `dispatchLog[]` 元素 `{ entryId, outcome: "system"|"degraded"|"failed", ts }`，
  语义是 **dispatch 尝试**的日志、**不是通知本身**（两个环互相独立：通知被裁剪后
  日志仍在，反之亦然）；未注入 sink 时为空数组——没有尝试就没有日志，不伪造 `degraded`。
  ⚠️ 诚实边界：依赖闭包里没有 `tauri-plugin-notification`，而硬约束禁止新增外部依赖，
  因此 Tauri 实现做的是**两件真实但不同的事**：`app.emit("tauron://notification")`
  投递给前端刷新通知中心 + 对首个窗口 `request_user_attention(Informational)`
  （Win 任务栏闪烁 / macOS 程序坞跳动，仅 warning/error），然后返回 `Ok(false)` = 降级。
  这**不是** no-op，但**也不声称**系统通知气泡已送达：生产路径上
  `DispatchOutcome::System` 目前不可达（只有 mock sink 覆盖）。
  **事件载荷只带信号与归属，不带正文**（轮 11 审计修正）：`Manager::emit` 的语义是
  "发给**所有** target"，而多插件宿主里每个插件都有自己的 webview —— 载荷里一旦有
  `title`/`message`/`data`，宿主与别的插件的通知正文（可能含跳转 token）就进入了
  **每一个插件 webview** 的回调。现在载荷是 `{ id, pluginId, kind, ts }`，正文的唯一
  出口是按身份过滤的 `host_notifications_list`（插件只见自己的条目，见 §5）。
  残留（如实登记）：`pluginId`/`kind`/`ts` 仍对所有 webview 可见，即"某插件在 T 时刻发了
  一条 X 类通知"这层元信息；要连这层也去掉需 `emit_to("main")` + `emit_to("plugin-<id>")`
  定向投递，而那要求宿主知道主窗**真实** label（本仓未在真实运行时验证过 label 取值）。
  另一条已披露的限制：通知环是**全局共享**的（满 256 丢最旧），插件可以刷自己的通知把
  别人/宿主的条目挤出环（可用性破坏，不是可读性泄露）——按插件配额属设计变更，未做。
- **设置族的身份绑定（R7 + 轮 11 审计）**：`host_settings_get/set` 的线形不变
  （`{ key }` / `{ key, value }`），但轮 11 起按调用主体收口——主窗可读写任意键，
  插件主体只能读写**自己命名空间**内的键（`plugin:<id>` 前缀），越界拒绝且无写入副作用；
  畸形 label 一律拒绝（不降级成主窗）。原因：R4 的身份模型要求 self 档命令的身份
  **由宿主从 webview label 解析**，而此前设置族完全没有判定——插件 webview 可以写
  任何插件的键或宿主级键（跨插件完整性）。
  迁移能力有**显式线上入口**（轮 11 前它们是"没有渠道可达"的纯函数，只有 Rust 单测能碰到）：

  | 命令 | 线参数 | 返回 | 谁能调 |
  | --- | --- | --- | --- |
  | `host_settings_adopt_legacy` | `{ doc }`（对象 = 键→值，v1 裸键语义、可含 `.`） | `null` | 仅主窗 |
  | `host_settings_migrate` | 无 | 迁移**步数**（`0` = 已最新/无数据，**幂等**） | 仅主窗 |

  承接与迁移**必须分开**：v1 与 v2 的键编码契约不同（v2 走转义键，`%`/`.`/`$` 编码），
  直接把旧文档按新契约 `settingsSet` 进去，旧键会读不出来。宿主侧不做隐式改写
  （隐式迁移意味着改一个无关键也可能重写整层，出错无法审计）。迁移全有或全无：
  链缺失 / 自环（步数上限 16）/ 结果过不了新 schema / **有数据但无版本标注**
  → 一个字节都不改 + 返回 `E_INVALID_MANIFEST`；`adopt` 的非对象文档同样不留痕
  （不写用户层、不标版本，避免留下"有数据无版本"的半态）。
  数据版本存于 `SettingsStore`（`1.0.0` = 旧裸键契约，`2.0.0` = 转义键契约）；
  键编码 `%`→`%25`、`.`→`%2E`、`$`→`%24` 保证"一键 = 一段 = 一叶"，
  使写 `a` 标量后再写 `a.b` 不再撞上 `merge::write_path` 的中间节点断言。
- **运行期订阅审批**无线上入口：`EventBus::approve` 无命令/无 TS 方法，
  `approvals` 恒空，跨插件订阅私有 topic 只能靠声明方标 `public: true`；
  失败方向 fail-closed（`E_AUTH_DENIED`）。安装期授权另有一套（`grants.ts`
  纯策略库，需接入方显式调用才生效）。
- **进程插件运行时（P0-2）**两条命令（均主窗特权，见 §5）：

  | 命令 | 线参数 | 返回 |
  | --- | --- | --- |
  | `host_runtime_spawn` | `{ pluginId, profile }`（两个**顶层**参数） | `{ pid, lease }` |
  | `host_runtime_health` | `{ lease }` | `{ alive, pid, crashes, consecutiveFailures, reap }` |

  `profile` 走 camelCase：`{ binaryPath?, args, env, signature{algorithm,signature,signerId}, binaryHash, abi{rustVersion,interfaceHash} }`；
  **不含 `generatedAt`**——校验时刻由宿主时钟决定，前端给的时间戳不可信
  （字段同构由门禁逐个锁定）。

  语义（都是"两边不一致就会出事"的那类）：

  - **重复 spawn 幂等**：返回既有租约。两个并发入口若都按"尚无租约"判定，
    就会起两个进程而租约只指向其一 → 孤儿 PID，正好违背 `lease ↔ pid` 一一绑定。
    启动与登记在同一把 runtime 锁内完成（临界区含一次 `spawn`，毫秒级）。
  - **只有活租约才幂等**：租约对应的进程已死 → 先终止旧 pid 再换新租约。
  - **租约离开租约表的唯一途径是先终止进程**（卸载 / 清除 / 换新 / 显式移除四条
    路径同一条私有 `terminate()`）。只删表项 = 孤儿进程。终止失败**不**让卸载失败
    （租约必须消失，否则重装同名插件会撞旧租约），但一定计入 `reap`
    （`attempts` / `terminated` / `alreadyGone` / `failures` / `lastError`）；
    未注入终止器也按失败记账（漏注入表现为计数器增长，而非"看起来一切正常"）。
  - **失败码**：非 `process` 类型 → `E_PLUGIN_TYPE_NO_RUNTIME`（不伪造 pid）；
    未知/失效租约 → `E_LEASE_EXPIRED`（**不是** `E_CALL_NOT_FOUND`：调用方下一步是
    重新 spawn，而不是放弃一次 pending 调用）。
  - **崩溃预算**：`CrashTracker` 窗口内超限（缺省 3 次 / 5min）→ 复用
    `E_PLUGIN_DISABLED`（不可重试）+ 投 `RUNTIME_CRASH`… 预算耗尽走既有
    `RetryExhausted → ERRORED_USER_CONFIRM`；放行出口是既有
    `host_registry_admin(enable)`（同时清零进程侧窗口，否则会出现"用户已确认、
    状态机 Enabled、spawn 仍被窗口拒绝"的死结）。
  - **插件必须处于可用状态**（`ENABLED` / `RUNNING`）才允许 spawn：
    注册表说禁用、进程却在跑是逻辑不自洽。故 `INSTALLED` 直接 spawn、
    safemode（`DISABLED`）期间、`ERRORED_USER_CONFIRM` 均被拒（同 `E_PLUGIN_DISABLED`）。
    顺序是**先 enable/trial-enable 使状态可用，再 spawn**。
  - **spawn 成功即驱动状态机**：全新进程起来后由**宿主自己**投 `Event::Attach`
    （`ENABLED → RUNNING`）。进程插件没有 webview 可上报 ATTACH，不投就会出现
    「状态机说已启用、进程在跑」的不一致；幂等返回既有租约的那条路径**不重复投**。
  - **崩溃检测是轮询式**（不调 `host_runtime_health` 就发现不了死亡），无后台监控
    线程、无进程组/作业对象（`kill` 只覆盖直接子进程）；sidecar 的 stdin/stdout
    JSON-RPC 帧回路**未接线**（stdout 走 `Stdio::null()`，避免接管道无人读把子进程
    阻塞死）。以上属已披露的未接线范围，见计划「轮 10」。
- **重启与建窗（R8，两条新命令；均**仅主窗**，代码层判定见 §5）**：

  | 命令 | 线参数 | 返回 |
  | --- | --- | --- |
  | `host_window_relaunch` | 无 | `{ reconcile, relaunchRequested, reason }` |
  | `host_window_create` | `{ pluginId, title?, width?, height? }` | `{ label, pluginId, created, reason }` |

  - **顺序不变量（relaunch）**：宿主**先**对账恢复阶段（`reconcile` =
    `{ scanned, entered, exited, ignored }`），**后**请求重启。顺序反了会把"上一次
    未干净结束"的标记带进下一次启动 → 重启循环。`reconcile` 出现在返回里不是装饰：
    它是"顺序对了"在运行时可观测的证据（有测试钉住这个次序）。
  - **降级如实上报**：`relaunchRequested: false` / `created: false` 表示**宿主没有该
    原语**（非 Tauri 宿主、或宿主未注入 Sink），此时 `reason` 说明原因。
    调用方**不得**据此认为应用会重启 / 窗口已创建——这正是 R8 之前那类"仿真冒充成功"
    的对立面。
  - **label 由宿主铸，且只认 manifest**：`host_window_create` **没有 URL 参数**（刻意的）
    ——URL 只能来自 manifest 的 `entry.ui`；让调用方指定 URL 等于让主窗把"带插件身份的
    webview"指向任意地址，而 label 决定身份。窗口 label 恒为 `plugin-<插件 id>`，
    它同时是 `cleanup_closed_window` 的回收键，因此**不需要新钩子**。
  - **插件不在注册表 → 拒绝**（`E_UNKNOWN_PLUGIN`），且在任何窗口副作用之前失败，
    不留半态窗口。`host_window_create` 因此绑 `PluginRuntimeState`（属插件域命令集，
    档 3 才注册），而 `host_window_relaunch` 只需底座状态（底座集合里就有）。
- **商城三命令的返回值（R8 起有类型，`simulated` 是线字段）**：

  | 命令 | 返回 |
  | --- | --- |
  | `host_market_check` | `{ available, simulated, version, reason }` |
  | `host_market_download` / `host_market_install` | `{ ok, simulated, version, reason }` |

  为什么必须成为线字段：R8 之前这三条返回裸 `{ ok, simulated, version }` /
  `{ available: false }`，"这是模拟结果"只写在**宿主源码注释**里，前端拿不到任何判据。
  `ok: true` **只表示命令跑通**，不代表更新落地——判据是 `simulated`（当前恒 `true`：
  没请求任何 endpoint、没下载字节、没验签、没替换文件），`reason` 写明未接入更新源。
  真实连线后只需替换函数体：`simulated → false`、`reason → null`，**线形不变**。

## 4. 生命周期词表（镜像）

| 词表 | TS | Rust | 线名风格 |
| --- | --- | --- | --- |
| 状态（宿主单一写入） | `LIFECYCLE_STATES`（10） | `lifecycle::State` | SCREAMING_SNAKE_CASE |
| 事件（插件上报 + 宿主探测） | `LIFECYCLE_EVENTS`（18） | `lifecycle::Event` | SCREAMING_SNAKE_CASE |

两侧逐名、逐序一致，由 `@tauron/contract-tests` 的镜像门禁锁定。

`RUNTIME_CRASH`（第 18 条，P0-2）是**宿主探测**而非插件上报：轮询
`host_runtime_health` 发现 sidecar 不存活时由宿主投递，与 `ERROR_RETRYABLE`
同形（崩溃重启是有预算的自动重试，`tauron-proc` 的 `CrashLimit` 缺省
3 次 / 5min），预算耗尽才升级为需用户确认；试验性启用中崩溃按试验失败回落
`DISABLED`。

### 4.1 topic 命名空间（勿混淆的三套约定）

| 约定 | 形态 | 用途 |
| --- | --- | --- |
| 事件 topic | `plugin:<插件id>:<事件名>` | 事件总线投递（TS `eventNamespace` ↔ Rust `EVENT_TOPIC_PREFIX`，dispatch 以冒号切分） |
| i18n 文案键 | `plugin:<插件id>.oc.<键>` | 插件文案命名空间（`host_i18n_load` 传 `pluginId` 时自动加前缀；`host_i18n_cleanup_plugin` / Uninstall/Purge 按命名空间清理） |
| 框架内部源 | `deep-link` 等裸名 | 非插件发布源（`DEEP_LINK_TOPIC`，发布者 `core.deep-link`） |

应用层 topic 由**宿主装配方声明**（`EventBus::declare_topics` 先声明后订阅/发布；
发布未声明 topic 会被越界丢弃并计数）。manifest 的 `events.publish` 只声明裸名
（禁止含冒号），插件安装装配时由嵌入方补 `plugin:<id>:` 前缀。

## 5. 能力表（命令 → 授权档位）

TS `capabilities.ts` 的 `CAPABILITIES`（13 条 self/scoped-read 插件命令 + 3 条
privileged 主窗命令）与 Rust `authz::COMMANDS` / `ADMIN_COMMANDS` 逐命令、
逐档位同构（`self` / `scoped-read` / `privileged` kebab-case 线名），
由形状门禁锁定。

**覆盖范围（有意如此，不是遗漏）**：本表覆盖「**插件侧可触达的命令面** + 管理命令」。
主窗专属命令（窗口 / i18n / notify / settings / recovery / market / dialog /
clipboard / brand 等）由 Tauri ACL 按窗口 label 管辖，不在表内。判据是**谁可能越权**：
插件 webview 能摸到的命令必须有档位（否则授权层对它没有定义），主窗命令不存在"下放"路径。
轮 11 的审计**正是靠这条判据**抓出 4 条插件面命令（`host_events_drain`、
`host_stream_open/write/close`）此前完全没有档位：`HostClient` 已在调用它们，
而 CLI 脚手架又用本表当能力白名单（`packages/tauron-app-cli/src/scaffold.ts` 直接
`import { CAPABILITIES } from '@tauron/host'` → `validateCapabilities` → 生成
`src/capabilities.json`），于是这 4 条在脚手架应用里根本无法启用。已补登记，并加门禁锁死
「`HostClient` 触达的每条命令都必须在表内且档位合法」。

3 条特权命令及**为什么必须特权**：

| 命令 | 档位 | 理由 |
|---|---|---|
| `host_registry_admin` | privileged | 管理操作（disable/enable/uninstall/purge）改变别的插件的状态 |
| `host_runtime_spawn` | privileged | **进程执行原语**：按入参 `pluginId` 启动可执行文件。插件 webview 若能调用，任何插件都能起别人的 sidecar |
| `host_runtime_health` | privileged | 暴露 pid 与崩溃计数（同样的信息面） |

档位的**强制点**分两种注册形态（§1）：`plugin:tauron|<cmd>` 形态由 Tauri v2
capability/ACL 强制（生产客户端应采用——把 3 条特权命令只授予主窗、
不给插件 webview）；裸命令形态下 origin ACL 是唯一咽喉点。
**轮 11 起**代码层再叠一层判定，把"特权 = 仅主窗"从"只靠部署配置"变成**代码里的真判定**。
三类判定、两个函数（都在 `crates/tauron-adapter/src/lib.rs`，拒绝码一律是既有的
`E_AUTH_DENIED`，不新增错误码；判定点一律在**任何副作用之前**）：

| 判定 | 函数 | 命令 |
| --- | --- | --- |
| 仅主窗 | `require_main_window(caller, cmd)` | `host_registry_list_all`、`host_registry_admin`、`host_runtime_spawn`、`host_runtime_health`、`host_settings_adopt_legacy`、`host_settings_migrate`、`host_window_relaunch`、`host_window_create`（R8）、`host_recover_trial_enable`、`host_market_check`/`download`/`install`、`host_i18n_set_locale`（切的是**全局**语言）、`host_deep_link_register`（写的是**应用级**协议） |
| 绑定到自己的键空间 | `require_settings_key_scope(caller, key)` | `host_settings_get` / `host_settings_set`（插件只能读写 `plugin:<自己>.…`；用 `strip_prefix` + 空/`.` 判据，裸前缀比较会让 `plugin:p.a` 与 `plugin:p.ab` 互相穿透） |
| 绑定到自己的身份 | `require_self_plugin_scope(caller, cmd, claimed)` | `host_notify`（署名）、`host_i18n_load`（命名空间归属）、`host_i18n_cleanup_plugin`（销毁目标）、`host_recover_report`（失败预算 / 故障归因）、`host_notifications_read`（只能标记**自己**的通知；`None` = 全局"全部已读"与未知 id 一律拒绝——对未知 id 放行会让返回值变成存在性预言机）——插件只能以自己名义；`None`（宿主级命名空间 / 应用级上报）对插件一律拒绝，主窗任意 |
| **按身份过滤（不是拒绝）** | `visible_notifications(...)` | `host_notifications_list`：插件只拿到署名是自己的条目，且 `total`/`unread`/分页/`dispatchLog` 与**过滤后的可见集合同源**（只裁数组、留着全局未读数仍是泄露；`limit` 必须在过滤**之后**取，否则别人的条目会占满窗口把插件自己的挤掉）。主窗数学上等于原实现 |

为什么后两类不能一刀切成"仅主窗"：插件**本来就需要**读自己的设置、以自己名义发通知、
装自己的文案、报自己的状态——一刀切会关掉正常功能，所以判的是"**以谁的名义**"。

**为什么这几条必须有代码层判定**（每一条都是真越权原语，不是理论风险）：
`host_recover_trial_enable` 消耗**别人的**试验预算（1 次即回落、之后不可再试）；
`host_market_*` 操作的是**宿主级产物**（接入后 = 把宿主指向攻击者更新源 / 替换应用自身二进制，
现状是桩但线形同形，接线时若判定缺席，越权面会在没人注意时从"无害桩"变成"供应链入口"）；
`host_notify` 的 `pluginId` 是**署名**（不判定即可伪装宿主告警或以别的插件名义栽赃）；
`host_i18n_load` 的 `pluginId` 就是**命名空间归属**（A 写进 B 的命名空间 = B 界面上每句文案
都可能被改写，不执行任何代码就能伪造 B 给用户看的内容）；`host_i18n_cleanup_plugin` 是
**销毁**；`host_recover_report` 的 `pluginId` 决定后果落在谁头上，且**不可逆**（对正处于
`TrialEnable` 的插件报一条 failure 就能永久关掉它）；`host_notifications_read` 的 `None` 是
**全局**"全部已读"（插件调一次就把宿主的未读角标清零），`Some(别人的)` 等于替别人把消息吞掉；
`host_notifications_list` 返回的是**通知正文**（含跳转载荷）而不是元信息，不按身份过滤就是
内容泄露；`host_i18n_set_locale` 改的是**整个应用界面**的单一语言状态源，而且**静默成功**
（用户看到界面变语言的根因在另一个插件的一次调用里）；`host_deep_link_register` 写的是
**应用级**协议，换值会先注销旧协议 → 插件一次调用就能让用户点原本的 `tauron://` 链接
**不再拉起本应用**。

**安全边界（如实说明）**：这三个判定只回答"**你能不能以这个主体身份做这件事**"，
**不判**目标是否存在/是否已安装（那是 `E_UNKNOWN_PLUGIN` 的语义），也**不替代** ACL——
部署期仍需把 3 条特权命令只授予主窗。另外 `crates/tauron-adapter` 目前**没有
`permissions/` 目录**，走 `plugin:tauron|…` 路由的宿主在启用能力检查时会因缺权限条目被拒
（部署配置缺口，见 §1 的告警）。

## 6. 权限词表分层（勿混淆）

- **内层（框架 ACL，动态授予）**：§3.3 风格（`store:read`、`http:fetch`…），
  TS `@tauron/types` `PERMISSION_GRANULARITY` 为描述词表；框架层
  `check_plugin_permission`（Rust）/ `getMissingPermissions`（TS）是
  **嵌入式扩展点**，默认分发主链不调用。
- **外层（应用层 manifest，静态声明）**：只能取自机器生成的
  `schema/permissions.index.json`（Tauri 标识符，如 `store:allow-get`），
  表外即安装失败（`manifest.validate` 强制）。

## 7. 错误契约（结构化穿越，不是文本转储）

宿主错误以 **JSON 对象**穿越 IPC：

```json
{ "code": "E_CALL_TIMEOUT", "message": "call expired", "retryable": true }
```

- `code` 为 `E_*` 大写蛇形，**线名即 Rust 枚举变体名**（`ErrorCode` 未配
  `rename_all`，`Display` 输出与序列化一致）；**18 个**变体与 TS
  `HOST_ERROR_CODES` 逐名、逐序镜像。
- `retryable` 三值（`E_CALL_TIMEOUT` / `E_HOST_PANIC` / `E_PLUGIN_FILTERED`）
  与 Rust `ErrorCode::retryable()` 同集合；TS 侧以
  `RETRYABLE_HOST_ERROR_CODES` 判定，两者由门禁锁定。
- `message` 面向开发者/日志，**不得**作为 UI 文案或分支判断依据。

Rust 侧 `to_tauri_err` 必须走 `TauriError::from(error)`（`InvokeError` 底层是
`serde_json::Value`，`From<T: Serialize>` 原样保留字段），
**不得**用 `format!("{:?}", e)`——文本转储会丢掉 `message`，
前端只能靠正则从 `HostError { code: E_XXX, … }` 里反抠错误码。
Tauri `InvokeError(pub serde_json::Value)` 是值语义，故结构化形态可以无损抵达。

TS `normalizeError` 的还原优先级（任一失败即回退下一级，永不抛异常）：

1. 对象带 `E_*` 的 `code` 字段 → 直接取 `message` / `retryable`；
2. 内层消息是 JSON 字符串（运行时把拒绝值包成字符串时）→ `JSON.parse` 还原；
3. 正则 `/(E_[A-Z_]+)/` 抽取（宿主未结构化、或 Tauri 压成纯文本时）；
4. 以上皆无 → `E_UNKNOWN` 且 `retryable: false`（不猜测）。

框架层另有独立码表 `SC-####`（`tauron-shell` ↔ `@tauron/types` `PluginErrorCode`），
与应用层 `E_*` **不共用命名空间**，各自由门禁锁定。

### 7.1 两套词表的边界是**显式**的（R2-c）

「有两套词表」不是缺陷，「穿越边界时哪套胜出从未定义」才是：调用方按 `code`
分支，却可能在边界处被**静默换码**。因此边界变成显式枚举 + 显式翻译：

```ts
export type HostBoundary =
  | 'plugin-webview→host'   // invoke 拒绝：强制归一为宿主词表的 HostException
  | 'host→plugin-webview'   // 宿主码原样保留，不二次翻译
  | 'plugin-internal';      // 插件内部：应用层 SC-#### 原样保留

translate_at_boundary(err, boundary): {
  error: HostException;     // 边界之后只有一种形态可用
  boundary: HostBoundary;   // 在哪个边界翻译的
  translated: boolean;      // 码是否被**改写**（不在宿主词表内 → 收窄为 E_UNKNOWN）
  foreignVocabulary: boolean; // 原始码属另一套词表（SC-####）
  rawCode: string | null;   // 原始码**永不丢弃**
}
```

- 穿越点一律 `translate_at_boundary(...).error`，**不得**就地
  `new HostException(normalizeError(err))`——后者丢掉边界身份，也让「本端不认识
  那个码」与「宿主真的返回了 `E_UNKNOWN`」混成同一个值（门禁锁定）。
- `translated: true` 表示发生了跨词表收窄：日志/遥测据此区分这两种情形。
- **`SC-####` 现在也被识别为「像码」并保留**：此前 `normalizeError` 只认 `E_*`，
  跨层抵达的应用层码会被整条丢弃（`rawCode: null`），是"边界隐式"的最典型症状。
  识别形态 ≠ 接受语义：非宿主码仍是 `E_UNKNOWN`（判定用），只多了原始码（诊断用）。
- 两套词表**不合并**：`HOST_ERROR_CODES` 里不得出现 `SC-####`，
  `PluginErrorCode` 里不得出现 `E_*`（门禁锁定）。
- UI 展示层（`@tauron/ui` 的 plugin-manager）只做**展示归一**（读已归一错误的
  `code`/`message`），不构造边界异常，因此不属于穿越点。
- **`HostBoundary` 是全集，接线的只是子集**，未接线的那两个值由
  `UNWIRED_BOUNDARIES` 显式登记（门禁要求"每个值要么有生产调用点、要么在清单里"，
  两处都没有即失败；接线后必须从清单删掉，否则清单自身会开始说谎）。
  两个未接线的值及原因：`host→plugin-webview` 与 `plugin-internal` 的消费方都在
  插件侧 SDK，而插件侧 SDK 不应依赖 `@tauron/host`（会把整个宿主客户端打进插件包）；
  已定位的缺口是 `packages/tauron-plugin-sdk/src/bridge.ts` 的 handler 失败分支——
  插件抛出的 `SC-####` 被硬写成通用 `SC-9001`（码被丢掉，与 R2-c 修的是同一类问题，
  只是发生在插件侧、需要在保持依赖方向的前提下修）。

## 8. 调用生命周期不变量（pending 表不得楔死）

`host_plugin_call` 登记的 pending 条目带 `expires_at = now + pending_ttl`
（配置项 `pending_ttl_secs`，默认 30s）。存活不变量：

- **`gc_expired()` 必须在生产路径被调用**。它在 `call_begin` 的容量检查前，
  于**表已满**时先行驱逐过期条目，再重判容量。否则僵尸条目（前端超时后
  从未 `callEnd` 的调用）会永久占用容量位，累积到 `max_pending_calls` 后
  所有后续调用永久 `E_CALL_PENDING_FULL`——一次永久挂起即可楔死整条调用链。
- **只在满时驱逐**：稳态（表未满）不做 GC，保证 `call_status` 对过期条目
  仍返回 `E_CALL_TIMEOUT`（可重试）而非 `E_CALL_NOT_FOUND`（不可重试）——
  两者语义不同，不能塌缩。
- **不得驱逐在途条目**：`gc_expired` 只回收 `expires_at <= now` 的条目，
  合法在途调用（含 stream 长调用）不受影响。
- **窗口关闭必须清理**：`call_end_all(pluginId)` 一次清空该插件全部 pending
  （ADR-04：不清理会导致 JS 侧 `invoke()` 永久挂起）。
- **TTL 短于前端超时是安全的**：前端默认 30s 超时，宿主 30s TTL 到期即被
  GC；宿主侧 `call_end` 对已过期条目仍返回 `E_CALL_NOT_FOUND`，前端不会
  把已超时调用的迟到终帧误当成功。

## 9. 资源回收契约（谁负责清、什么时候清）

订阅、反向索引、队列与 pending 调用**全部以插件 id 为键**。注册表条目消失后
它们再无引用者，不回收就留到进程结束（卸载→重装、窗口反复开关都会累积）。

| 时机 | 回收什么 | 谁执行 |
| --- | --- | --- |
| 卸载 / 清除（`host_registry_admin`） | 订阅 + 反向索引 + 队列 + **topic 声明** + pending | **适配器自动**（`cmd_registry_admin` 迁移成功后调 `dispose_publisher` / `dispose_subscriber`） |
| 窗口销毁 | 该插件的订阅 + 反向索引 + 队列 + pending（**不动 topic 声明**） | **宿主装配方**：App Builder 的 `on_window_event(Destroyed)` → `tauron_adapter::tauri::cleanup_closed_window` |
| 表满时的 TTL GC | 过期 pending 条目（§8） | 适配器自动 |
| 队列溢出 | 见下（Event/State 丢最旧；Request 报错） | 适配器自动 |

关键点：

- **窗口销毁回收必须由宿主接线**。`tauri::plugin::Builder` **没有**窗口事件钩子
  （只有 App Builder 有），适配器无法自行注册；`examples/minimal-app` 已示范。
  不接线时订阅会保留到卸载或进程结束——这是**契约**，不是待修 bug。
- **无需「最后窗口」判定**：身份模型是**一插件一 webview**，label 恰为
  `plugin-<插件id>`（`tauron-acl` 生成的 capability 同样是
  `"webviews": ["plugin-<id>"]`），label 与插件 id 一一对应；带后缀的子窗口
  label 解析出的 id 含 `:`，`PluginId::new` 直接拒绝，不在身份模型内。
- **窗口关闭不回收 topic 声明**：声明留到卸载才回收，否则窗口重开时发布者
  会因「topic 未声明」被静默丢弃。
- **禁用不回收**：`disable` 之后仍可 `enable`，订阅与声明必须原样保留。
- **各表都有上限（`window` 不是无限增长维度）**：`topics` 超限走
  `evict_overflow`、单插件队列有 `MAX_QUEUE=1000`、pending 有
  `max_pending_calls`、通知有 `NotifyStore::capacity`、通知**兼容日志**有
  `MAX_NOTIFICATION_LOG=256`、订阅表有 `MAX_SUBSCRIPTIONS=4096`。
  最后一条是必需的：`(subscriber, window, topic)` 才是幂等键，而 `window` 由
  调用方给定且不校验（`host_events_subscribe` 属 self 档），没有上限时任何插件
  都能用不断变化的 `window` 让 `subs` / `topic_subscribers` 无限增长。
  达限返回 **`E_SUBSCRIPTION_FULL`**（确定性失败，不可自动重试，与其余「表满」
  类一致）；**幂等重复订阅不受上限影响**（复用既有 token），且卸载/退订会释放
  容量，故正常使用不会撞限。

### 9.1 通道溢出语义（三通道不同）

| 通道 | 溢出行为 | 前端可见性 |
| --- | --- | --- |
| `event` | 丢最旧，`overflow` 计数 | 静默（事件可丢，设计文档 §2.3） |
| `state` | 保持最新一帧（快照语义） | 静默 |
| `request` | **硬失败**：`E_CALL_PENDING_FULL`（可重试） | `eventsPublish` 抛 `HostException` |

`host_events_publish` 走 `request` 通道（可靠语义），因此
`eventsDrain()` **默认 `kind: 'request'`** 与其配对——只订阅不取件等于没订阅：
队列只进不出，最终溢满并让后续发布持续报错。`drain` 的 `kind` 只能是
`event` / `request` / `state`，表外值返回 `E_INVALID_MANIFEST`。

**仓内取件闭环**：`@tauron/app-plugin-sdk` 的 `PluginContext` 内置取件泵——
`events.subscribe` 建立宿主订阅后自动周期 `eventsDrain('request' | 'event')`
并按 topic 分发给本地订阅者（退订/`disposeEvents` 收泵）。接入方若自建桥接
不走 `PluginContext`，必须自行成对调用 `eventsDrain`，否则帧只进不出。

**贡献注册身份绑定（self 档）**：`host_contributes_register` 线形是
`{ entry: { kind, id, label } }`，**不收 `pluginId`**——署名由宿主从 webview
label 解析（非插件 label 直接 `E_AUTH_DENIED`）。插件侧入口是
`HostClient.contributesRegister`（`createPlugin` 激活 `def.contributes` 时
自动调用，best-effort：重复 id 只告警不阻断激活）；主窗 `ShellClient` 只保留
读取（`contributesList`，scoped-read）。
