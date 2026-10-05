# Minimal App Example — tauron 应用层贯通示例

可运行的集成示例：宿主主窗 + **iframe 沙箱插件**（握手/调用全协议）+
**`host_*` 命令族 83 条**（底座 61 + 插件运行时 22；`plugin-install` 另 2 条是 **opt-in**
——`tauron-adapter` 的 `default = []`，本示例 `src-tauri/Cargo.toml` 的
`default = ["plugin-install", "runtime-wasm-broker"]` 显式开启，故实际注册 **85** 条）
+ `@tauron/ui` Web Components。

## 安装包获取与运行

不想自己构建的话，[Releases](https://github.com/coeasy/tauron/releases) 页有各平台
安装包（Windows NSIS / macOS dmg / Linux deb·rpm·AppImage）。注意两点：

- 推送版本 tag 后，Release 工作流自动构建并创建公开版本；
- 安装包**未签名、未公证**，Windows 会弹 SmartScreen、macOS 会被 Gatekeeper 拦——
  这不是安装包坏了。处理办法与「装完怎么自检」见
  [安装与使用](../../docs/installation.md)。

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

出安装包：`pnpm tauri build`（只想出 NSIS 用 `pnpm tauri build --bundles nsis`）。

## 项目结构

```
minimal-app/
├── index.html                 # 宿主主窗（入口 src/main.ts）
├── plugin.html                # 沙箱 iframe 插件页（legacy 对照，入口 src/plugin/legacy-first.ts）
├── plugin-window.html         # 插件面板窗口页（0.4-A2 主推，入口 src/plugin/first.ts）
├── app-icon.svg               # 图标矢量源（tauri icon 的输入，1024×1024）
├── vite.config.ts             # 三入口 rollup 配置
├── src/
│   ├── main.ts                # 宿主侧全部接线（5 条链路）
│   └── plugin/
│       ├── first.ts           # @tauron/app-plugin-sdk 插件（createPlugin + 执行泵）
│       └── legacy-first.ts    # legacy iframe 插件（deprecated，仅对照）
├── src-tauri/
│   ├── src/main.rs            # root 注册 85 条命令（底座 61 + 运行时 22 + plugin-install 2，install 默认开启）+ state_init
│   ├── Cargo.toml
│   ├── build.rs               # tauri-build
│   ├── tauri.conf.json        # 含 bundle.icon 声明
│   └── icons/                 # 由 app-icon.svg 生成（png / icns / ico + MSIX Logo）
└── package.json
```

图标由矢量源生成，改动后重新生成即可（在 `examples/minimal-app/` 下执行）：

```bash
pnpm dlx @tauri-apps/cli@2 icon app-icon.svg
```

它会覆写 `src-tauri/icons/`，并额外产出 `android/`、`ios/` 两棵移动端目录——
本示例是桌面应用，生成后可删掉这两个目录。`tauri.conf.json` 的 `bundle.icon`
只列桌面端用到的那几个文件。

## 七条演示链路

| 链路 | 前端 | Rust |
|---|---|---|
| 沙箱插件（legacy） | `PluginBridge.createIframe('plugin.html')` → 握手 token 经 URL hash 注入 → `callPluginMethod(bridge,'format',…)` | 无（postMessage 协议） |
| 跨主体调用（0.4-A1/A2 主推） | `ShellClient.windowCreate('com.example.formatter')` 开插件 webview（页面内 `@tauron/app-plugin-sdk` 的 `createPlugin` 注册命令即开执行泵）→ `ShellClient.callPlugin(…)` 投递 → 轮询 `callTakeResult` 取件 | `host_window_create` → `host_call_plugin`（JsCallDelivery 经事件总线投帧）→ `host_call_result`（插件泵回填）→ `host_call_take` |
| 窗口控制 | `ShellClient.windowMinimize()` | `host_window_minimize` → `window.minimize()` 真实操作 |
| 系统能力 | `DialogClient.clipboardRead()` / `AutoUpdateClient.checkUpdate()` | `host_clipboard_read`（进程内）/ 检查走**真通道** `host_updater_check`（必填入参 `currentVersion`，取自 `package.json` 的版本；宿主未注入 `EndpointClient` 时如实返回 `degraded`，UI 不得显示成"已是最新版本"）；下载/安装两条仍是 `host_market_*` 桩（响应带 `simulated:true`，SDK 守卫拒绝推进） |
| UI | `<oc-toast>.push(…)` | — |
| 壳层动作（插件管理 / 命令面板 / 更新对话框） | `new ShellController({backend, currentVersion, onRegistryChange, onUpdaterCheck, onError}).start()` —— `<oc-plugin-manager>` 的开关/卸载/安装、`<oc-command-palette>` 的选中与 `<oc-updater-dialog>` 的「检查更新」**统一**经它路由；命令列表由 `ShellClient.contributesList('command')` 喂，检查结论由 `onUpdaterCheck` 写回对话框的 `version`/`message`/`status`（哑组件自己不做判定；`status` 的取值与主按钮动作都由 `@tauron/shell-events` 定，见轮 32） | `host_registry_admin`（enable/disable/uninstall）→ `host_registry_install_preview` / `host_registry_install`（feature-gated）→ `host_window_create`；命令面板选中 → `host_contributes_list` 找归属 → `host_call_plugin` → `host_call_take`；`oc-updater-check` → `host_updater_check`（`currentVersion` 为必填入参，缺它则**零调用**并经 `onError` 报缺参，不回落桩）→ 答"没有更新 / 答不了"时再读 `host_updater_status`，把灰度批次、崩溃门禁与宿主账本（含 `stateSimulated`：桩推进的账本会写明"模拟推进，未真的装上"）并进 `info.reason` 后由 `describeUpdate()` 打印（轮 33；SDK 那条入口共用同一个口径，两条入口不再一个报依据、一个只报"没有更新"。有更新时**不**多读这条命令）→ 主按钮 `oc-update-start` → `host_market_download`/`host_market_install`（仍是 `simulated` 桩，SDK 守卫拒绝推进）；状态词表与「状态 → 主按钮」映射来自 `@tauron/shell-events`（轮 32）：只有安装真的成功（`status='ready'`）才点亮「立即重启」→ `oc-restart` → `host_window_relaunch` |
| 启动恢复（§4.14） | `ShellClient.recoverReport('success')` 上报"本轮已干净启动"（**漏报 = 每次重启被计一次崩溃**，连续两次进安全模式），并读回阶段决策驱动 UI | `host_recover_report` → `tauron-recovery` 计数器 + 阶段判定（normal / safemode / repairmode）；安全模式下 `host_recover_trial_enable` 逐个试启 |

## 集成要点（读代码前先读这里）

1. **命令注册两种形态，选其一**（重复 `manage::<CommandState>` 会 panic）：
   - **root 注册（本示例，零配置）**：
     `.plugin(tauron_adapter::tauri::state_init_with_adapter_config(...))` +
     `.invoke_handler(tauron_adapter::tauron_generate_handler![])`；
     前端 `new TauriBackend({ commandPrefix: '' })`（裸命令名）。
   - **插件注册（生产客户端）**：`.plugin(tauron_adapter::tauri::init())`；
     前端用默认前缀 `plugin:tauron|`。**必须**为 `tauron` 插件配置
     capability/permission 授予所需命令——Tauri v2 对 `plugin:*` 命令强制 ACL，
     未授予会被运行时拒绝（"not allowed by ACL"）。
2. **握手 token 通道（legacy iframe 链路）**：宿主 `PluginBridge.createIframe`
   把 token 注入 iframe URL hash（`#tauron-token=…`）；插件侧 `registerPlugin` →
   `createPluginContext` 从 hash 读回。插件页必须与真实 bridge 同宿主窗口
   协作（见 `src/plugin/legacy-first.ts` 零配置即通）。
   **0.4-A2 起 主推路径是 `@tauron/app-plugin-sdk`**（`src/plugin/first.ts`）：
   插件跑在 `plugin-<id>` webview 里，经事件总线 + 调用投递与宿主通信，
   无需宿主侧写桥接 handler。
3. **深链接（可选生产件）**：接 `tauri-plugin-deep-link` 后，在
   `RunEvent::NewDeepLinkRequest` 回调里调用
   `tauron_adapter::tauri::deliver_deep_link(app, &url)`——双管道投递
   （Tauri 原生事件 + EventBus），`DeepLinkClient` 即可收到。

## Rust 构建说明

`src-tauri` 已从 cargo workspace 显式 `exclude`（独立依赖树）。
所有依赖（tauri 2.11 / tauri-build 2.6 / wry）在本仓库的 cargo 离线缓存内，
`cargo check --offline` 可直接验证。
