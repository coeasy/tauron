# Tauron v1 支持边界

Tauron v1 的官方宿主集成目标是 **Tauri 2 桌面应用**。Tauron 管理插件调用、能力授权和宿主运行时；Tauri 仍负责窗口、WebView、IPC 与应用打包。

## 前端选择

| 前端 | 接入方式 | 边界 |
|---|---|---|
| React | `@tauron/adapter-react` hooks；也可直接使用 `@tauron/host` | 使用同一 Tauri 2 host transport |
| Vue | `@tauron/adapter-vue` composables；也可直接使用 `@tauron/host` | 使用同一 Tauri 2 host transport |
| Svelte | `@tauron/adapter-svelte` stores/actions；也可直接使用 `@tauron/host` | 使用同一 Tauri 2 host transport |
| 原生 TypeScript | 直接使用框架无关的 `@tauron/host` API | 仍需要 Tauri 2 host |

React、Vue、Svelte 是 Web UI 层的选择，不改变 Rust 宿主要求，也不代表 Tauron 为各类桌面运行时提供统一安装器。

## 其他宿主

Electron、普通 Web 页面、原生移动端及其他自定义客户端**不属于 v1 的内置宿主支持范围**。这类宿主若要复用 Tauron 前端 API，必须实现并验证自定义 `HostTransport`/`Backend`，负责把调用、安全主体、权限判定、取消与错误映射到它自己的宿主协议。不能绕过宿主现有的授权边界直接暴露命令。

Tauron 不承诺“一套安装方式覆盖所有客户端”。各宿主仍需使用自身工具链、权限模型和打包方式；`tauron-app new/init` 的自动接入仅针对 Tauri 2。

## 迁移边界

- `tauron-app new` 生成 Tauri 2 工程，支持 React、Vue、Svelte 和 Vanilla TypeScript 前端。
- `tauron-app init` 面向已有 Tauri 2 工程；会保留现有 `invoke_handler`、Tauri capability 和 Tauron client config。已有处理器需要手工合并时，CLI 会报告不完整并返回非零状态。
- 插入的依赖默认固定到 Tauron `1.1.0`；本地 Tauron 源码开发须明确传入 `--tauron-path`。
- 对于已有自定义 host protocol、Capability Broker、租约或其他运行时约束的复杂宿主，应通过独立适配层接入 Tauron；本项目不代替宿主的安全与生命周期治理。

`harness_dock` 可作为复杂宿主边界的架构参考，不能据此推断 Tauron 已兼容它或已实现其专用协议适配器。
