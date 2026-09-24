# Minimal App Example — tauron 应用层贯通示例

可运行的集成示例：宿主主窗 + **iframe 沙箱插件**（握手/调用全协议）+
**45 条 `host_*` 命令族**（窗口/剪贴板/更新…）+ `@tauron/ui` Web Components。

## 运行

```bash
# （仓库根目录）安装 workspace 依赖（本示例是 pnpm workspace 成员）
pnpm install

# 前端构建验证（无需 Tauri 运行时）
pnpm --filter minimal-app build

# 桌面运行（首次需网络下载 tauri-cli；Rust 依赖已可通过 cargo 离线缓存构建）
cd examples/minimal-app
pnpm tauri dev
```

## 项目结构

```
minimal-app/
├── index.html                 # 宿主主窗（入口 src/main.ts）
├── plugin.html                # 沙箱 iframe 插件页（入口 src/plugin/first.ts）
├── vite.config.ts             # 双入口 rollup 配置
├── src/
│   ├── main.ts                # 宿主侧全部接线（4 条链路）
│   └── plugin/first.ts        # registerPlugin 声明式插件
├── src-tauri/
│   ├── src/main.rs            # root 注册 45 条命令 + state_init
│   ├── Cargo.toml
│   ├── build.rs               # tauri-build
│   ├── tauri.conf.json
│   └── icons/icon.ico         # Windows 资源所需占位图标
└── package.json
```

## 四条演示链路

| 链路 | 前端 | Rust |
|---|---|---|
| 沙箱插件 | `PluginBridge.createIframe('plugin.html')` → 握手 token 经 URL hash 注入 → `callPluginMethod(bridge,'format',…)` | 无（postMessage 协议） |
| 窗口控制 | `ShellClient.windowMinimize()` | `host_window_minimize` → `window.minimize()` 真实操作 |
| 系统能力 | `DialogClient.clipboardRead()` / `AutoUpdateClient.checkUpdate()` | `host_clipboard_read`（进程内）/ `host_market_check`（模拟响应，带 `simulated:true`） |
| UI | `<oc-toast>.push(…)` | — |

## 集成要点（读代码前先读这里）

1. **命令注册两种形态，选其一**（重复 `manage::<CommandState>` 会 panic）：
   - **root 注册（本示例，零配置）**：
     `.plugin(tauron_adapter::tauri::state_init())` +
     `.invoke_handler(tauron_adapter::tauron_generate_handler![])`；
     前端 `new TauriBackend({ commandPrefix: '' })`（裸命令名）。
   - **插件注册（生产客户端）**：`.plugin(tauron_adapter::tauri::init())`；
     前端用默认前缀 `plugin:tauron|`。**必须**为 `tauron` 插件配置
     capability/permission 授予所需命令——Tauri v2 对 `plugin:*` 命令强制 ACL，
     未授予会被运行时拒绝（"not allowed by ACL"）。
2. **握手 token 通道**：宿主 `PluginBridge.createIframe` 把 token 注入
   iframe URL hash（`#tauron-token=…`）；插件侧 `registerPlugin` →
   `createPluginContext` 从 hash 读回。插件页必须与真实 bridge 同宿主窗口
   协作（见 `src/plugin/first.ts` 零配置即通）。
3. **深链接（可选生产件）**：接 `tauri-plugin-deep-link` 后，在
   `RunEvent::NewDeepLinkRequest` 回调里调用
   `tauron_adapter::tauri::deliver_deep_link(app, &url)`——双管道投递
   （Tauri 原生事件 + EventBus），`DeepLinkClient` 即可收到。

## Rust 构建说明

`src-tauri` 已从 cargo workspace 显式 `exclude`（独立依赖树）。
所有依赖（tauri 2.11 / tauri-build 2.6 / wry）在本仓库的 cargo 离线缓存内，
`cargo check --offline` 可直接验证。
