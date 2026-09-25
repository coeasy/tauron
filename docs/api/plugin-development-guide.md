# tauron 插件开发指南

本文档提供 tauron 插件开发的完整指南：从清单结构、权限词表，到宿主命令面、
错误码与生命周期状态机。**所有"已实现 / 未实现"的判定都以仓库当前代码为准**，
凡未接线处一律如实标注（本仓库的规矩是"没接线就写没接线"）。

## 目录

- [概述](#概述)
- [环境准备](#环境准备)
- [创建插件](#创建插件)
- [插件结构（两个清单）](#插件结构两个清单)
- [权限系统](#权限系统)
- [插件 SDK](#插件-sdk)
- [宿主命令面](#宿主命令面)
- [错误码](#错误码)
- [生命周期状态机](#生命周期状态机)
- [进程插件与 sidecar ABI 契约](#进程插件与-sidecar-abi-契约)
- [测试插件](#测试插件)
- [打包与发布](#打包与发布)

---

## 概述

tauron 插件系统在**清单层**定义了四种插件类型。下表第三列是**今天真实成立**的隔离手段，
不是设计意图——本仓库的规矩是"没接线就写没接线"：

| 类型 | 语言 | 今天的真实隔离手段 | 今天能跑吗 | 适用场景 |
|---|---|---|---|---|
| **JS** | JavaScript/TypeScript | **身份 Webview**：插件跑在自己的 webview 里，label 恒为 `plugin-<插件 id>`；宿主命令按身份判定（`Caller`），越权调用的主体在**副作用之前**被拒 | ✅ 能（这是 B 类的默认形态） | UI 扩展、轻量逻辑 |
| **WASM** | WebAssembly | **无**（未接线） | ❌ **不能**：以 WASM 类型启动运行时返回 `E_PLUGIN_TYPE_NO_RUNTIME`（**诚实失败**，不会静默降级成"跑起来了"） | 计算密集、跨平台（**待接线**） |
| **Process** | Rust/C/C++/Node.js | **独立进程**：sidecar spawn 有健康检查与崩溃事件 | ⚠️ 能（有边界，见下） | 系统操作、原生集成 |
| **B+** | JS + Host | **无**（`@tauron/dual-world` 的进程内 JS 沙箱未接运行时，调用一律 fail closed） | ❌ 不能：`SANDBOX_UNAVAILABLE`，**不会**伪报 `executed: true` | 需要宿主能力的插件（**待接线**） |

**必须说清的两点**（此前本文件在这里写成"QuickJS-WASM 沙箱 / WASM 沙箱 / 双世界隔离"，
是**未兑现的宣称**，轮 12 改判）：

1. **JS 插件的隔离来自 webview 边界 + 身份判定，不是来自进程内沙箱**。宿主对命令面
   做代码层身份判定（仅主窗 / 绑定自身命名空间 / 绑定自身身份 / 按身份过滤四类），
   但**不**做资源限额（内存、CPU、通知环占用都没有 per-plugin 配额，见
   `docs/architecture/app-layer-wire.md` 的限制登记）。
2. **Process 插件的边界**：崩溃检测目前是**轮询式**（不是内核级进程组回收）、RPC 帧循环
   尚未接线、进程退出不会连带杀掉它的子进程。所以"进程隔离"成立，但"崩溃自动清理"
   不成立。

---

## 环境准备

### 获取 CLI

> ⚠️ `@tauron/cli` **尚未发布到 npm**（20 个 npm 包当前都是 `private: true`），
> 所以 `npm install -g @tauron/cli` 装不到任何东西。在仓库内直接跑 bin。

**下文一律用 `tauron` 代指 `node packages/tauron-cli/bin/tauron.js`（在仓库根执行）。**

### 验证

```bash
tauron --version   # tauron v0.1.0
tauron doctor      # 环境诊断：探测 Node / pnpm / Rust / Tauri CLI
```

---

## 创建插件

`plugin new` **真的会把文件写到盘上**（落盘是 0.3 轮修正的——此前它只打印
`Created plugin '…' with N files` 而**不写一个字节**，属谎报成功）。
目标目录已存在时默认**拒绝覆盖**，确认覆盖要加 `--force`；`--dry-run` 只报告不写盘。

```bash
tauron plugin new com.example.myplugin --type js
tauron plugin new com.example.calc --type wasm     # 骨架含 Cargo.toml + src/lib.rs；wasm 运行时未接线，跑不起来
tauron plugin new com.example.sys  --type process  # 骨架含 main.js
```

`--type` 支持 `--type wasm` 与 `--type=wasm` 两种写法；**非法值如实失败**，
不会静默降级成 js。

### 产出文件

| 文件 | 作用 |
|---|---|
| `tauron.plugin.json` | **宿主清单**——宿主与生命周期命令读的就是它 |
| `package.json` | npm 打包描述（`type: module`，**不含权限**） |
| `src/index.js`（js/process/wasm 类型） | JS 入口骨架 |
| `main.js`（process 类型） | sidecar 进程骨架 |
| `Cargo.toml` + `src/lib.rs`（wasm 类型） | 可编译的 cdylib 骨架（`cargo check` / `cargo test` 实测通过） |
| `README.md` | 插件说明 |

---

## 插件结构（两个清单）

`tauron plugin new` 生成**两个**清单文件，职责不同，**不要混用**：

| 文件 | 给谁读 | 内容 |
|---|---|---|
| `tauron.plugin.json` | **tauron 宿主**（及 `plugin dev/test/pack/sign/publish` 生命周期命令） | 插件身份、类型、入口、框架版本区间、权限、ABI |
| `package.json` | npm / 打包工具 | 包名、版本、`type: module`、依赖 |

> 生命周期命令读的是 `tauron.plugin.json`。只有 `package.json` 时它们会如实报
> 「no tauron.plugin.json found」——这不是 bug，是清单分工。

### tauron.plugin.json（宿主清单）

```json
{
  "id": "com.example.myplugin",
  "name": "my-plugin",
  "version": "0.1.0",
  "type": "js",
  "entry": { "js": "src/index.js" },
  "framework": ">=2.0 <3.0",
  "permissions": ["store:allow-get", "http:allow-fetch"]
}
```

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | ✅ | 反域名标识（`com.example.myplugin`）。`derivePluginId` 会规范化；不足两段时补 `com.example.` 前缀 |
| `name` | ✅ | 展示名 |
| `version` | ✅ | semver |
| `type` | ✅ | `js` / `rust` / `process` / `wasm` |
| `entry` | ✅ | 入口，按类型取对应键：`js` / `sidecar`（process）/ `wasm` / `ui` |
| `framework` | ✅ | **必填** semver range；缺失即视为与框架不兼容 |
| `permissions` | ✅ | 只能取自 `schema/permissions.index.json`；表外字符串会被宿主拒绝 |
| `abi` | 仅 A/D 类 | `{ "rust": "…" }`（rust 类）或 `{ "wasm": "…" }`（wasm 类）；B/C 类声明它会被拒 |

**入口与 ABI 的加载期硬校验**（`crates/tauron-host/src/manifest.rs`，D13）：

- process 类必须声明非空 `entry.sidecar`；
- wasm 类必须声明非空 `entry.wasm`；
- rust 类必须声明 `abi.rust`，wasm 类必须声明 `abi.wasm`；
- 非 A/D 类（js / process）**不得**声明 `abi`。

> 宿主侧的 `PluginManifest` 还支持 `scopes` / `platforms` / `contributes` /
> `settings_schema` / `events` / `host_functions` / `min_allowed_version` /
> `signature` / `publisher` 等字段（见 `manifest.rs`）。上面的表是**脚手架生成的
> 必填子集**——手写清单时按需补齐，未知字段会被 `deny_unknown_fields` 拒绝。

### package.json

```json
{
  "name": "com.example.myplugin",
  "version": "0.1.0",
  "type": "module",
  "main": "src/index.js",
  "devDependencies": { "@tauron/plugin-sdk": "^0.1.0" }
}
```

> `type` 恒为 `module`（脚手架产出 ESM）。**权限不写在这里**——它在
> `tauron.plugin.json`。早期文档把它写成 `"type": "js"` 并塞入 `permissions`，
> 两者都会让打包 / 加载走错路径。

---

## 权限系统

权限标识符只能取自 `schema/permissions.index.json`。写表外字符串（如 `store:read`）
会被宿主 `PluginManifest::validate` **拒绝**——不会静默忽略。

- **risk**：框架自补的元数据（Tauri 官方未提供），用于审批 UI 分级：
  `low` / `elevated` / `high`。
- **scoped**：该权限在 Tauri 侧需要额外的作用域声明才生效。

### 存储（store）

| 标识符 | risk | 说明 |
|---|---|---|
| `store:allow-get` | low | 读取单个值 |
| `store:allow-get-many` | low | 批量读取多个键 |
| `store:allow-contains` | low | 判断键是否存在 |
| `store:allow-set` | elevated | 写入单个值（scoped） |
| `store:allow-set-many` | elevated | 批量写入 |
| `store:allow-merge` | elevated | 合并写入 |
| `store:allow-remove` | elevated | 删除单个键 |
| `store:allow-clear` | **high** | 清空整个存储 |

### 文件系统（fs）

| 标识符 | risk | 说明 |
|---|---|---|
| `fs:allow-app-read` | low | 读应用数据目录（scoped） |
| `fs:allow-app-read-recursive` | elevated | 递归读应用目录（scoped） |
| `fs:allow-app-write` | elevated | 写应用数据目录（scoped） |
| `fs:allow-app-write-recursive` | **high** | 递归写应用目录（scoped） |
| `fs:allow-read-text-file` | low | 读单个文本文件（scoped） |
| `fs:allow-write-text-file` | elevated | 写单个文本文件（scoped） |
| `fs:allow-copy-file` | elevated | 复制文件（scoped） |
| `fs:allow-rename-file` | elevated | 重命名 / 移动（scoped） |
| `fs:allow-remove` | **high** | 删除文件或目录（scoped） |

### 网络 / 事件

| 标识符 | risk | 说明 |
|---|---|---|
| `http:allow-fetch` | elevated | 发起 HTTP/HTTPS 请求（scoped） |
| `http:default` | elevated | HTTP 默认权限集 |
| `event:allow-emit` | low | 广播事件 |
| `event:allow-listen` | low | 监听事件 |
| `event:allow-unlisten` | low | 取消监听 |

### 系统集成

| 标识符 | risk | 说明 |
|---|---|---|
| `shell:allow-execute` | **high** | 执行任意外部命令（**在禁止授予清单内**） |
| `shell:allow-open` | elevated | 用系统默认程序打开 URL / 文件 |
| `opener:allow-open-url` | elevated | 调用系统 opener |
| `dialog:allow-open` / `allow-save` / `allow-message` | low | 系统对话框 |
| `window:allow-create` | elevated | 创建新窗口 |
| `window:allow-close` / `allow-set-title` / `allow-set-focus` | low | 窗口控制 |
| `app:allow-version` / `allow-name` | low | 读宿主版本 / 名称 |
| `global-shortcut:allow-register` / `allow-unregister-all` | elevated | 全局快捷键 |
| `autostart:allow-enable` | **high** | 注册开机自启 |
| `autostart:allow-disable` | elevated | 注销开机自启 |
| `updater:allow-check` | elevated | 检查更新 |
| `updater:allow-download-and-install` | **high** | 下载并安装更新 |
| `clipboard-manager:allow-read-text` / `allow-write-text` | elevated | 剪贴板 |
| `os:allow-platform` | low | 读平台标识 |
| `process:allow-restart` | **high** | 重启宿主进程 |

> **禁止授予清单**（`crates/tauron-host/src/authz.rs::FORBIDDEN_PLAIN`）：申请
> `shell:allow-execute` 会被 `E_FORBIDDEN_PERMISSION` **拒绝**，而不是静默降级。

---

## 插件 SDK

JS 插件通过 `@tauron/plugin-sdk` 注册：

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
});
```

SDK 的 `createPlugin()` 在 `attach` 期调用 `host_contributes_register` 注册贡献，
在 `dispose` 期调用 `host_events_unsubscribe` 退订。事件取件走 `host_events_drain`
（拉取本插件待投递帧）。测试替身见 `@tauron/plugin-sdk/testing`。

---

## 宿主命令面

命令面分两层。**档位强制不在统一 dispatcher 里**——每条命令的档位语义在
**各自的 wire 入口**落地（self 档从 webview label 解析身份、privileged 档限主窗），
登记表（`authz::COMMANDS` / `ADMIN_COMMANDS`）的强制兑底是**门禁测试**
（`validate_command_registry` + TS 镜像 `capabilities.ts`）。

### 插件面命令（16 条，登记在档位表内）

**Self_（12 条）**——身份取自 webview label（`plugin-<id>`），入参里的身份字段一律忽略（防冒充）：

| 命令 | 说明 |
|---|---|
| `host_plugin_call` | C/D 类插件的 JS↔宿主调用往返（取代信封式 `host_call_begin`） |
| `host_call_end` | 流式调用终帧确认 |
| `host_cancel` | 取消调用，取消传播到 sidecar / supervisor |
| `host_lifecycle_report` | 上报生命周期事件（state / reason） |
| `host_contributes_register` | 注册 contributes（commands / menus / panels / …） |
| `host_events_publish` | 事件发布唯一入口（越界丢弃 + 计数） |
| `host_events_subscribe` | 事件订阅（跨插件订阅需对方 `public: true`） |
| `host_events_unsubscribe` | 事件退订（窗口销毁时由宿主回收） |
| `host_events_drain` | 拉取本插件待投递帧（只取自己订阅的可见集） |
| `host_stream_open` | 为一次已挂帧载体的调用开流 |
| `host_stream_write` | 写一帧（seq 由宿主铸） |
| `host_stream_close` | 发终帧并使句柄失效 |

**ScopedRead（1 条）**：

| 命令 | 说明 |
|---|---|
| `host_registry_list` | 列出可见插件（结果按可见性过滤） |

**Privileged（3 条）**——仅主窗 / 宿主 UI：

| 命令 | 说明 |
|---|---|
| `host_registry_admin` | 管理操作：`disable` / `enable` / `uninstall` / `purge` |
| `host_runtime_spawn` | 启动进程插件 sidecar（幂等：已有租约则返回既有 pid / lease） |
| `host_runtime_health` | 按租约查询 sidecar 健康（pid / 崩溃窗口计数；暴露 PID 故同属特权） |

### 底座命令（主窗专属）

其余命令（`host_window_*` / `host_settings_*` / `host_notify` / `host_recover_*` /
`host_market_*` / `host_i18n_*` / `host_brand_info` / `host_registry_list_all` /
`host_contributes_list` 等）**不在档位表内**——它们是主窗专属命令，由 Tauri
capability 的 `windows` 字段限制（只授予 `main`），不走 `authz` 表。
完整线格式见 `docs/architecture/app-layer-wire.md`。

> **`host_call_begin` / `host_grant_request` 是故意移除的命令**，回归会被
> `packages/tauron-host/src/gates.test.ts` 拦下。

---

## 错误码

两套码表**零交集**：框架层 `SC-xxxx`（插件沙箱语义），应用层 `E_*`（宿主底座语义）。

### 框架层 `SC-xxxx`（14 个，`@tauron/types`）

| 码 | 常量 | 说明 |
|---|---|---|
| `SC-0001` | `PLUGIN_NOT_FOUND` | 插件不存在 |
| `SC-0002` | `PLUGIN_DISABLED` | 插件已禁用 |
| `SC-0003` | `PLUGIN_ERRORED` | 插件处于错误态 |
| `SC-1001` | `PERMISSION_DENIED` | 权限被拒 |
| `SC-1002` | `PLUGIN_PERMISSION_DENIED` | 插件权限被拒 |
| `SC-1003` | `CAPABILITY_REQUIRED` | 需要 capability |
| `SC-2001` | `TIMEOUT` | 超时（**可重试**） |
| `SC-2002` | `CANCELLED` | 已取消 |
| `SC-2003` | `INVALID_PAYLOAD` | 载荷非法 |
| `SC-2004` | `CHANNEL_BROKEN` | 通道断开（**可重试**） |
| `SC-3001` | `PLUGIN_PANIC` | 插件 panic |
| `SC-3002` | `PLUGIN_OOM` | 插件内存耗尽 |
| `SC-3003` | `PLUGIN_EXITED` | 插件退出 |
| `SC-9001` | `INTERNAL` | 内部错误（**可重试**） |

### 应用层 `E_*`（19 个，`tauron-host`）

变体名即**跨 IPC 线协议名**（改名即破坏兼容）。TS 侧 `HOST_ERROR_CODES`
按**声明顺序**比对（wire-gate 门禁）。

| 码 | 触发点 |
|---|---|
| `E_HOST_PANIC` | handler panic，经 `catch_unwind` 归一化（**可重试**） |
| `E_UNKNOWN_PLUGIN` | 未知 `plugin_id` |
| `E_AUTH_DENIED` | 档位不满足，或身份被伪造 |
| `E_INVALID_MANIFEST` | 清单不合法：未知字段、非法 id、权限表外字符串、缺 `framework`、`abi` 字段缺失/非法 |
| `E_STATE_INVALID_TRANSITION` | 状态机无匹配规则（非法迁移） |
| `E_CALL_NOT_FOUND` | pending call 不存在或已结束 |
| `E_CALL_TIMEOUT` | pending call 超时（**可重试**） |
| `E_FORBIDDEN_PERMISSION` | 申请了禁止授予清单内的权限 |
| `E_ABI_MISMATCH` | `host_runtime_spawn` 的 ABI 契约比对失败（见下节） |
| `E_PLUGIN_DISABLED` | 插件已禁用（含崩溃预算耗尽） |
| `E_REGISTRY_FULL` | 注册表达到活跃身份上限（缺省 8） |
| `E_CALL_PENDING_FULL` | pending call 表达到上限 |
| `E_SUBSCRIPTION_FULL` | 订阅表达到上限 |
| `E_PLUGIN_EXISTS` | 插件已存在（重复注册） |
| `E_INSTALL_FAILED` | 安装期失败：验签 / hash / 解包 / range |
| `E_PLUGIN_FILTERED` | 被配置过滤器排除（**可重试**） |
| `E_PLUGIN_TYPE_NO_RUNTIME` | 该插件类型没有运行期执行器（如对 js/wasm 插件调 `host_runtime_spawn`） |
| `E_LEASE_EXPIRED` | 运行时租约不存在或已失效 |
| `E_STREAM_FULL` | 流句柄数达到上限（`MAX_STREAMS = 1024`）——先 `host_stream_close` 再开 |

**可重试集合只有 3 个**：`E_HOST_PANIC` / `E_CALL_TIMEOUT` / `E_PLUGIN_FILTERED`。
其余一律不可自动重试——把一个确定性失败标成可重试会让前端无限重试。

---

## 生命周期状态机

**10 态 / 18 事件 / 7 守卫**，表驱动（`crates/tauron-host/src/lifecycle.rs::TRANSITIONS`，
57 条规则，表内顺序即匹配优先级）。

### 状态（10）

```
DISCOVERED → INSTALLING → INSTALLED → ENABLED ⇄ RUNNING
                                 ↓        ↓
                             DISABLED  ERRORED_RETRYABLE
                                 ↓        ↓
                          INSTALL_FAILED  ERRORED_USER_CONFIRM
                                 ↓        ↓
                              UNINSTALLED（终态）
```

| 状态 | 说明 |
|---|---|
| `DISCOVERED` | 初始态（唯一无入边的状态） |
| `INSTALLING` | 安装中 |
| `INSTALLED` | 装完未启用 |
| `ENABLED` | 已启用（可用） |
| `RUNNING` | 已附着运行 |
| `DISABLED` | 已禁用（含 `disabledBySafemode`） |
| `ERRORED_RETRYABLE` | 可重试错误档 |
| `ERRORED_USER_CONFIRM` | 需用户确认 |
| `INSTALL_FAILED` | 准终态（仅可卸载，不自动重试） |
| `UNINSTALLED` | **完全终态**（无任何出边） |

### 事件（18）

用户可触发：`InstallStart` / `InstallOk` / `InstallFail` / `Enable` / `Disable` /
`Uninstall` / `Purge` / `TrialEnable`。

系统触发：`Attach` / `Detach` / `ErrorRetryable` / `ErrorFatal` / `RetryOk` /
`RetryExhausted` / `SafemodeEnter` / `SafemodeExit` / `HealthOk` / `RuntimeCrash`。

### 三个关键预算

| 常量 | 值 | 作用 |
|---|---|---|
| `MAX_RETRY` | 3 | 自动重试次数上限，超过即升级为需用户确认 |
| `CIRCUIT_THRESHOLD` | 3 | 连续失败熔断阈值。熔断打开后 `ErrorRetryable` **不再自动重试**，直接交人工 |
| `MAX_TRIAL_ATTEMPTS` | 3 | 安全模式内试验性启用次数上限（跨次累积） |

> **熔断的读写闭环**：`circuit_open` 由 `IncrementFailure` 累积到阈值时置位，
> 由 `HealthOk` 的 `DecayRetry`（retry / failure 同时归零）或 `Enable` 的
> `ResetCounters` 关断。门禁 `every_guard_and_action_is_referenced_by_a_rule`
> 保证每个守卫 / 动作都有规则引用（防止"有实现、没入口"的死代码）。

---

## 进程插件与 sidecar ABI 契约

进程插件（`type: process`）的 sidecar 由 `host_runtime_spawn` 启动，参数是
`{ pluginId, profile }`（两个顶层参数）。`profile` 走 camelCase：

```json
{
  "binaryPath": "…",
  "args": ["--serve"],
  "env": { "RUST_LOG": "info" },
  "signature": { "algorithm": "ed25519", "signature": "…", "signerId": "…" },
  "binaryHash": "…64 位 hex…",
  "abi": { "rustVersion": "tauron-proc-abi/1", "interfaceHash": "tauron-sidecar-rpc/1" }
}
```

### ABI 契约（必须填对）

`abi` 的两个维度**必须等于宿主当前支持的 ABI 契约**，否则 spawn 被拒（**启动面
之前**——不会拉起 sidecar、不留租约），返回 `E_ABI_MISMATCH`：

| 维度 | 契约值 |
|---|---|
| `rustVersion` | `tauron-proc-abi/1` |
| `interfaceHash` | `tauron-sidecar-rpc/1` |

前端请直接用 `@tauron/host` 导出的常量，**不要硬编码字符串**：

```typescript
import { SIDECAR_ABI_CONTRACT } from '@tauron/host';

await shell.runtimeSpawn('com.example.sys', {
  args: [], env: {}, signature: { /* … */ }, binaryHash: '…',
  abi: { ...SIDECAR_ABI_CONTRACT },   // 别自己拼版本串
});
```

> 契约串**不随框架发版变动**：`/1` 只在 sidecar 协议本身发生**不兼容**改动时递增。
> 两端的同值由 wire-gate 门禁锁死（`门禁：sidecar ABI 契约 TS ↔ Rust 同值`）。
>
> **诚实边界**：`profile.abi` 是调用方**自报**的，因此这道校验挡的是"配置错配 /
> 前端用了旧模板"，**不是**"恶意调用方伪造 ABI"——后者需要 sidecar 在 RPC 握手时
> 自报指纹（**尚未实现**）。它与 `validate_spawn_config` 的签名 / 哈希检查同属
> "配置一致性"层，不是安全边界。

### 失败码

| 情形 | 错误码 |
|---|---|
| 插件类型不是 `process` | `E_PLUGIN_TYPE_NO_RUNTIME`（**不伪造 pid**） |
| ABI 契约不符 | `E_ABI_MISMATCH`（版本兼容问题，该升级而非重装） |
| 签名 / hash 不合格 | `E_INSTALL_FAILED` |
| 插件不处于 `ENABLED` / `RUNNING` | `E_PLUGIN_DISABLED`（先 enable 再 spawn） |
| 未知 / 失效租约 | `E_LEASE_EXPIRED` |

---

## 测试插件

```bash
tauron plugin test    # ⚠️ 未实现：不执行任何测试，也不给出通过/失败计数
tauron plugin dev     # ⚠️ 未实现：不启动 dev server、不监听端口、不做 watch
```

**这两条命令目前都会如实返回 `success: false`**，并说明"未实现"。`plugin dev`
会**读** `tauron.plugin.json` 并回显插件身份（证明清单可解析），但**不启动任何服务**；
`plugin test` 只做文件发现，`executed: false`，**不给出** `passed` / `failed` 字段
（不存在的执行结果不该有字段可填）。测试请直接用 vitest / jest：

```bash
npx vitest run          # 或 npx jest
```

---

## 打包与发布

```bash
tauron plugin pack     # ⚠️ 未实现：只列"将要打包"的清单，不产出 .tgz
tauron plugin sign     # ✅ 实现：算真实 SHA-256 内容摘要并写出 .sig（不是非对称签名）
tauron plugin publish  # ⚠️ 未实现：不会上传，也不编造商城地址
```

三处的真实边界（都在返回值里字段化，不靠文档口头承诺）：

| 命令 | 真做了什么 | 没做什么 |
|---|---|---|
| `pack` | 读清单、按 `includes` 列文件、算总字节数 | **不写任何归档文件**（`package: null`，只有 `plannedPackage` 名字） |
| `sign` | 读取产物、算真实 SHA-256 摘要、把 `.sig` **真的写出来** | 不做 Ed25519 非对称签名（要真签名用 `tauron-app plugin sign`，走 `@tauron/market`） |
| `publish` | 校验清单形状 | **不上传**、不返回商城 URL（`published: false` / `simulated: true`） |

需要完整发布链路时，请用你自己的流水线打包，再对产物跑 `sign`，最后交给
`tauron-app` 侧的商城客户端（`@tauron/market`，Ed25519 验签）。
