# @tauron/ui-primitives

tauron UI 原语：**零宿主依赖**的 Web Components + 设计令牌 + 动效基础设施。

## 定位（R3 依赖反转）

本包是「任意桌面宿主底座」可直接取用的 UI 层。它**不依赖 `@tauron/host`**：

- 哑组件只接受**属性喂数**，只派发 `oc-*` 事件（事件名与 detail 类型来自
  `@tauron/shell-events` 契约）；
- 需要数据时由接入方传入（例如命令面板的 `commands` 属性），组件本身不发命令。

因此一个**只想要标题栏/通知/更新对话框**的宿主项目，取本包即可，不会把宿主
命令面（Backend、ShellClient、能力矩阵、注册表）拖进来。

## 依赖方向

```
@tauron/shell-events（零依赖）
        ↑
@tauron/ui-primitives（Lit peer）
        ↑                    ↑
@tauron/host          @tauron/ui（便利包）
```

- `@tauron/host` 只以**动态** import 可选获取本包的动效/启动画面能力，
  绝不静态引入（保持宿主入口 DOM / `lit` 无关）。
- `@tauron/ui` 是便利包：转出本包，并额外提供需要宿主命令面的
  `PluginManagerStore`。

## 导出

| 类别 | 内容 |
|---|---|
| 设计令牌 | `designTokens`、`safemodeTokens`、`toCssVariables`、`toCssText` |
| 状态层（纯，可离线测试） | `TitleBarStore`、`UpdaterStore`、`ToastStore`、`CommandPaletteStore`、`ShortcutRecorderStore` |
| Web Components | `<oc-toast>`、`<oc-title-bar>`、`<oc-tray-menu>`、`<oc-updater-dialog>`、`<oc-command-palette>`、`<oc-shortcut-recorder>`、`<oc-plugin-manager>`、`<oc-theme-picker>`、`<oc-splash>`、`<oc-skeleton>` |
| 动效 | `motion`（时长/缓动/关键帧）、`wc-motion`（SplashStore）、`animation-hook` |
| 时序工具 | `ExitAnimation`、`animateEnter`/`animateLeave`/`staggerIn`/`staggerOut`/`animateListUpdate` |

> `ExitAnimation` 与组件动画自 `@tauron/host` 迁入（R3）：它们是纯 DOM 时序工具，
> 不是宿主能力。`WindowState` **留在** `@tauron/host`——它包装 `Backend` 的窗口
> 命令，是货真价实的宿主能力。

## 注册自定义元素

```ts
import '@tauron/ui-primitives/wc';        // oc-toast
import '@tauron/ui-primitives';           // 或经入口（含壳组件注册）
```

兼容入口：`@tauron/ui/wc`（转出本包，供既有应用使用）。

## 子路径导出

`@tauron/ui-primitives`、`@tauron/ui-primitives/tokens`、
`@tauron/ui-primitives/wc`、`@tauron/ui-primitives/theme-picker`
