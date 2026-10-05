# @tauron/shell-events

tauron 壳层事件契约：`oc-*` 事件名常量与 detail 类型。**零依赖**。

## 为什么存在

此前 `oc-*` 事件名是**隐式字符串契约**——派发侧写在 UI 组件的模板里，监听侧写在
`@tauron/host` 的 `ShellController` 里，两侧各写各的字面量。后果有先例：

- 「开始更新」(`oc-update-start`)、「立即重启」(`oc-restart`)、插件启用开关
  (`oc-plugin-toggle`) 曾长期**零监听**（死按钮），旧头注还捏造了不存在的
  `oc-updater-install`；
- 反过来 `oc-updater-check` 曾是**零派发的孤儿监听**（更新对话框根本没有
  「检查更新」按钮）。
- 同一类漂移也发生在**状态词表**上（轮 32）：写侧 `AutoUpdateClient` 的安装成功终态
  叫 `ready`，读侧 `<oc-updater-dialog>` 的重启分支却等 `done`，而全仓没有任何生产者
  产出 `done`——「立即重启」在真装配里永远点不出来，`ready` 反倒落进兜底分支显示
  「开始更新」（点一下把下载+安装重跑一遍）。于是本包除事件名外还立了
  `UPDATER_STATUSES` 与 `UPDATER_PRIMARY_ACTION`：**状态取值与「状态 → 主按钮动作」
  映射同为唯一事实源**，写侧 `UpdateStatus` 是它的类型别名，读侧组件的 `status`
  是它的类型。

本包把事件名与 detail 类型收敛为**唯一事实源**：派发方与监听方都从这里 import，
改名/新增在编译期或门禁期暴露，而不是运行时静默断裂。

## 用法

```ts
import { SHELL_EVENTS } from '@tauron/shell-events';

// 派发侧（组件）
this.dispatchEvent(
  new CustomEvent(SHELL_EVENTS.pluginToggle, {
    detail: { id, enabled } satisfies PluginToggleEventDetail,
    bubbles: true,
    composed: true,
  }),
);

// 监听侧（宿主控制器）
el.addEventListener(SHELL_EVENTS.pluginToggle, (e) => { /* ... */ });
```

## 导出

| 导出 | 说明 |
|---|---|
| `SHELL_EVENTS` | 事件名常量表（key → `oc-*` 线名） |
| `SHELL_EVENT_NAMES` | 值域数组（测试/门禁用） |
| `ShellEventName` | 事件名联合类型 |
| `TrayItemEventDetail` / `CommandSelectEventDetail` / `ShortcutChangeEventDetail` / `PluginToggleEventDetail` / `ThemeChangeEventDetail` / `ToastActionEventDetail` | 各事件 detail 类型 |
| `DetailLessShellEvent` | 无 detail 的事件名联合（标题栏三键、检查/开始更新、重启） |
| `UPDATER_STATUSES` | 更新流程状态词表（8 个取值，唯一事实源；不含从未被产出的 `done`/`updating`） |
| `UpdaterStatus` | 上述词表的联合类型；`@tauron/host` 的 `UpdateStatus` 是它的别名 |
| `UPDATER_PRIMARY_ACTION` | `Record<UpdaterStatus, 事件名>`：更新对话框主按钮派发什么（**只有 `ready` → `oc-restart`**） |

## 约束

- **零依赖**：本包不得引入任何运行时依赖（它要被 UI 与宿主两侧同时依赖）。
- 事件名一律 `oc-` 前缀、全小写连字符；自定义事件以
  `bubbles: true, composed: true` 派发（跨 Shadow DOM 边界可达）。
- `packages/tauron-contract-tests` 的 wire-gate 锁死了「组件不得写字面量
  `oc-*` 名」「控制器监听方不得多于派发方」「更新状态词表只有一个事实源且
  `ready` 是唯一能点亮重启按钮的状态」三条不变量。
