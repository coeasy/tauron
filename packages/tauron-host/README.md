# @tauron/host

tauron 能力编排层（开发计划 §4.9）。

宿主侧对应实现：`crates/tauron-host`（Rust）。

## 职责

- `invoke` / `event` / `channel` 封装
- 结构化错误规范化（ADR-04：宿主永不向 JS 抛裸 panic）
- 能力探测（每能力有 `available()`）
- **Backend provider 抽象**（ADR-16）
- tree-shaking 友好导出（`sideEffects: false`）

## 关键约束

1. **唯一 `@tauri-apps/api` 引用点** = `src/tauri-backend.ts`。
   其余模块只依赖 `Backend` 接口，因此可在无 Tauri 依赖的环境下
   （契约测试、SSR 冒烟、类型检查）完整运行。
   该约束由 `src/gates.test.ts` 的静态扫描门禁保证（§8-1）。
2. **二进制载荷禁止 base64 走 JSON**（架构 §4.8 R6）：走 `argsRaw` + `Channel`。
3. **Backend 的浏览器/mock 实现仅供契约测试，不作为 Web 交付**（ADR-16）：
   mock 单独从 `@tauron/host/testing` 引入。
4. `self` 档命令的 `pluginId` **不得**由调用方传入（ADR-17）——宿主只认
   webview label（`plugin-<id>`）。
5. **能力表只来自运行期协商**（V4 §8.1 R1-4）：`TauriBackend` 构造时只认
   bootstrap 命令（`BOOTSTRAP_COMMANDS` = `['host_capabilities']`），其余命令
   在协商回填前一律 **unknown = unsupported**。`adoptCapabilities()` 拒绝空集合、
   也拒绝不含 `host_capabilities` 的集合（那说明协商根本没成功，不能当能力真相采纳）。
   `FRAMEWORK_COMMANDS` 只是**门禁/文档基线**，不是运行期能力表——历史上它被无条件
   灌进 `capabilities()`，于是 feature-gated 命令对 `available()` 误报已注册，
   调用方直到 `invoke` 才拿到 `command not found`。

## 导出

```ts
// 核心（无 Tauri 依赖）
import { HostClient, normalizeError, capabilityMatrix, CAPABILITIES } from '@tauron/host';

// 真实后端（唯一 import 点）
import { TauriBackend, pluginIdFromLabel } from '@tauron/host/tauri';
// 命令面基线（门禁/装配自检用；**不是**运行期能力表，可用性请读协商结果）
import { FRAMEWORK_COMMANDS, OPTIONAL_FRAMEWORK_COMMANDS } from '@tauron/host/tauri';

// 契约测试专用
import { MockBackend } from '@tauron/host/testing';
```

协商用法（宿主启动后一次性回填，失败就保持 fail-closed 并重试）：

```ts
const backend = new TauriBackend({ commandPrefix: '' });
const caps = await backend.invoke<{ commands: string[] }>('host_capabilities');
backend.adoptCapabilities(caps.commands); // 拒空 / 拒缺 host_capabilities
// 之后 shell.supports(...) 与 isAvailable(backend, ...) 才报得出真实可用性
```

两个探测入口口径不同：`supports()` 只答**注册与否**（构建 / 协商事实，feature-gated 命令
在默认构建下为 `false`）；`isAvailable()` 与 `capabilityMatrix()` 还按**调用方主体**过滤
——`host_capabilities` 返回的是构建级命令集、不带调用方参数，宿主不会替插件 webview 少报，
所以 SDK 自己按 `Backend.principal()` 过滤：插件主体（含畸形 label）下 10 条主窗特权命令
（`host_registry_admin` / `host_runtime_spawn` / `host_production_doctor` 等）一律报
`false`，主窗主体可见全部。真正的执行判定在宿主代码层（`require_main_window`）。

## 跨语言契约门禁

`src/gates.test.ts` 读取真实 Rust 源码并断言两侧同构：

| 门禁 | 内容 |
|---|---|
| §8-1 | 整个 `src/` 只有 `tauri-backend.ts` 导入 `@tauri-apps/api` |
| 错误码 | TS `HOST_ERROR_CODES` 与 Rust `error::ErrorCode` 枚举**按码名集合全等**（顺序不属于协议，V4 A69/§89；改名或漏码即红） |
| 可重试 | TS `RETRYABLE_HOST_ERROR_CODES` 与 Rust `ErrorCode::retryable()` 的 `matches!` 集合一致 |
| 命令面 | TS `CAPABILITIES` 与 Rust `authz::COMMANDS` / `ADMIN_COMMANDS` 逐项一致 |
| 废弃命令 | `host_grant_request`（D16）、`host_call_begin`（D2）两侧都不存在 |

因此任何一侧改了协议而忘了同步另一侧，CI 会在这里失败。

## 命令面（计划 §2.1 定稿）

**28 条已登记命令**：`crates/tauron-host/src/authz.rs` 的 `COMMANDS` **20 条**
（18 `self` + 2 `scoped-read`）+ `ADMIN_COMMANDS` **8 条特权**（仅主窗）；
另有 **2 条 feature-gated**（`host_registry_install_preview` / `host_registry_install`，
挂 `plugin-install`——该特性是 **opt-in**（`crates/tauron-adapter/Cargo.toml` 的
`default = []`，V4 minimal-substrate 规则），接入方须显式
`features = ["plugin-install"]` 才注册）。

> 「28 条」是**需登记档位**的命令；Tauri 实际注册的命令面是另一个口径：**83 条**
> （`tauron_plugin_handler!` = 底座 61 + 插件运行时 22），显式开启 `plugin-install`
> 后 85 条。两侧一致性由 `src/gates.test.ts` 逐名比对，不靠本文维护。

| 命令 | 档位 | 消费方 |
|---|---|---|
| `host_plugin_call` | self | plugin-sdk `createPlugin()` |
| `host_call_end` | self | plugin-sdk（流式终帧确认） |
| `host_cancel` | self | plugin-sdk |
| `host_lifecycle_report` | self | plugin-sdk 生命周期钩子 |
| `host_contributes_register` | self | plugin-sdk（attach 期） |
| `host_events_publish` | self | plugin-sdk |
| `host_events_subscribe` | self | plugin-sdk |
| `host_events_unsubscribe` | self | plugin-sdk（dispose 期） |
| `host_events_drain` | self | plugin-sdk（事件取件泵） |
| `host_stream_open` | self | plugin-sdk（流式开流） |
| `host_stream_write` | self | plugin-sdk（流式写帧） |
| `host_stream_grant` | self | plugin-sdk（流式 credit 补充，V4） |
| `host_stream_close` | self | plugin-sdk（流式收尾） |
| `host_contributes_reconcile` | self | plugin-sdk（激活后自检贡献声明） |
| `host_call_plugin` | self | 宿主主窗 / plugin-sdk（插件→插件） |
| `host_call_result` | self | plugin-sdk（执行方回填） |
| `host_call_take` | self | 宿主主窗 / plugin-sdk（发起方取件） |
| `host_capabilities` | self | 任何主体（R1-4 协商入口：宿主自述命令面，只读，故不要求身份） |
| `host_registry_list` | scoped-read | plugin-sdk / `<oc-plugin-manager>` |
| `host_contributes_list` | scoped-read | `ShellClient.contributesList` / 应用设置中心 |
| `host_registry_admin` | privileged | 宿主 UI 主窗（`<oc-plugin-manager>`） |
| `host_runtime_spawn` | privileged | 宿主 UI 主窗（插件生命周期监管） |
| `host_runtime_health` | privileged | 宿主 UI 主窗（插件生命周期监管） |
| `host_resource_stats` | privileged | 宿主 UI 主窗（资源配额诊断） |
| `host_events_approve` | privileged | 宿主 UI 主窗（Event 审批，主题须已声明） |
| `host_events_revoke` | privileged | 宿主 UI 主窗（撤销 Event 审批） |
| `host_events_approvals` | privileged | 宿主 UI 主窗（只读审批事实清单） |
| `host_production_doctor` | privileged | 宿主 UI 主窗（生产就绪自检 A109，只读） |

> **订阅取件**：`host_events_subscribe` 只把帧排进每订阅者队列；
> 前端必须调用 `HostClient.eventsDrain(kind)` 取走帧（`host_events_publish`
> 走可靠语义，帧在 `request` 通道）。不取件等于没订阅。

## 主窗客户端（应用层）

以下客户端面向**主窗**，通过 `host_window_*` / `host_dialog_*` / `host_clipboard_*` /
`host_market_*` / `host_deep_link_*` / `host_capabilities` 等主窗命令族工作（完整命令面见
`crates/tauron-adapter/src/tauri.rs` 的 `tauron_substrate_handler!` / `tauron_plugin_handler!`
宏：底座 **61** + 插件运行时 **22** = **83** 条；`plugin-install` 另 **2** 条为
**opt-in**（`default = []`），接入方显式开启后共 **85** 条）：

以下三项在**本仓里有真实消费者**（`examples/minimal-app/src/main.ts` 逐条 `new`），
因此可以直接当"接上就能用"的参考实现读：

- `ShellClient` / `ShellController` — 标题栏动作、主题、更新事件路由（已接线）。
  更新对话框（`<oc-updater-dialog>`）的「检查更新」自轮 31 起走真通道
  `host_updater_check`，判定与 `AutoUpdateClient.checkUpdate()` 共用同一个
  `toUpdateInfo`；结论经 `onUpdaterCheck` 交回接入方写 UI，`currentVersion` 是
  构造参数（缺它则**零调用**并报错，不回落宿主桩）。对话框主按钮派发什么事件**不在
  这里决定**：状态词表与「状态 → 主按钮动作」映射都在 `@tauron/shell-events`
  （`UPDATER_STATUSES` / `UPDATER_PRIMARY_ACTION`，轮 32），只有 `ready` 点亮
  「立即重启」→ `oc-restart`。
  答"没有更新"时自轮 33 起还会补一句**通道事实**：`host_updater_status` 是灰度批次、
  崩溃门禁、宿主进程内账本的唯一出口，此前在 TS 侧零消费者，于是"你不在灰度批次"
  "被崩溃门禁停发""宿主没装端点"在 UI 上都是同一句"没有更新"。补读数的口径与
  `AutoUpdateClient.checkUpdate()` 共用同一个 `enrichUpdaterInfoWithChannel`（两条入口
  各拼一句就是第三面镜像）；`state` 与 `stateSimulated` 成对读——账本现有写入方都是
  `simulated` 桩，只看 `state` 就会把"点了一下模拟安装"说成"已安装"。命令缺席 / 报错 /
  答空三种情况都**不改**检查结论，有更新时**不**多打这条诊断命令。
  插件安装的审批问法自**轮 56** 起照办宿主的判定：`host_registry_install_preview` 每行带回
  `defaultChecked` / `scope` / `confirmationHint`（由 Rust `tauron_acl::build_approval_rows` 单源
  生成，轮 55），`_installPlugin` 按 `defaultChecked` **分叉**——默认真勾的行是一问一答，
  默认不勾的高危行显示 `scope` 并要求**逐字输入**宿主给的确认词。输入不一致 / 用户取消 /
  宿主没给确认词，三种情况都**整次安装中止**（绝不把缺项的批准集交给宿主换一句"已安装"）；
  缺确认词那条走 `onError`（context `'plugin.install'`），因为那是宿主契约缺口而不是用户选择。
  行形状在 TS 侧只有 `grants.ts` 的 `ApprovalRow` 一处声明，`host.ts` 引用它——曾经那份字段名
  与线上不同（`humanText` vs `description`）的平行类型已改成逐字段镜像。逐字比对发生在**壳层**：
  绕过壳层直接 `invoke` 安装命令的路径归 A83 的评审令牌链管，不在这一环。
  通知中心自**轮 36** 起接上。宿主 `TauriDispatchSink::send` 每次都向
  `tauron://notification` `emit` 一条**信号**（`{ id, pluginId, kind, ts }`，**不带正文**——
  `emit` 是广播给所有 webview 的，正文进广播就是跨插件内容泄露，轮 11 因此把它降成信号），
  而正文的唯一出口是 `host_notifications_list`（按调用方身份过滤）。此前**全仓没有任何代码
  监听那个 topic**，`notificationsList()` 也只有自己的单测在调：发出端一路 `Ok`、读端零消费者，
  通知到不了用户而且**不报错**。现在 `start()` 订阅 `NOTIFICATION_TOPIC`（该线值常量由
  `@tauron/host` 导出，是 Rust 侧 `pub const` 的镜像，两者一致性由 `wire-gate` 逐字比对，
  不靠注释），收到信号后拉一次快照交回给你：

  ```typescript
  const controller = new ShellController({
    backend,
    notificationLimit: 20, // host_notifications_list 的 limit，缺省 20
    onNotification: (snapshot) => {
      badge.textContent = String(snapshot.unread); // 未读是全量真值，不受 limit 影响
      for (const item of snapshot.items) {
        if (!item.read) toast.push({ title: item.title, message: item.message, level: item.kind });
      }
    },
  });
  controller.start(); // DOM 事件 + 宿主信号订阅
  controller.stop(); // 退订；在途拉取的晚到回帧由代际令牌丢弃，不再上屏
  ```

  三条口径：**不传 `onNotification` 就完全不订阅**（没人展示还挂监听 = 每条通知白打一次命令）；
  **突发合并**（在途只允许一个拉取，期间到达的信号合并为收尾补拉一次，5 条信号 = 2 次命令）；
  **失败必有出口**（订阅失败 `notification.subscribe`、拉取失败 `notification.pull`、
  你的回调抛错 `notification.render`，全部走 `onError`，不静默吞）。
  标记已读仍走 `ShellClient.notificationsRead(id?)`——控制器**不代你做这个决定**：
  自动清未读会把"用户还没看见"变成"用户看过了"，那是产品语义，不是接线细节。
- `AutoUpdateClient` — 检查/下载/安装/重启状态机 + 定时自动检查。检查腿（轮 29）
  读 `host_updater_check` 且要求 `currentVersion`；下载/安装两条仍是有意的
  `simulated` 桩，客户端有守卫拒绝把桩结果推进为"已下载/已安装"。
  `UpdateStatus` 自轮 32 起是契约 `UpdaterStatus` 的**类型别名**，不再自己列一遍字面量：
  此前它安装成功的终态叫 `ready`，而对话框的重启分支等的是 `done`（没人产出），
  于是「立即重启」在真装配里点不出来。同理，`ready` 今天只能由**真实**下载+安装到达
  ——走宿主桩到不了那个状态，这是设计而非缺陷。
  **命名坑（轮 33 记录，未改）**：本包里 `UpdaterStatus` 这个名字指**两样东西**——
  `@tauron/shell-events` 的状态词表（`idle`/`ready`/…，`UpdateStatus` 是它的别名）与
  `shell-client.ts` 的 `host_updater_status` 线读数（通道状况）。后者是 1.0 起 `index.ts`
  的公开导出，改名属破坏性变更，要走单独批准，所以本包内部先用导入别名
  （`UpdaterStatus as UpdaterChannelStatus`）把两件事分开。读代码时别把两者当一个。
- `DialogClient` — 文件对话框 / 消息框 / 剪贴板
- 菜单与托盘（R9，**轮 37 接上点击腿**）— `ShellClient` 的六条主窗命令
  （`menuSet` / `menuPopup` / `menuReset` / `trayCreate` / `traySetMenu` / `trayRemove`）
  建的是**原生**菜单；缺省装配（非 Tauri 宿主）时宿主如实返回 `UnsupportedBody`，
  不假装弹出。点击的回传走 **Tauri 事件通道**（`AppHandle::emit`），**不是**
  `host_events_*` 总线——此前四处叙述都写着「经 `host_events_drain` 取件」，照它接线
  的人会在一条永远不会有菜单帧的队列上等一次点击（那条总线是订阅 + 取件的拉模型，
  与点击回传没有接线关系）。

  ```typescript
  import { MENU_CLICK_TOPIC } from '@tauron/host';
  import type { MenuClickFrame } from '@tauron/host';

  // 先挂接收方，再建菜单：宿主没有登记路由时就不发帧，早订阅收不到假事件。
  await backend.listen(MENU_CLICK_TOPIC, async (payload) => {
    const click = payload as MenuClickFrame; // { id, source: 'menu' | 'tray', native: true }
    if (click.id === 'menu-minimize') await shell.windowMinimize();
  });

  await shell.menuSet({
    items: [{ id: 'menu-minimize', label: '最小化', event: MENU_CLICK_TOPIC }],
  });
  await shell.trayCreate({
    tooltip: 'my app',
    menu: { items: [{ id: 'menu-minimize', label: '最小化', event: MENU_CLICK_TOPIC }] },
  });
  ```

  三条口径：**`event` 缺省的菜单项只记录不发布**（点了没有帧，宿主不替你猜 topic）；
  **应用菜单与托盘是两条 lane**——`menuReset()` 只失效应用菜单那条路由，托盘右键菜单
  仍可回传，`trayRemove()` 反之（宿主侧 `MenuRouteTable` 按 lane 整体替换，旧 id 随之
  失效，不会残留一条把点击发往无人监听 topic 的路由）；**降级装配零帧**
  （`native_supported()` 为 `false` 的 sink 不登记路由，`listen` 挂上也等不到事件）。
- 事件 topic 词表（**轮 38 全量收口**）— `host-topics.ts` 是宿主 `emit` 的全部五条 topic
  在 TS 侧的唯一镜像（`NOTIFICATION_TOPIC` / `MENU_CLICK_TOPIC` / `DEEP_LINK_TOPIC` /
  `DIALOG_DEGRADED_TOPIC` / `DEEP_LINK_NATIVE_TOPIC`，均从本包入口导出）。线值在 TS 侧
  **恰好只出现一次**且就在该文件里，`wire-gate` 与 Rust 的 `pub const` 逐字比对——
  要用线值就从这个模块导入，别手打字符串。两条诊断帧（`DIALOG_DEGRADED_TOPIC` /
  `DEEP_LINK_NATIVE_TOPIC`）**仓库内零监听方**：它们只是给接入方核对用的镜像，权威结论
  在命令返回值里，挂监听等结论等于等一条不会到的事件。

`DeepLinkClient`（`host_deep_link_*`）与 `WindowState`（`host_window_*`）也在同一
命令族上工作，但**本仓示例不调用它们**——它们和下一节的库级 API 一样是
**面向接入方宿主应用**的参考实现，各自有单测。轮 16 前它们混在本节列表里，
读起来像"示例已经接上"，那是不实的接线声称（见下一节表格）。

> **启动编排不在本包**：曾经有一个 `bootstrap()`（7 阶段启动编排：配置 → 动效 →
> splash → 插件拓扑注册 → …），但它**只被自己的测试引用**，且是 `@tauron/host`
> 里唯一一处运行时 import `@tauron/core` 的地方——本包声明 `sideEffects: false`，
> 于是它在示例产物里根本不存在。它已在 1.0-W1 删除。启动编排属**应用装配层**：
> 接入方按自己的顺序组合上面的客户端（见 `examples/minimal-app/src/main.ts`）。

## 库级 API（宿主 App 侧，仓库内没有消费者）

下面这些模块**是** `@tauron/host` 的公开面，但本仓的示例与 SDK 都不调用它们——
它们面向的是**接入方自己的宿主应用**。列在这里是为了让"没有消费者"这件事
可核对，而不是让人误以为它们是断链或死代码（每个模块都有自己的单测）。

| 模块 / 导出 | 干什么 | 对应线上入口 |
|---|---|---|
| `LazyPluginLoader` / `createLazyPluginLoader` | 本地懒加载状态机：`register({ id, entry: () => import(...) })` → 首次 `load()` 才执行工厂，`pending/loading/loaded/failed` 四态 + 失败可 `reset()` 重试 | **纯前端**，不占命令面（加载的是宿主自己的模块） |
| `validateClientConfig` / `fullLoadConfig` / `minimalConfig` / `templateConfig` | 客户端配置的无 IO 校验与三档预置，字段与 Rust `ClientConfig` / `PluginFilter` 镜像 | 无命令：配置在装配期读，运行期由宿主侧生效 |
| `capabilityOfGrantSet` / `scopeGrew` / `requiresReapproval` / `GRANT_SET_SCHEMA_VERSION` | 安装期授予集 → Tauri capability 模板，以及"权限/scope 变多必须重审"的纯策略判定 | 无命令：**安装期**策略库，需接入方显式调用；运行期订阅审批走 `host_events_approve` |
| `toHostRpc(client)` | 把 `HostClient` 适配成传输无关的 `HostRpc`（`stream` / `event` / `subscribe`），换 IPC 后端时业务代码不动（方案 R5） | 复用既有命令，不新增 |
| `MemoryTransport` | 纯进程内 `HostTransport`：给非 Tauri 壳 / 嵌入式宿主 / 测试复用同一套客户端协议 | 无 IPC（命令名仍必须是 `host_*`，构造时校验） |
| `FrameSink` | 流式帧的落地口（`HostClient` 内部与 `MemoryTransport` 共用） | `host_stream_*` |
| `DeepLinkClient` / `createDeepLinkClient` | 注册深链接协议并分发 `deep-link` 事件；OS 回调由 Rust `tauron_adapter::tauri::deliver_deep_link` 双管道投递（Tauri 原生事件 + EventBus） | `host_deep_link_register`（注册）＋ `deep-link` 主题（投递） |
| `WindowState` / `createWindowState` | 窗口几何的**前端**持久化：读写 `localStorage`（键可配，越界钳制后回写），再落到窗口 | `host_window_set_position` / `host_window_set_size` / `host_window_maximize` |

> 判断"是不是断链"的口径：上面每一项的**能力**在线上都有对应命令或是纯本地逻辑；
> 真正没有线上入口的东西会在表里写「无命令」，不会假装接了。

## 开发

```bash
pnpm --filter @tauron/host test         # vitest（全量用例，数量随门禁增长）
pnpm --filter @tauron/host typecheck    # tsc --noEmit
pnpm --filter @tauron/host build        # 产出 dist/
```
