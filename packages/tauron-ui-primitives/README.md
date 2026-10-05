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
| 状态层（纯，可离线测试） | `TitleBarStore`、`ToastStore`、`CommandPaletteStore`、`ShortcutRecorderStore`；`UpdaterStore` **未接线**（见下节） |
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

## `<oc-updater-dialog>` 的状态契约（轮 32）

组件是**哑的**：`open` / `version` / `message` / `status` / `progress` 由接入方喂，
「稍后」只收起对话框（派发 `oc-updater-dismiss`，**不是** `oc-close`——那是关窗口）。

`status` 的取值不是本包定的，而是 `@tauron/shell-events` 的 `UPDATER_STATUSES`
（`idle` / `checking` / `available` / `downloading` / `downloaded` / `installing` /
`ready` / `error`）。主按钮**派发什么事件也从这张表推导**（`UPDATER_PRIMARY_ACTION`）：

| status | 主按钮 | 派发 |
|---|---|---|
| `idle` / `checking` / `error` | 检查更新 | `oc-updater-check` |
| `available` / `downloaded` / `downloading` / `installing` | 开始更新 | `oc-update-start` |
| `ready` | **立即重启** | `oc-restart` |

`downloading` / `installing` 额外渲染进度条。为什么把词表与映射都放进契约包：
轮 32 之前写侧（`@tauron/host` 的 `AutoUpdateClient`，安装成功终态 `ready`）与读侧
（本组件，重启分支等 `done`）各用一套字符串，而全仓没人产出 `done`——「立即重启」
在真装配里点不出来，`ready` 反倒显示「开始更新」，点一下把下载+安装重跑一遍。
组件的 `status` 当时是裸 `string`，这个漂移无处显形。今天它是 `UpdaterStatus`，
写错状态是编译错误；词表里也刻意不含 `done` / `updating`。

> **`UpdaterStore`（`updater-dialog.ts`）不是更新流程的状态契约**，且今天没有生产
> 消费者：它自建一套 `…installing → restarting` 状态机，`endpoint` / `retryCount` /
> `maxDownloadSpeed` 三个配置项没有任何代码读过。它已登记在
> `contracts/orphan-public-api.json`（随 v1.0.0 发布过，收口属破坏性变更，需单独批准）。
> 更新流程的权威状态机在 Rust 侧（`tauron-distribute` 的 `UpgradeRunner`），
> 事件契约在本节上面的表里，接线中的 UI 是 `wc-shell.ts` 的 `OcUpdaterDialog`。
