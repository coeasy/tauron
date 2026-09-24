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

## 约束

- **零依赖**：本包不得引入任何运行时依赖（它要被 UI 与宿主两侧同时依赖）。
- 事件名一律 `oc-` 前缀、全小写连字符；自定义事件以
  `bubbles: true, composed: true` 派发（跨 Shadow DOM 边界可达）。
- `packages/tauron-contract-tests` 的 wire-gate 锁死了「组件不得写字面量
  `oc-*` 名」「控制器监听方不得多于派发方」两条不变量。
