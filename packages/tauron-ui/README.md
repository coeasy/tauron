# @tauron/ui

tauron **UI 便利包**：转出零宿主依赖的 UI 原语，并额外提供需要宿主命令面的
`PluginManagerStore`。

## 定位（R3 依赖反转后）

```
@tauron/shell-events（零依赖契约）
        ↑
@tauron/ui-primitives（Lit peer，零宿主依赖）
        ↑                    ↑
@tauron/host          @tauron/ui ← 本包
```

本包只做两件事：

1. **转出** `@tauron/ui-primitives` 的全部内容（设计令牌、`<oc-*>` 组件、
   各 Store、动效、退出/组件动画）；
2. **补充** `PluginManagerStore`——它调用 `host_registry_list` /
   `host_registry_admin`，因此本包（也只有本包）依赖 `@tauron/host`。

> **底座项目请注意**：若你的宿主不需要「插件管理」这类需要宿主命令面的能力，
> 请**直接依赖 `@tauron/ui-primitives`**，这样不会把 `@tauron/host` 的整个客户端层
> （Backend、ShellClient、能力矩阵、注册表类型）拖进来。

## 安装

```bash
pnpm add @tauron/ui @tauron/ui-primitives lit
```

`@tauron/host` 为运行时依赖（`PluginManagerStore` 需要），由应用层提供。

## 快速上手

```typescript
import { ToastStore, OcToast } from '@tauron/ui';
import '@tauron/ui/wc'; // 注册 <oc-toast> 等自定义元素（转出 ui-primitives/wc）

const store = new ToastStore({ maxItems: 5 });
store.push({ level: 'success', message: '已保存' });

// <oc-toast .store=${store}></oc-toast>
```

## 子路径导出

| 子路径 | 内容 |
|---|---|
| `@tauron/ui` | 全部原语 + `PluginManagerStore` |
| `@tauron/ui/plugin-manager` | 仅 `PluginManagerStore` 及其类型 |
| `@tauron/ui/wc` | 注册自定义元素（转出 `@tauron/ui-primitives/wc`） |
| `@tauron/ui/tokens` | 设计令牌（转出） |
| `@tauron/ui/theme-picker` | 主题选择器（转出） |

## 事件契约

组件派发的业务事件名与 detail 类型**不在本包定义**，而是取自零依赖契约包
`@tauron/shell-events`（`SHELL_EVENTS` 常量 + 各 `*EventDetail` 类型）。

组件与宿主控制器共用同一份常量，因此改名或漏接会在**编译期或门禁期**暴露，
而不是运行时静默断裂（曾有先例：三个按钮长期零监听，`oc-updater-check` 反过来
是零派发的孤儿监听）。所有事件以 `bubbles: true, composed: true` 派发。

## 详细文档

组件清单、Store 职责、动效系统与可访问性说明见
[`@tauron/ui-primitives`](../tauron-ui-primitives/README.md)。

## 相关包

- `@tauron/ui-primitives` — 原语实现（零宿主依赖）
- `@tauron/shell-events` — `oc-*` 事件契约
- `@tauron/host` — 宿主命令客户端（`ShellClient` / `ShellController` / `AdminClient`）
- `@tauron/types` — 共享类型（`TauronConfig.motion` 等）
