# tauron

> Tauri 2 插件化框架 —— 安全沙箱、双世界隔离、多形态插件加载的开源客户端基础设施。

[![CI](https://github.com/coeasy/tauron/actions/workflows/ci.yml/badge.svg)](https://github.com/coeasy/tauron/actions/workflows/ci.yml)
[![Tests](https://img.shields.io/badge/tests-1577%20TS%20%C2%B7%201258%20Rust-informational)](#测试)
[![License](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)
[![Node](https://img.shields.io/badge/Node-22.x-brightgreen)](#)
[![Rust](https://img.shields.io/badge/Rust-1.98-orange)](#)

---

## 特性

| 特性 | 说明 |
|---|---|
| **插件信封协议** | 单一 `plugin_invoke` 命令，统一调用/取消/进度 |
| **双世界沙箱** | 双世界隔离架构（QuickJS-WASM 引擎接入为路线图项） |
| **4+1 插件形态** | Rust / JS（真实实现）/ WASM / Process（配置+模拟原型）+ B+ 混合模式 |
| **双层 ACL** | 外层 Tauri 静态 ACL + 内层框架动态 ACL |
| **事件总线** | 每插件队列隔离，背压可配置，命名空间隔离 |
| **配置 4 层合并** | session > plugin > user > default |
| **Shell 矩阵** | local / local-server / remote-url / sub-webview（接口已定义，运行时为模拟原型） |
| **插件市场** | HMAC-SHA256 签名，注册表搜索/发布 |
| **CLI 工具链** | create/plugin new/dev/test/pack/sign/publish/doctor |
| **UI 适配层** | React / Vue / Svelte hooks 和 composables |
| **契约测试** | TS↔Rust 跨语言协议验证 |

---

## 快速开始（第三方集成）

### 1. 安装依赖

> ⚠️ **尚未发布到 registry**：20 个 npm 包当前都是 `private: true`，15 个 crate 的内部
> 互引用是 path 依赖（cargo 打包要求 path 依赖同时给出 `version`）。
> 所以下面**不能**用 `pnpm add @tauron/...` / `cargo add tauron-*` 的 registry 形式，
> 现阶段请按 path / workspace 方式接入：

```bash
# 前端：同一 pnpm workspace 内用 workspace 协议
pnpm add @tauron/types@workspace:* @tauron/core@workspace:*
# 跨仓库则用 file: 协议指向本仓库的 packages/<包名>
```

```toml
# src-tauri/Cargo.toml
[dependencies]
tauron-shell = { path = "../tauron/crates/tauron-shell", features = ["tauri"] }
```

发布到 npm / crates.io 的阻塞项登记在 [CHANGELOG.md 的「已知债务」](./CHANGELOG.md)。

### 2. 注册 Rust 命令层

`tauron-shell` 默认**不依赖 `tauri`**，命令注册层由 `tauri` feature 提供：

```toml
# src-tauri/Cargo.toml
[dependencies]
tauron-shell = { version = "0.1", features = ["tauri"] }
```

```rust
// src-tauri/src/main.rs
fn main() {
    tauri::Builder::default()
        // 状态 + 事件出口（不注册命令）
        .plugin(tauron_shell::commands::state_init())
        // root 注册 plugin_invoke / plugin_cancel / plugin_emit
        //（裸命令名，与 @tauron/core createTauriBackend() 直接匹配，零 capability 配置；
        //  若改用 .plugin(tauron_shell::commands::init()) 插件形态，
        //  则必须为 `tauron-shell` 插件配置 capability——Tauri v2 对 plugin:* 命令强制 ACL）
        .invoke_handler(tauron_shell::tauron_generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

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
# tauron = node packages/tauron-cli/bin/tauron.js（@tauron/cli 尚未发布到 npm）
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

> ⚠️ `@tauron/cli` **尚未发布到 npm**（20 个 npm 包当前均为 `private: true`），
> `npx tauron` 装不到东西。下文用 `tauron` 代指
> `node packages/tauron-cli/bin/tauron.js`（在仓库根执行）。

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
│   └── tauron-wasm/                 # WASM Supervisor
├── packages/                        # 框架层（npm 包）
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
│   └── tauron-contract-tests/       # @tauron/contract-tests — TS↔Rust 跨语言门禁
├── packages/                        # 应用层（npm 包）
│   ├── tauron-host/                 # @tauron/host — 能力编排层（HostClient/TauriBackend）
│   ├── tauron-shell-events/         # @tauron/shell-events — 壳层事件名常量（零依赖）
│   ├── tauron-ui-primitives/        # @tauron/ui-primitives — UI 原语（零宿主依赖）
│   ├── tauron-app-cli/              # @tauron/app-cli — 应用脚手架（bin: tauron-app）
│   ├── tauron-app-plugin-sdk/       # @tauron/app-plugin-sdk — 应用级插件 SDK
│   ├── tauron-app-contract-kit/     # @tauron/app-contract-kit — 应用契约套件
│   ├── tauron-framework/            # @tauron/framework — 框架薄封装
│   └── tauron-ui/                   # @tauron/ui — Lit Web Components
├── examples/
│   └── minimal-app/                 # 最小应用示例
└── docs/
    ├── architecture/                # 权威架构说明 / 线格式协议 / canonical 归属 / 0.3 方案
    ├── integration/                 # 渐进接入指南（三档装配）
    ├── api/                         # 插件开发指南
    └── competitive-analysis/        # 竞品分析（带兑现度标记）
```

> **两层架构说明**
>
> - **框架层**（`@tauron/types` / `core` / `plugin-sdk` / `adapter-*` + `tauron-shell`）
>   面向通用集成：信封协议 `plugin_invoke`、双世界沙箱、插件市场。
>   第三方客户端从这一层接入。
> - **应用层**（`@tauron/host` / `app-*` / `framework` / `ui` + `tauron-host` / `tauron-adapter`）
>   面向完整客户端交付：`host_*` 命令族、生命周期、设置中心、白标。
>   它复用框架层的类型与协议，但不改变框架层契约。

---

## 测试

### 用例数

| 侧 | 用例数 | 口径 |
|---|---|---|
| TypeScript | **1577**（97 个测试文件 / 20 包） | `pnpm -r test` 实跑通过 |
| Rust | **1258** | 源码内 `#[test]` 声明数（静态计数） |
| 跨语言契约 | **125** | `@tauron/contract-tests` 的 wire-gate |

> **关于 Rust 一栏的口径**：Rust 侧除默认特性外还有 **feature 门控**用例
> （`tauron-adapter` / `tauron-shell` 的 `tauri` feature），两者不是同一个数。
> 这里给的是源码声明数，**执行结果以 CI 的 `Rust` / `Rust (tauri feature)`
> 两个 job 为准** —— 徽章上的数字不冒充执行结果。

### 本地命令

```bash
# 安装依赖
pnpm install

# ── TypeScript ───────────────────────────────
# ⚠️ 顺序不可换：测试从各包的 dist/ 解析 workspace 导入，
#    不先 build 会得到一批 "Failed to resolve import" 的假失败。
pnpm -r build
pnpm -r --no-bail typecheck
pnpm -r lint          # ESLint（当前 0 error / 82 warning）
pnpm -r --no-bail test

# 等价的一键命令（build → typecheck → test）
pnpm verify

# ── Rust ─────────────────────────────────────
cargo test --workspace                  # 默认特性：不依赖 tauri，无需系统库
cargo test -p tauron-shell   --features tauri
cargo test -p tauron-adapter --features tauri

# 格式化与 lint（详见 CHANGELOG.md「已知债务」：当前为 advisory）
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
| [架构概览](./docs/architecture/overview.md) | 整体架构图、两层架构、包命名体系、模块依赖、关键设计决策、架构演进 |
| [应用层线格式协议](./docs/architecture/app-layer-wire.md) | `host_*` 命令族的参数 / 返回 / 生命周期事件 / 能力档位规范与限制登记 |
| [canonical 归属](./docs/architecture/canonical-owners.md) | 唯一事实源与已冻结的 legacy 门面（改这张表等于改架构） |
| [0.3 优化改进方案](./docs/architecture/multi-plugin-substrate-roadmap.md) | 多插件框架 × 任意宿主底座：成熟度记分卡、残差清单、S/M/X 改进项与轮次编排 |
| [渐进接入指南](./docs/integration/incremental-adoption.md) | 三档装配：只取底座 / 底座 + 插件运行时 / 完整客户端 |
| [插件开发指南](./docs/api/plugin-development-guide.md) | 创建、测试、打包、发布插件（含 CLI 各命令的真实边界） |
| [竞品分析](./docs/competitive-analysis/competitive-analysis.md) | 竞品全景图、功能对比矩阵、头条特性兑现度标记 |
| [示例应用](./examples/minimal-app/README.md) | 最小集成示例 |
| [CHANGELOG](./CHANGELOG.md) | 版本变更记录与「已知债务」清单 |

---

## 持续集成与发布

| 工作流 | 触发 | 做什么 |
|---|---|---|
| [`ci.yml`](./.github/workflows/ci.yml) | `main` push / PR | TS（build→typecheck→lint→test）、Rust 默认特性、Rust `tauri` feature、wire-gate |
| [`release.yml`](./.github/workflows/release.yml) | 推 `v*` tag | 版本号一致性校验 → windows / macOS(arm64+x64) / linux 矩阵构建 → 草稿 Release |

发布流程：

```bash
# 1. 三处版本号一起升（Cargo.toml 的 [workspace.package] + 根 package.json
#    + 20 个 packages/*/package.json；release 工作流会强校验它们一致）
# 2. 更新 CHANGELOG.md
git tag v0.1.0 && git push origin v0.1.0
```

> `ci.yml` 里 `cargo fmt` / `cargo clippy` / `cargo-deny` 三个 job 目前是
> **advisory**（`continue-on-error`），只报不拦。原因与转正条件见
> [`rustfmt.toml`](./rustfmt.toml) 与 [`CHANGELOG.md`](./CHANGELOG.md)。

---

## 许可证

[MIT](./LICENSE)
