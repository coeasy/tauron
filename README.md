# tauron

> Tauri 2 之上的插件化桌面客户端基础设施 —— 插件隔离、受控能力面、跨语言契约与多形态插件执行。

[![CI](https://github.com/coeasy/tauron/actions/workflows/ci.yml/badge.svg)](https://github.com/coeasy/tauron/actions/workflows/ci.yml)
[![Tests](https://img.shields.io/badge/tests-1759%20TS%20%C2%B7%201426%20Rust-informational)](#测试)
[![License](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)
[![Node](https://img.shields.io/badge/Node-22.x-brightgreen)](#)
[![Rust](https://img.shields.io/badge/Rust-1.98-orange)](#)

---

## 这是什么

tauron 是**跑在 Tauri 2 之上的插件化桌面客户端基础设施**。它解决的不是「怎么画界面」，
而是「一个桌面客户端要怎么安全地加载第三方代码、把它们互相隔离、给它们受控的能力，
并且能在不重新发版的前提下换掉它们」。

它由两层组成，**可以分开取用**：

| 层 | 面向 | 拿到什么 |
|---|---|---|
| **框架层** | 通用集成 | 信封协议 `plugin_invoke`、`PluginType` 四形态（Js / Process 有生产执行器；Rust / Wasm 诚实返回 `E_PLUGIN_TYPE_NO_RUNTIME`，代码里**不存在**「B+ 混合模式」）、双层 ACL、事件总线、插件市场（Ed25519 验签）、CLI。**诚实边界**：`@tauron/dual-world` 的进程内沙箱是 fail-closed 模拟（`SANDBOX_UNAVAILABLE`），进程内 WASM 运行时仍为路线图项；`plugin_invoke` 的**执行体**（`PluginDispatcher`）仓库内只有测试实现（`EchoDispatcher`），宿主不装载自己的 dispatcher 就只会拿到 `SC-9001` |
| **应用层** | 完整客户端交付 | `host_*` 命令族（83 条 = 底座 61 + 插件运行时 22；`plugin-install` 另加 2 条，该 feature 是 **opt-in**（`crates/tauron-adapter/Cargo.toml` 的 `default = []`，V4 minimal-substrate 规则）→ 显式开启后共 85 条；示例应用已开启，故其装配为 85 条）、生命周期状态机、三档授权、设置中心、白标、主题、菜单/托盘、允许根内的文件 I/O、更新通道、崩溃恢复、Event 审批与生产就绪自检（1.1）、UI 组件 |

### 它不是什么

把边界说清楚比列特性更有用：

- **不是成品应用**。仓库里**唯一能装**的是 `examples/minimal-app` 示例应用，
  用来演示链路，不是产品形态的客户端。
- **不是 Tauri 的替代品**。它建在 Tauri 2 之上——Tauri 管窗口 / WebView / IPC，
  tauron 管其上的插件运行时与能力治理。
- **可通过 registry 安装的 SDK**。20 个公开 npm 包与 15 个 Rust crate 可从公共 registry 安装
  （**registry 上的 `latest` 目前是 `1.0.2`**；本仓库的 `1.1.0` 已通过发布内容校验，
  但尚未执行发布）。计数别搞混：`crates/` 下有 **16** 个成员，第 16 个 `tauron-ffi`
  **从未发布过**（1.0.2 时代漏发，不是豁免——它没有 `publish = false`，且在
  `publish:crates` 的发布集合里），所以 registry 上只有 15 个可装。具体接入方式见
  [安装与使用](./docs/installation.md)。
- **当前版本为 1.1.0**。公开 API 遵循语义化版本；具体未接入的运行时与平台能力见下方成熟度说明。

### 成熟度：哪些是真的，哪些还是占位

本项目刻意区分「真实实现」与「接口占位」，用 `simulated` 字段与诚实失败码表达，
**不伪造成功**。判断某个能力到底能不能用，看这三处（不要只看本页的特性表）：

- [架构概览的「接线状态（诚实披露）」](./docs/architecture/overview.md)
- [竞品分析的兑现度标记](./docs/competitive-analysis/competitive-analysis.md)（带核对日期）
- [CHANGELOG 的「已知债务」](./CHANGELOG.md)

一句话概览：**信封协议、命令层、双层 ACL、注册表、事件总线、生命周期状态机、设置
中心、崩溃恢复、i18n、通知、白标/主题是真实实现；QuickJS-WASM 引擎接入、Shell 矩阵
的运行时、Process 插件的执行体、更新下载链路是接口占位或模拟原型。**

### 现在适合拿它做什么

| 场景 | 适不适合 |
|---|---|
| 给已有 Tauri 应用加一套插件系统 | ⚠️ 分两种：**要现成能跑**→ 接应用层（`host_*`，见三档装配）；**只要信封协议**→ 接框架层，但你必须自己实现并装载 `PluginDispatcher`——仓库内没有生产实现，未装载时 `plugin_invoke` 恒回 `SC-9001` |
| 做一个要装第三方插件的桌面客户端 | ✅ 接应用层，按「三档装配」选档 |
| 需要一个带设置中心 / 白标 / 崩溃恢复的客户端底座 | ✅ 接应用层 |
| 想要开箱即用的成品桌面应用 | ❌ 这是框架，没有成品 |
| 想要 `pnpm add` 就能用的稳定 SDK | ✅ Tauron SDK v1.1.0 提供 npm 与 crates.io 安装 |

**准备上手**：[安装与使用](./docs/installation.md) —— 三种「安装」怎么选、
各平台安装步骤、从源码构建、装完怎么自检。

---

## 特性

| 特性 | 说明 |
|---|---|
| **插件信封协议** | 单一 `plugin_invoke` 命令，统一调用/取消/进度 |
| **进程内沙箱（dual-world）** | 架构与接口已定义，运行时为 **fail-closed 模拟**（`SANDBOX_UNAVAILABLE`）；QuickJS-WASM 引擎接入为路线图项 |
| **四种插件形态（`PluginType`）** | Js / Process **有生产执行器**；Rust / Wasm 诚实返回 `E_PLUGIN_TYPE_NO_RUNTIME`。代码里**不存在**「B+ 混合模式」——该宣称已删除 |
| **双层 ACL** | 外层 Tauri 静态 ACL + 内层框架动态 ACL |
| **事件总线** | 每插件队列隔离，背压可配置，命名空间隔离 |
| **配置 4 层合并** | session > plugin > user > default |
| **Shell 矩阵** | local / local-server / remote-url / sub-webview（接口已定义，运行时为模拟原型） |
| **插件市场** | **Ed25519** 签名（非对称：私钥签发、公钥验证），注册表搜索/发布（宿主侧 `host_market_*` 目前是桩，见上文成熟度） |
| **CLI 工具链** | `create` / `plugin new` 真落盘；`doctor` 真探测环境；`plugin sign` 写真实 SHA-256 摘要（如实标 `simulated:true`，非签名）；`plugin dev/test/pack/publish` **未实现**且如实返回 `success:false`——不谎报成功 |
| **UI 适配层** | React / Vue / Svelte hooks 和 composables |
| **契约测试** | TS↔Rust 跨语言协议验证 |

---

## 快速开始（第三方集成）

### 0. 一键脚手架（推荐起点）

**能一键跑通到哪一步**——先把边界说清，避免期待错位：

| 产出 | 状态 |
|---|---|
| `src-tauri/`（`Cargo.toml` / `main.rs` / `build.rs` / `capabilities/default.json` / `tauri.conf.json`） | ✅ **真装配**：`state_init_with_adapter_config` + `tauron_generate_handler![]`（85 条）+ 窗口销毁回收 + capability 覆盖 `plugin-*` 窗。形态与 `examples/minimal-app` 同源 |
| 依赖坐标 | ⚠️ 固定到**本 CLI 的版本**（`src/framework-version.ts`，仓库源码为 `1.1.0`，npm 上装到的 CLI 是 `1.0.2`）；1.1.0 尚未发布，所以从源码生成的工程暂时装不到 registry，源码开发请用 `--tauron-path` |
| 前端 bundler / dev-server 配置 | ✅ `vite.config.ts`（`server.port` 与 `devUrl` 一致、`outDir` 与 `frontendDist` 一致、排除 `src-tauri/`）+ 根 `index.html` + `tauri.conf.json` 的 `beforeDevCommand` / `beforeBuildCommand` |
| `src-tauri/icons/` | ⚠️ 生成**纯色占位图**（`32x32.png` / `128x128.png` / `128x128@2x.png` / `icon.png` / `icon.ico` / `icon.icns`）——**发布前须替换成品牌图标**。不给文件连 `cargo check` 都过不去：`tauri-build` 在 Windows 上要 `icons/icon.ico` 才能生成资源文件 |

> **实测（2026-09-27）**：生成的工程（含占位图标与 vite 配置）开箱即可编译——默认形态
> 与 `--features substrate-only` 两档 `cargo check` **均通过**（`Finished dev profile`）。
>
> **公共 registry 消费验收**已覆盖脚手架创建、前端依赖安装与构建、Rust adapter 安装及 Tauri 2 项目编译。
> 本机 `npm run tauri dev` 仍需要安装系统 WebView 与 Tauri CLI。另有两条源码开发前置：
> ① `file:` 依赖是**软链**，所以本机 tauron 检出必须已 `pnpm install`（供 `@tauron/host`
> 解析它自己的 `workspace:*` 依赖）；② `packages/*/dist` 必须是已构建状态（`pnpm -r build`）。

```bash
# 公共 registry（latest 现为 1.0.2；别点名 @1.1.0，它尚未发布，会 ETARGET）：
npm create tauron-app@latest -- ./my-app --framework react

# 在 Tauron 仓库内开发时，显式启用本地源码依赖：
node packages/tauron-app-cli/dist/cli.js new ./my-app --tauron-path ..

# 一步到位：
cd my-app && npm install && npm run tauri dev

# 或先单独验证「真接上了 tauron」：
cd my-app/src-tauri && cargo check                          # 85 条命令
cargo check --features substrate-only                       # 只取底座：61 条命令
```

常用参数：`--framework react|vue|svelte|vanilla`、`--shell tauri|electron`、
`--capabilities host_registry_list,host_lifecycle_report`、`--tauron-path <检出根相对路径>`
（显式切换为本地源码）、`--dry-run`、`--force`。

**已有 Tauri 2 项目**改用注入式接入：`tauron-app init --dir <你的项目>`——它会加固定
`1.1.0` 依赖、接线 Builder 链，并保留现有权限与 client config；若你原有 `.invoke_handler(..)`
已存在，它会保留该处理器、报告需要手工合并的步骤，并以非零状态结束。

v1 官方宿主范围为 Tauri 2；React、Vue、Svelte 与原生 TypeScript 是前端选择。其他客户端宿主
须实现自己的 `Backend`/`HostTransport` 适配并保留宿主授权边界。详情见[支持边界](./docs/integration/support-boundary.md)。

### 1. 安装依赖

> **SDK 可从公共 registry 安装**：20 个公开 npm 包与 15 个 Rust crate（registry 现值
> **`1.0.2`**；本仓库的 `1.1.0` 已备好但尚未发布，点名 `@1.1.0` 现在装不到）；另有
> 1 个仅供仓库内部测试的私有 npm 包。下面给出最小依赖示例：

```bash
# 前端：选择需要的入口包，包管理器会安装其传递依赖
pnpm add @tauron/host @tauron/ui
```

```toml
# src-tauri/Cargo.toml
[dependencies]
tauron-adapter = { version = "=1.1.0", features = ["tauri"] }
```

本地源码开发可使用 workspace/path 依赖；发布包用户无需手工安装内部依赖。

### 2. 注册 Rust 命令层

`tauron-shell` 默认**不依赖 `tauri`**，命令注册层由 `tauri` feature 提供：

```toml
# src-tauri/Cargo.toml
[dependencies]
tauron-shell = { version = "=1.1.0", features = ["tauri"] }
```

```rust
// src-tauri/src/main.rs
fn main() {
    tauri::Builder::default()
        // 状态 + 事件出口（不注册命令）
        .plugin(tauron_shell::commands::state_init())
        // root 注册 plugin_invoke / plugin_cancel / plugin_emit
        //（裸命令名，与 @tauron/core createTauriBackend() 直接匹配；
        //  **不是「零 capability」**：Tauri v2 下不匹配任何 capability 的 webview 完全没有
        //  IPC 访问，所以任何形态都至少需要一份 capability 文件，示例已随仓提供。
        //  若改用 .plugin(tauron_shell::commands::init()) 插件形态，
        //  则必须为 `tauron-shell` 插件配置 capability——Tauri v2 对 plugin:* 命令强制 ACL）
        .invoke_handler(tauron_shell::tauron_generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

> ⚠️ 上面这段能注册命令，但**跑不通一次真实调用**：框架层的信封需要
> `PluginDispatcher` 的生产实现，仓库内只有测试用的 `EchoDispatcher`。
> 未装载分发器时 `plugin_invoke` 一律返回 `SC-9001`（`tauron-shell` 的
> `dispatch.rs` 明写「装载插件分发器。未装载时 `plugin_invoke` 返回 `SC-9001`」）。
> 想要**装完就能用**的插件系统，请走应用层（下一节 `tauron_adapter::tauron_generate_handler![]`）。

### 3. 初始化前端运行时

```typescript
// src/main.ts
import { createTauriBackend, invokePlugin } from '@tauron/core';

const backend = createTauriBackend();

// 调用插件
const result = await invokePlugin(
  backend,
  'com.example.formatter',  // 插件 ID
  'format',                 // 方法名
  { code: '  let  x = 1; ' } // 参数
);

if (result.ok) {
  console.log(result.result);  // { formatted: 'let x = 1;' }
}
```

### 4. 订阅事件

```typescript
import { listenEvent } from '@tauron/core';

const unlisten = await listenEvent(
  backend,
  'plugin:com.example.formatter:data-changed',
  (payload) => console.log('Event:', payload)
);

// 取消订阅
unlisten();
```

> **调用链**：`invokePlugin()` → `createTauriBackend()` → Tauri IPC `plugin_invoke`
> → `tauron_shell::HostState::handle_invoke()` → 注册表状态检查 → `PluginDispatcher`。
> 命令名、camelCase 字段与错误码由 `@tauron/contract-tests` 的跨语言门禁锁定。

---

## UI 框架集成

### React

```tsx
import { TauronProvider, useInvoke, useEvent } from '@tauron/adapter-react';

function App() {
  return (
    <TauronProvider backend={backend}>
      <Formatter />
    </TauronProvider>
  );
}

function Formatter() {
  const { state, invoke } = useInvoke('com.example.formatter');
  
  return (
    <button onClick={() => invoke('format', { code: '...' })}>
      {state.loading ? 'Formatting...' : 'Format'}
    </button>
  );
}
```

### Vue

```vue
<script setup>
import { useInvoke, useEvent } from '@tauron/adapter-vue';

const { state, invoke } = useInvoke('com.example.formatter');
</script>
```

### Svelte

```svelte
<script>
import { createInvokeStore } from '@tauron/adapter-svelte';
const { subscribe, invoke } = createInvokeStore('com.example.formatter');
</script>
```

---

## 插件开发

### 创建插件

```bash
# tauron = node packages/tauron-cli/bin/tauron.js（npm 上 @tauron/cli 的 latest 是 1.0.2，落后于仓库源码）
tauron plugin new my-plugin --type js
```

### 插件入口 (JS)

```javascript
import { registerPlugin } from '@tauron/plugin-sdk';

registerPlugin({
  name: 'my-plugin',
  version: '1.0.0',
  methods: {
    async format({ args, ctx }) {
      return { formatted: args.code.replace(/\s+/g, ' ') };
    },
  },
  events: {
    'data-changed': ({ payload }) => payload,
  },
  onEnable(ctx) { console.log('Plugin enabled'); },
});
```

### 进程插件 (Process)

```javascript
const { createServer } = require('net');

const server = createServer((socket) => {
  socket.on('data', (data) => {
    const request = JSON.parse(data.toString());
    const response = handleRequest(request);
    socket.write(JSON.stringify(response) + '\n');
  });
});
```

---

## 配置

```typescript
import type { TauronConfig } from '@tauron/types';

const config: TauronConfig = {
  capabilities: ['base', 'store', 'network'],
  plugins: {
    local: './plugins',
    registry: 'https://registry.tauron.dev',
    autoUpdate: true,
  },
  shell: {
    type: 'local',
    distDir: './dist',
  },
  updater: {
    endpoints: ['https://updates.tauron.dev'],
    pubkey: 'public-key-hex',
  },
  brands: {
    default: './config/brand.json',
  },
  i18n: {
    defaultLocale: 'zh-CN',
    supported: ['zh-CN', 'en-US', 'ja-JP'],
  },
};
```

---

## Shell 形态

| 形态 | 场景 | 配置 |
|---|---|---|
| `local` | 本地文件加载 | `{ form: 'local', entryPath: '/path/to/plugin.html' }` |
| `local-server` | 本地 HTTP 服务 | `{ form: 'local-server', host: 'localhost', port: 8080 }` |
| `remote-url` | 远程加载 | `{ form: 'remote-url', url: 'https://cdn.example.com/plugin' }` |
| `sub-webview` | 子窗口隔离 | `{ form: 'sub-webview', webviewUrl: 'https://...', devtools: true }` |

```typescript
import { createShellManager } from '@tauron/shell-matrix';

const shell = createShellManager();
const instance = shell.create({
  pluginId: 'com.example.plugin',
  form: 'local-server',
  host: 'localhost',
  port: 8080,
});
await instance.start();
```

> **这段代码现在只做状态编排，不拉起真实容器**：`shell-matrix` 的实例恒带
> `simulated: true`（`src/manager.ts`），`start()` 只把状态机推到 `ready`，
> **不会**起本地 HTTP 服务、不会开子 webview、不会加载远程 URL。所以
> `status: 'ready'` 不等于"插件已经跑起来"——判据是那个 `simulated` 标志，
> 细节见 `packages/tauron-shell-matrix/README.md`。本仓里真正加载插件 UI 的路径是
> 宿主的 `host_window_create`（`plugin-*` 标签走安装目录资产协议）与
> `@tauron/plugin-sdk` 的 `PluginBridge`。

---

## 错误处理

```typescript
import { PluginErrorCode, isRetryable } from '@tauron/types';

try {
  const result = await invokePlugin(backend, pluginId, method, payload);
  
  if (!result.ok) {
    if (isRetryable(result.error!.code)) {
      // 可重试：TIMEOUT, CHANNEL_BROKEN, INTERNAL
      await retry();
    } else {
      throw new Error(`${result.error!.code}: ${result.error!.message}`);
    }
  }
} catch (err) {
  console.error(err);
}
```

### 错误码表

| 代码 | 类别 | 说明 | 可重试 |
|---|---|---|---|
| SC-0001 | 插件系统 | PluginNotFound | ❌ |
| SC-0002 | 插件系统 | PluginDisabled | ❌ |
| SC-0003 | 插件系统 | PluginErrored | ❌ |
| SC-1001 | 权限 | PermissionDenied | ❌ |
| SC-1002 | 权限 | PluginPermissionDenied | ❌ |
| SC-1003 | 权限 | CapabilityRequired | ❌ |
| SC-2001 | 通信 | Timeout | ✅ |
| SC-2002 | 通信 | Cancelled | ❌ |
| SC-2003 | 通信 | InvalidPayload | ❌ |
| SC-2004 | 通信 | ChannelBroken | ✅ |
| SC-3001 | 实现 | PluginPanic | ❌ |
| SC-3002 | 实现 | PluginOom | ❌ |
| SC-3003 | 实现 | PluginExited | ❌ |
| SC-9001 | 系统 | Internal | ✅ |

---

## 权限管理

```typescript
import { checkPluginPermission, grantPermissions } from '@tauron/core';

// 授予权限
grantPermissions(grants, 'com.example.plugin', [
  'store:read',
  'http:fetch',
  'fs:read-app-dirs',
], 'install');

// 检查权限
const error = checkPluginPermission(
  'com.example.plugin',
  'format',
  ['store:read', 'http:fetch'],
  grants,
);
```

---

## CLI 工具

> ⚠️ **registry 现状（2026-10-02 实测）**：npm 上 `@tauron/cli` 的 `latest` 是 **1.0.2**——
> 本仓库的 `1.1.0` 已通过 `pnpm publish:npm -- --check`，但**还没有 `--publish`**，
> 所以 `npx @tauron/cli` 拿到的是 1.0.2 的行为，与当前代码不一定一致。
> 下文用 `tauron` 代指 `node packages/tauron-cli/bin/tauron.js`（在仓库根执行），
> 以本仓库代码为准。

```bash
# 环境诊断
tauron doctor

# 创建应用（真的落盘；目录已存在需 --force）
tauron create my-app

# 插件开发（真的落盘；--type 支持空格与等号两种写法）
tauron plugin new com.example.myplugin --type js
tauron plugin new com.example.calc     --type wasm

# 以下四条**未实现**，会如实返回 success: false（不谎报、不编造结果）
tauron plugin dev      # 未实现：不启动 dev server
tauron plugin test     # 未实现：不执行测试、不编造通过数
tauron plugin pack     # 未实现：不产出归档
tauron plugin publish  # 未实现：不上传、不编造商城地址

# 已实现：算真实 SHA-256 内容摘要并写出 .sig
tauron plugin sign
```

各命令的真实边界见 [插件开发指南](./docs/api/plugin-development-guide.md)。

---

## 项目结构

```
tauron/
├── crates/                          # Rust 工作区
│   ├── tauron-shell/                # ★ 框架壳层：信封/命令层/ACL/注册表/事件总线/配置
│   ├── tauron-host/                 # 应用宿主核心（注册表/生命周期/授权/清单）
│   ├── tauron-adapter/              # Tauri 命令适配层（host_* 命令族，feature = "tauri"）
│   ├── tauron-acl/                  # 权限授予与审批
│   ├── tauron-schema/               # Schema 规范化管线
│   ├── tauron-settings/             # 设置中心（四层合并）
│   ├── tauron-market/               # 插件商城
│   ├── tauron-brand/                # 白标品牌化
│   ├── tauron-theme/                # 主题 / 皮肤
│   ├── tauron-i18n/                 # 国际化
│   ├── tauron-notify/               # 通知中心
│   ├── tauron-recovery/             # 崩溃恢复
│   ├── tauron-distribute/           # CI 分发运维
│   ├── tauron-proc/                 # 进程插件 Host
│   ├── tauron-wasm/                 # WASM Supervisor
│   ├── tauron-test-sidecar/         # CI 真 sidecar 夹具（`publish = false`，永不上架）：
│   │                                #   可执行 stdio 帧回路端，仅供 tauron-proc 的端到端测试起真进程
│   └── tauron-ffi/                  # 稳定 C ABI 所有权边界（供**非 Rust 宿主**接入 Universal Wire；
│                                    #   仓内没有 Rust 消费者是设计使然，契约见 docs/contracts/ffi-v1.md）
├── packages/                        # npm 包（21 个目录：20 公开 + 私有 `tauron-contract-tests`；下面按层分组）
│   # ── 框架层（12 个）──
│   ├── types/                       # @tauron/types — 信封/错误码/ACL/Manifest/事件
│   ├── tauron-core/                 # @tauron/core — invoke/backend/registry/event-bus/acl/config
│   ├── tauron-plugin-sdk/           # @tauron/plugin-sdk — bridge/context
│   ├── tauron-plugin-context-contract/  # @tauron/plugin-context-contract — 两套 PluginContext 的共享契约
│   ├── tauron-dual-world/           # @tauron/dual-world — QuickJS-WASM 双世界沙箱
│   ├── tauron-adapter-react/        # @tauron/adapter-react — TauronProvider/useInvoke/useEvent
│   ├── tauron-adapter-vue/          # @tauron/adapter-vue — composables
│   ├── tauron-adapter-svelte/       # @tauron/adapter-svelte — stores
│   ├── tauron-cli/                  # @tauron/cli — 插件工具链（bin: tauron）
│   ├── tauron-market/               # @tauron/market — 签名/注册表
│   ├── tauron-shell-matrix/         # @tauron/shell-matrix — 4 种 shell 形态
│   ├── tauron-contract-tests/       # @tauron/contract-tests — TS↔Rust 跨语言门禁
│   # ── 应用层（9 个）──
│   ├── tauron-host/                 # @tauron/host — 能力编排层（HostClient/TauriBackend）
│   ├── tauron-shell-events/         # @tauron/shell-events — 壳层事件名常量（零依赖）
│   ├── tauron-ui-primitives/        # @tauron/ui-primitives — UI 原语（零宿主依赖）
│   ├── tauron-app-cli/              # @tauron/app-cli — 应用脚手架（bin: tauron-app）
│   ├── create-tauron-app/           # create-tauron-app — `npm create` 入口（未 scoped；与 `tauron-app new` 同一实现）
│   ├── tauron-app-plugin-sdk/       # @tauron/app-plugin-sdk — 应用级插件 SDK
│   ├── tauron-app-contract-kit/     # @tauron/app-contract-kit — 应用契约套件
│   ├── tauron-framework/            # @tauron/framework — 框架薄封装
│   └── tauron-ui/                   # @tauron/ui — Lit Web Components
├── examples/
│   └── minimal-app/                 # 最小应用示例（仓库里唯一可打包运行的应用）
└── docs/
    ├── installation.md              # ★ 安装与使用（落地入口：怎么装 / 怎么跑 / 怎么自检）
    ├── architecture/                # 权威架构说明 / 线格式协议 / canonical 归属 / 0.3 方案
    ├── integration/                 # 渐进接入指南（三档装配）
    ├── api/                         # 插件开发指南
    └── competitive-analysis/        # 竞品分析（带兑现度标记）
```

> **两层架构说明**
>
> - **框架层**（`@tauron/types` / `core` / `plugin-sdk` / `adapter-*` + `tauron-shell`）
>   面向通用集成：信封协议 `plugin_invoke`、`PluginType` 四形态、插件市场（Ed25519 验签）。
>   第三方客户端从这一层接入。
>   ⚠️ **注意**：这一层目前**没有生产装配**——`PluginDispatcher` 仍是零生产实现，
>   `plugin_invoke` 三命令无接线（详见架构概览的接线状态表）。
> - **应用层**（`@tauron/host` / `app-*` / `framework` / `ui` + `tauron-host` / `tauron-adapter`）
>   面向完整客户端交付：`host_*` 命令族、生命周期、设置中心、白标。
>   它复用框架层的类型与协议，但不改变框架层契约。

---

## 测试

### 用例数

| 侧 | 用例数 | 口径 |
|---|---|---|
| TypeScript | **1759**（104 个测试文件 / 20 包，0 failed） | `pnpm -r test` 实跑通过（2026-10-02 本机，轮 9 收尾那次运行） |
| Rust | **1426 passed / 0 failed**（21 个测试二进制） | `cargo test --workspace --locked --lib --tests` 实跑（2026-10-02 本机，默认特性，exit 0）；源码 `#[test]` **声明数** 1498 |
| 跨语言契约 | **130** | `@tauron/contract-tests` 的 wire-gate（`vitest run src/wire-gate.test.ts` 实跑）；本包合计 151 条 / 2 个文件 |

> **关于 Rust 一栏的口径**：表中给的是**执行结果**（默认特性）。Rust 侧另有
> **feature 门控**用例（`tauron-adapter` / `tauron-shell` 的 `tauri`、
> `tauron-adapter` 的 `plugin-install`），所以要高于默认特性执行数——源码
> `#[test]` 声明数 **1498** 就是这个差额的来源。feature 矩阵的执行结果以 CI 的
> `Rust (tauri feature)` / `Rust (minimal substrate)` 两个 job 为准。

### 本地命令

```bash
# 安装依赖
pnpm install

# ── TypeScript ───────────────────────────────
# ⚠️ 顺序不可换：测试从各包的 dist/ 解析 workspace 导入，
#    不先 build 会得到一批 "Failed to resolve import" 的假失败。
pnpm -r build
pnpm -r --no-bail typecheck
pnpm lint             # ESLint（0 error / 0 warning）
pnpm -r --no-bail test

# 等价的一键命令（build → typecheck → test）
pnpm verify

# ── Rust ─────────────────────────────────────
cargo test --workspace                  # 默认特性：不依赖 tauri，无需系统库
cargo test -p tauron-shell   --features tauri
cargo test -p tauron-adapter --features tauri

# 格式化与 lint（硬门禁）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

---

## 架构设计

```
┌─────────────────────────────────────────────────────────────┐
│                     前端应用层                                 │
│   React / Vue / Svelte / 原生 TS                              │
├─────────────────────────────────────────────────────────────┤
│                     适配层 (@tauron/adapter-*)                 │
│   useInvoke / useEvent / store / composables                 │
├─────────────────────────────────────────────────────────────┤
│                     核心层 (@tauron/core)                      │
│   invokePlugin / EventBus / ACL / Registry / Config           │
├─────────────────────────────────────────────────────────────┤
│                     类型层 (@tauron/types)                     │
│   信封 / 错误码 / ACL / Manifest / 事件                        │
├─────────────────────────────────────────────────────────────┤
│                     Rust 壳层 (tauron-shell)                   │
│   信封处理 / 错误映射 / 权限检查 / 状态机 / 配置合并           │
└─────────────────────────────────────────────────────────────┘
```

---

## 文档

| 文档 | 说明 |
|---|---|
| [安装与使用](./docs/installation.md) | 三种「安装」辨析、各平台安装示例应用、从源码构建、接入自己项目、装完自检、已知限制与 FAQ |
| [架构概览](./docs/architecture/overview.md) | 整体架构图、两层架构、包命名体系、模块依赖、关键设计决策、架构演进 |
| [应用层线格式协议](./docs/architecture/app-layer-wire.md) | **关键** `host_*` 命令的参数 / 返回 / 生命周期事件 / 能力档位规范与限制登记 |
| [命令面全量参考](./docs/api/command-surface.md) | 85 条 `host_*` 的业务形参 / 返回 / 档位与判定 / feature 门 / 前端落点——**生成物**，`pnpm command-surface:gen` 写、CI `command-surface:check` 复算 |
| [canonical 归属](./docs/architecture/canonical-owners.md) | 唯一事实源与已冻结的 legacy 门面（改这张表等于改架构） |
| [0.3 优化改进方案](./docs/architecture/multi-plugin-substrate-roadmap.md) | 多插件框架 × 任意宿主底座：成熟度记分卡、残差清单、S/M/X 改进项与轮次编排 |
| [V4 工业级缺口收口方案](./docs/architecture/v4-industrial-gap-closure-plan.md) | 逐条对照 V4 §134/§135/§136/§141：A01–A110 的已落 / 部分 / 未落台账、F1–F5 危险缺口、Batch 0–6' 编排与推迟清单 |
| [V5 架构竞分析与优化方案](./docs/Tauron-Architecture-Competitive-Analysis-Optimization-Plan-V5.md) | 七 Plane 目标架构、`RuntimeDriver` / HostProtocol / Manifest V3 契约、Phase A–G 优先级与退出门槛、能力诚实分级——**轮 13 起的对照基线**（前瞻方案，不描述现状） |
| [渐进接入指南](./docs/integration/incremental-adoption.md) | 三档装配：只取底座 / 底座 + 插件运行时 / 完整客户端 |
| [插件开发指南](./docs/api/plugin-development-guide.md) | 创建、测试、打包、发布插件（含 CLI 各命令的真实边界） |
| [客户端配置参考](./docs/api/client-config.md) | `ClientConfig` 的 10 个键：哪些有宿主落点、哪些写了不生效及原因、加载路径与校验规则 |
| [竞品分析](./docs/competitive-analysis/competitive-analysis.md) | 竞品全景图、功能对比矩阵、头条特性兑现度标记 |
| [示例应用](./examples/minimal-app/README.md) | 最小集成示例 |
| [CHANGELOG](./CHANGELOG.md) | 版本变更记录与「已知债务」清单 |

> **V4 进度的一句话口径**（可核查，改代码时同轮改这里，别凭印象写）：A64–A110 共 47 项
> —— **已落 22 / 部分 21 / 未落 4**；§134 的 DoD 表逐行数得 **✅ 16 / ⚠️ 15 / ❌ 1**。
> 逐条证据、门禁输出与「本轮未做」清单都在
> [V4 工业级缺口收口方案](./docs/architecture/v4-industrial-gap-closure-plan.md)。
> 对外表述维持 **「较强的 Tauri-first substrate 基础 + 完整的 Universal/Industrial 演进设计」**，
> 不出现 industrial-grade。

---

## 持续集成与发布

| 工作流 | 触发 | 做什么 |
|---|---|---|
| [`ci.yml`](./.github/workflows/ci.yml) | `main` push / PR | 版本号一致性（`version:check`）→ TS（build→typecheck→lint→test）、Rust 默认特性、Rust `tauri` feature、wire-gate |
| [`release.yml`](./.github/workflows/release.yml) | 推 `v*` tag | 版本号一致性校验 → Windows / macOS（arm64 + x64）/ Linux 安装包构建 → 公开 Release |

发布流程：

```bash
# 1. 一条命令写齐全部版本号落点（根 package.json 是唯一事实源 → Cargo workspace、
#    packages/* 的每个 package.json、示例应用的 package.json、它的 src-tauri
#    Cargo.toml 与 tauri.conf.json）：
pnpm version:sync 1.2.0
pnpm version:check     # 同一判定已进 CI；漏改任何一处都会在这里红，而不是在 Release 里
# 2. 更新 CHANGELOG.md 并提交。tag 号必须等于根版本号——release.yml 的 version-check
#    会把它与上述落点逐一比对（此前这一步只有校验器，没有生产者）
git tag v1.2.0 && git push origin v1.2.0
```

> `cargo fmt`、`cargo clippy`、ESLint 和应用示例装配均为硬门禁。`cargo-deny` 自轮 9
> 复核起也是**阻断式硬门禁**（`ci.yml` 的 `deny` 作业跑 `command: check --all-features`，
> 无 `continue-on-error`）；任何例外必须在 `deny.toml` 显式登记并附审查说明。
> 唯一还不在 CI 里的强制项是 **`main` 分支保护**——它是仓库设置，需人工开启，
> 详见 `docs/architecture/v4-industrial-gap-closure-plan.md` 的 Batch 0-7。

---

## 许可证

[MIT](./LICENSE)
