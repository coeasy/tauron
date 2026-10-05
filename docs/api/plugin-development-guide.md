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
- [跨主体调用（0.4-A1）](#跨主体调用04-a1)
- [设置变更观察（1.1 新增，V4 §33 R2-4 / W6）](#设置变更观察11-新增v4-33-r2-4--w6)
- [宿主命令面](#宿主命令面)
- [错误码](#错误码)
- [生命周期状态机](#生命周期状态机)
- [进程插件与 sidecar ABI 契约](#进程插件与-sidecar-abi-契约)
- [测试插件](#测试插件)
- [打包与发布](#打包与发布)

---

## 概述

tauron 插件系统在**清单层**定义了四种插件类型（`PluginType` 枚举：`Rust` / `Js` /
`Process` / `Wasm`，见 `crates/tauron-host/src/manifest.rs:301`）。下表第三列是
**今天真实成立**的隔离手段，不是设计意图——本仓库的规矩是"没接线就写没接线"：

| 类型 | 语言 | 今天的真实隔离手段 | 今天能跑吗 | 适用场景 |
|---|---|---|---|---|
| **Rust**（A 类） | Rust | **无执行器**：crate 直接编译进宿主是设计意图，但 `PluginType::Rust` 目前**只出现在清单校验与测试**中，运行期落到 `UnwiredDelivery` | ❌ **不能**：返回 `E_PLUGIN_TYPE_NO_RUNTIME`（**诚实失败**） | 与宿主同进程的深度集成（**待接线**） |
| **JS**（B 类） | JavaScript/TypeScript | **身份 Webview**：插件跑在自己的 webview 里，label 恒为 `plugin-<插件 id>`；宿主命令按身份判定（`Caller`），越权调用的主体在**副作用之前**被拒 | ✅ 能（这是默认形态） | UI 扩展、轻量逻辑 |
| **Process**（C 类） | Rust/C/C++/Node.js | **独立进程**：sidecar spawn 有健康检查与崩溃事件 | ⚠️ 能（有边界，见下） | 系统操作、原生集成 |
| **WASM**（D 类） | WebAssembly | **无**（未接线，进程内无 WASM 运行时） | ❌ **不能**：以 WASM 类型启动运行时返回 `E_PLUGIN_TYPE_NO_RUNTIME`（**诚实失败**，不会静默降级成"跑起来了"） | 计算密集、跨平台（**待接线**） |

> **`B+`（JS + Host）不是 `PluginType` 变体**：它是 `@tauron/dual-world` 的进程内 JS
> 沙箱**模式**，未接运行时，调用一律 fail closed（`SANDBOX_UNAVAILABLE`，**不会**伪报
> `executed: true`）。清单里写不出 `type: "b+"`——此前的表述容易让人误以为它是第五种类型。

**必须说清的两点**（此前本文件在这里写成"QuickJS-WASM 沙箱 / WASM 沙箱 / 双世界隔离"，
是**未兑现的宣称**，轮 12 改判）：

1. **JS 插件的隔离来自 webview 边界 + 身份判定，不是来自进程内沙箱**。宿主对命令面
   做代码层身份判定（仅主窗 / 绑定自身命名空间 / 绑定自身身份 / 按身份过滤四类），
   但**不**做资源限额（内存、CPU、通知环占用都没有 per-plugin 配额，见
   `docs/architecture/app-layer-wire.md` 的限制登记）。
2. **Process 插件的边界**。此前本文件在这里写"RPC 帧循环尚未接线、进程退出不会连带
   杀掉它的子进程"——那两条已被代码推翻（轮 11 核实，见
   `crates/tauron-proc/src/spawner.rs` 的 `CommandSpawner` 文档与 `Drop` 实现），
   正确的口径是：
   - **JSON-RPC 帧回路已接线**（0.4-A1）：stdin/stdout 走 `Stdio::piped()`，每条
     sidecar 在 `spawn` 时单独起一个读线程排空 stdout，回帧（含 `callId`）经
     `ProcessFrameSinkImpl` 调 `Registry::settle_call` 闭合链路。**轮 22 收口
     （V7-P1-05）**：此前这里如实写着「只有契约与格式级证据、没有运行期证据」——现在
     仓内有 `crates/tauron-test-sidecar`（`publish = false` 的 CI 夹具，永不上架），
     它的 `tests/sidecar_e2e.rs` 真起操作系统进程，并走这同一条投递/结算链路，
     "sidecar 真收帧 / 真回帧 / 宿主真结算 / 禁用时真杀进程"因此升级为运行期证据。
   - **进程树回收已接线**：Unix/macOS 用独立 POSIX process group
     （`kill(-pgid, SIGKILL)`），Windows 用带 `KILL_ON_JOB_CLOSE` 的 Job Object；
     `CommandSpawner` 被最终持有者丢弃时，仍在跟踪的每个 pid 都走同一套 tree-aware
     `kill`。终止失败**不再无人认领**：pid 进有界重试队列，宿主侧由
     `host_resource_stats`（主窗轮询诊断）推进，到尝试上限即固化成
     `reap.terminalRecords` 里的可核对证据（pid / 插件 / 原因）。重试**不会误杀同号的
     活进程**：待重试的 pid 若已被某条在册租约持有（OS 重用同号 + 插件重装是真实形状），
     本轮直接**让位**并计 `reap.skippedLivePid`——终止器按 pid 找子进程，分不出"旧进程
     已死、号被复用"与"旧进程还在"，让位是这里唯一诚实的第三种结局。仍然保留的 OS 语义
     限制：**宿主自身正在退出**时没有驱动方会再跑一轮；那次终止失败会落进跨重启台账
     （轮 41 起，恢复数据目录的 `reap-ledger.json`），下次启动时平台探测逐条定性——
     `Gone` 销账，仍在运行/无法判定的**只留证据、不盲杀**（跨重启验明不了进程身份，
     见 `reap.sweepSurvivors` / `reap.sweepUnknown`）。因此该子进程**可能存活，但这是
     有记录、查得到 pid 与原因的事实**（不假装已解决）。
   - **仍然成立的限制**：① 崩溃检测是**轮询式**——没有后台监控线程，宿主不调
     `host_runtime_health` 就发现不了死亡；② 两个 provider 都自报
     `ProcessSandboxEnforcement::Partial`（无文件系统/网络/系统调用隔离，Windows 还有
     post-spawn attach race），因此 `productionDoctor` 的 `process-sandbox` 检查项不会
     因它们变绿。Production 下**接上 `ProcSpawner` 即被拒启**（装配期
     `validate_process_runtime_for_start` 报 `PROCESS_SANDBOX_HARD_REQUIRED`）；即使
     绕过装配直接调 `host_runtime_spawn` 也一样被拒（`E_STATE_INVALID_TRANSITION`，
     判定只作用于"需要起新进程"那条路径，已有活租约的幂等查询不受影响）。
   一句话：**进程隔离与退出连带回收成立，OS 级资源/系统调用沙箱与事件驱动崩溃检测不成立。**

---

## 环境准备

### 获取 CLI

`@tauron/cli` 在 npm 上的 `latest` 是 **1.0.2**（2026-10-02 实测量：`npm view @tauron/cli version`
→ `1.0.2`）；本仓库的 `1.1.0` 已备好但**尚未发布**，所以 `npm install -g @tauron/cli@1.1.0`
现在会 `ETARGET`。在 Tauron 仓库内开发请直接运行 bin，无需安装：

**下文一律用 `tauron` 代指 `node packages/tauron-cli/bin/tauron.js`（在仓库根执行）。**

### 验证

```bash
tauron --version   # tauron v1.1.0（仓库 bin；npm 全局装到的是 1.0.2，会打印 v1.0.2）
tauron doctor      # 环境诊断：探测 Node / pnpm / Rust / Tauri CLI
```

`tauron doctor` 的退出码是契约的一部分：**报告里有任何 `fail` 项即 exit 1**，
`warn` / `skip` 只提示、不改判（轮 16 之前它无条件返回 0，CI 会把「✗ N check(s) failed」
这句诊断结论直接抹掉）。`tauron-app doctor` 同口径（`result.ok` 为假即 exit 1）。

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
>
> ⚠️ 其中 `min_allowed_version` **只被解析、尚未生效**：宿主安装路径对已存在的
> 插件 id 一律返回 `E_PLUGIN_EXISTS`，不存在"覆盖安装 / 升级 / 降级"流程，因此
> 这条版本下限没有判定点。写上它不会报错，但也不会产生任何效果。

### package.json

```json
{
  "name": "com.example.myplugin",
  "version": "0.1.0",
  "type": "module",
  "main": "src/index.js",
  "devDependencies": { "@tauron/plugin-sdk": "^1.1.0" }
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

JS 插件有**两代 SDK**。新插件一律从主轨起步；旧轨是 iframe 代理协议（§4.2B）一代的
过渡产物，仍是公开 API 但不再演进。

### 主轨：`@tauron/app-plugin-sdk`（推荐）

`createPlugin` 是声明式工厂——命令、设置页、贡献、事件订阅全部声明在定义对象上，
`activate` 期由 SDK 统一注册：

```javascript
import { createPlugin } from '@tauron/app-plugin-sdk';

const plugin = createPlugin({
  id: 'com.example.myplugin',
  name: 'my-plugin',
  version: '1.0.1',
  commands: {
    async format(args, ctx) {
      return { formatted: String(args.code).replace(/\s+/g, ' ') };
    },
  },
  events: { subscribe: ['data-changed'] },
  onEvent(topic, payload, ctx) { /* … */ },
});

export default plugin;
```

要点（都以代码为准）：

- **注册命令即开泵**：宿主总线是**拉取**模型（`host_events_drain`），帧只进队列、
  没人取就永远到不了订阅者。`createPluginContext` 内建的取件泵每 100ms 取回
  request / event 两路帧并分发——这只泵同时也是**跨主体调用的执行泵**（见
  [跨主体调用](#跨主体调用04-a1)）。没有泵，「订阅成功」永远收不到帧。
- `contributes` 在 `activate` 期逐条 `host_contributes_register`（best-effort：
  宿主侧重复 id 只告警不阻断激活，卸载时由宿主按插件 id 整体回收）。
- `PluginContext` 就是共享契约（`@tauron/plugin-context-contract`）的具体化版本，
  签名漂移由编译期断言拦下。
- 测试替身：`createPluginTestContext` / `createMockContext`
  （`@tauron/app-plugin-sdk/testing`）。

### 旧轨：`@tauron/plugin-sdk`（legacy，保留不删）

```javascript
import { registerPlugin } from '@tauron/plugin-sdk';

registerPlugin({
  name: 'my-plugin',
  version: '1.0.1',
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

旧轨面向 `__invoke:` / `__result:` postMessage 代理协议，`registerPlugin` 只 wire 了
`onEnable`（宿主没有下发禁用通知的手段，`onDisable` 是尽力而为）。已有插件可以继续用，
新插件不要从它起步。

---

## 跨主体调用（0.4-A1）

宿主 ↔ 插件、插件 ↔ 插件的跨主体调用已闭环：**登记 → 投递 → 执行 → 回填 → 取件**
每一跳都有生产实现（不是「有命令、没链路」）。发起主体由宿主从 webview label 解析
（防冒充），主窗即 `"main"`；配额记在**发起方**名下。

### 发起方（主窗示例）

```ts
import { isUnsupportedBody } from '@tauron/host';

const accepted = await shell.callPlugin('com.example.calc', 'add', { a: 1, b: 2 });
// 返回的是 `ProviderResult<PendingCallInfo>`：**先分流再取件**。
// `Unsupported` = 这个宿主没有投递通路，它**没有** `callId`，
// 直接 `accepted.callId` 会拿到 `undefined` 并让取件以非法入参失败。
if (isUnsupportedBody(accepted)) {
  throw new Error(`${accepted.reason}（建议：${accepted.fallback ?? '无'}）`);
}
// accepted 现在是 PendingCallInfo；state === 'pending' 表示已登记、已投递，等执行方回填
// 稍后取件（一次性语义：settled 取走即删；pending 返回副本、条目保留）
const done = await shell.callTakeResult(accepted.callId);
if (done.state === 'settled') {
  if (done.errorCode) { /* 执行方失败 */ } else { /* 用 done.result */ }
}
```

`ShellClient.callPlugin(target, method, argsJson?)` 只能在主窗调（宿主主窗视角的
封装）。插件侧发起用 `HostClient.callPlugin` / `takeCallResult`（self 档：只有
发起方本人能取走结果）。

两侧都返回 `ProviderResult<PendingCallInfo>`：`Unsupported` 支是**通路事实**（没有
投递通路），与「执行方回填了失败」是两个完全不同的失败面——后者有 `callId`、有
`errorCode`，前者什么都没有。分流用 `isUnsupportedBody()`，别用 `'callId' in x`。

### 执行方（插件）

**推荐什么都不用写**：只要命令是通过 `@tauron/app-plugin-sdk` 注册的（声明式
`commands` 字段或 `ctx.commands.register`），取件泵会自动识别 `plugin:<id>:__call`
帧、执行 handler 并经 `host_call_result` 回填结果；命令不存在或 handler 抛异常
都会回填 `E_CALL_EXEC_FAILED`（执行方侧约定码，**不属于**宿主 `E_*` 枚举——执行方
对自己「能不能执行」负责）。**失败也必须回填**：静默 = 发起方等到 TTL 超时。

需要手工结算时（例如 process 插件自行实现帧回路）用
`HostClient.reportCallResult({ callId, ok, result?, errorCode? })`：

- 仅该调用的 target 本人可回填（宿主校验身份 == target）；
- 对已结算的调用重复回填得 `E_CALL_ALREADY_SETTLED`（幂等拒绝，**不覆盖**）。

### 投递通路（宿主按目标插件形态选）

| 目标形态 | 通路 | 无通路时 |
|---|---|---|
| js | 事件总线 request 通道（topic `plugin:<id>:__call`，插件经 `host_events_drain` 取件） | — |
| process | sidecar stdin 帧回路（`tauron-adapter::process_delivery`） | — |
| `Rust` / `Wasm`（`PluginType` 变体；无 B+ 变体） | 无 | 结构化 `Unsupported` 失败（诚实失败，不假装投递成功） |

js / process 两条通路都在 `tauron-adapter::default_deliveries` 里生产装配；未装配
任何通路时落到 `UnwiredDelivery`，返回有类型的 `Unsupported` 而非静默成功。

### 诚实边界：跨主体调用是**一元**的（今天没有流式通路）

- `host_call_plugin` 线格式**没有** `channel` 形参，核心 `call_begin_cross` 也**不**
  为调用绑定帧载体；TS 侧 `HostClient.callPlugin` 只返回一元簿记，结算口
  （`reportCallResult` / `takeCallResult`）同样一元。故**跨主体调用无流式通路**。
- 流式命令（`host_stream_open` / `write` / `close`）只服务 `host_plugin_call`
  这条**自带后端**的调用路径；其身份取自 `plugin-<id>` label（`subscriber_of`），
  因此**主窗不能开流**（主窗 label 不是 `plugin-` 前缀，会被 `E_AUTH_DENIED` 拒）。
- 这是**两侧对称**的"未接线"，**不是**某一侧的断链：需要跨主体流式时按新链路立项，
  不要只补一侧（补一侧会立刻变成断链）。

---

## 设置变更观察（1.1 新增，V4 §33 R2-4 / W6）

设置写入走 `host_settings_set`，**观察者**走消息面：宿主在值**落盘提交之后**把变更
镜像到宿主所有的 topic `host:settings:changed`（`event` 通道）。插件侧不需要手写
任何订阅代码——声明 `onSettingsChanged` 即可：

```typescript
import { createPlugin, HOST_SETTINGS_CHANGED_TOPIC } from '@tauron/app-plugin-sdk';

export default createPlugin({
  id: 'com.example.formatter',
  name: 'Formatter',
  version: '1.0.0',
  // 入参是「本轮变化的键 → 新值」，键为**线形键**（与 set/get 用的形态一致，
  // 不是 Store 内部的编码点路径），可直接回查/回写。
  onSettingsChanged(settings, ctx) {
    const width = settings['plugin:com.example.formatter.width'];
    ctx.log.info(`宽度改为 ${String(width)}`);
  },
});
```

### 谁能收到

> 这条订阅是 SDK 在激活时**自动**建立的，与 `events.subscribe` 的声明清单**无关**：
> 不用把 `host:settings:changed` 也写进声明里。写进去不会重复建宿主订阅
> （同一 topic 只订阅一次），但会**额外**触发一次 `onEvent`——两个回调都会收到同一帧。
> 需要审批的仍然是**宿主侧**——见下表第一行。

| 条件 | 结果 |
|---|---|
| 未获主窗批准该 topic | 订阅拿不到 token → 取件泵不起跑 → 钩子**永不触发**（静默降级，激活照常成功） |
| 已获批准 | 只收到**本插件命名空间**的键：等于 `plugin:<自己 id>` 或以 `plugin:<自己 id>.` 开头 |
| 别的插件的键、宿主级键（`host.*`） | 不投递（SDK 侧按命名空间过滤，与宿主读写边界同一套判定） |

批准由主窗发起（`host_events_approve` 的 TS 口是 `AdminClient.eventsApprove`）：

```typescript
await admin.eventsApprove('com.example.formatter', HOST_SETTINGS_CHANGED_TOPIC);
await shell.settingsSet('plugin:com.example.formatter.width', 120); // 插件窗随后收到
```

`examples/minimal-app` 的「打开插件面板窗口」按钮跑的就是这两步。

### 诚实边界

- **未获批时的重试是有界的**：SDK 的惰性订阅按 `1 / 2 / 4 / 8 / 16 / 32` 秒退避，
  即**首次申请 + 6 次重试 = 共 7 次**（累计约 63 秒）。阶梯走空后不再自动重试，只打一条
  `warn` 日志——所以「批准后自动生效」的窗口就是这 63 秒。此后管理员再批准也**不会**自动
  接上：需要插件侧重新 `events.subscribe` 同一 topic（`onSettingsChanged` 那条订阅在
  激活时建立，等价于重载插件窗）才会重走一轮阶梯。退订会清掉待重试状态并回收宿主侧订阅，
  已退订的 topic 不会被残留定时器复活。
- **审批是 topic 粒度的，不是键粒度**：一旦某订阅者获批 `host:settings:changed`，
  宿主就会把该 topic 的全部帧投给它（SDK 只过滤**投递给钩子**的那一份，这是应用层
  约定，不是宿主级 ACL）。需要「按键授权」时得按新特性立项，别把 SDK 过滤当安全边界。
- **`event` 通道可丢**：慢消费者按 topic 丢最旧（队列受条数与字节双预算约束），所以
  这个钩子用于「响应最新值」，**不能**当变更流水账回放。要权威值仍用 `host_settings_get`。
- **等值写入不产生变更帧**：写入与当前值相同会被视为无操作回落，不镜像。
- **字节预算**（R3-5）：单个设置值上限 64 KiB（超限得 `ValueTooLarge`，在 schema 校验
  **之前**拒绝，零副作用）。丢弃发生在**两条不同的队列**上，别混着算：
  消息面投递用的是 EventBus 的 `(订阅者 × 通道)` 队列——1000 条 / 2 MiB 双上限，
  单帧上限 256 KiB；`SettingsStore` 自己的 Rust 观察队列是另一套 1 MiB 上限，
  只在宿主进程内嵌入方生效。
- **线上只有这一条设置通知线**：`SettingsStore::watch/drain` 是 **Rust 嵌入方的扩展点**
  （`host_settings_watch` 这类线上命令**不存在**，插件侧别去找它）。它与消息面镜像
  挂在**同一个提交口**（适配器的 `commit_settings_change` → `publish_committed_change`），
  所以宿主内即使有人用 Rust watcher，也**不会**和插件收到的帧分叉出两份事实；
  两条路各拿各的队列，一边溢出不会饿死另一边。
- **迁移失败不会吃掉你上一次的设置（A101），但恢复出来的是迁移前的那一版**。
  `host_settings_migrate` 在契约要求快照时，会把**迁移前**的用户层密封成
  `host-settings.rollback.json`（信封 schema `tauron.host-settings-rollback/1`），并且
  **先写镜像、再写正式文档**——顺序反了就会出现"已经迁了，却没有任何东西能回滚"。
  这份镜像是**一次性**的：① 下次启动时若正式文档校验不过，装配会消费它并立刻删除；
  ② 迁移落盘失败、内存 rewind 之后同样删除。所以插件作者要预期的边界是——
  走恢复路径启动后，`host_settings_get` 读到的是迁移前的值，宿主**不会**自动重跑迁移
  （`host_settings_migrate` 只在主窗显式调用时执行），需要管理员重新点一次迁移才回到当前
  schema。别把"能读到值"当成"迁移已完成"。

---

## 宿主命令面

命令面分两层。**档位强制不在统一 dispatcher 里**——每条命令的档位语义在
**各自的 wire 入口**落地（self 档从 webview label 解析身份、privileged 档限主窗），
登记表（`authz::COMMANDS` / `ADMIN_COMMANDS`）的强制兑底是**门禁测试**
（`validate_command_registry` + TS 镜像 `capabilities.ts`）。

### 插件面命令（20 条，登记在 `authz::COMMANDS`）

**Self_（18 条）**——身份取自 webview label（`plugin-<id>`），入参里的身份字段一律忽略（防冒充）：

| 命令 | 说明 |
|---|---|
| `host_plugin_call` | C/D 类插件的 JS↔宿主调用往返（取代信封式 `host_call_begin`） |
| `host_call_end` | 流式调用终帧确认（**仅该调用的发起方**可结束，见下） |
| `host_cancel` | 取消调用，取消传播到 sidecar / supervisor（**仅该调用的发起方**可取消） |
| `host_lifecycle_report` | 上报生命周期事件（state / reason） |
| `host_contributes_register` | 注册 contributes（commands / menus / panels / …） |
| `host_contributes_reconcile` | 对账本插件已注册的贡献（检出漂移并如实上报，0.4-W3） |
| `host_capabilities` | 拉取宿主真实命令面与域可用性（能力协商入口，R1-4 fail-closed 的真相源；无身份参数，任何主体可读） |
| `host_events_publish` | 事件发布唯一入口（越界丢弃 + 计数） |
| `host_events_subscribe` | 事件订阅（跨插件订阅需对方 `public: true`） |
| `host_events_unsubscribe` | 事件退订（窗口销毁时由宿主回收） |
| `host_events_drain` | 拉取本插件待投递帧（只取自己订阅的可见集） |
| `host_stream_open` | 为一次已挂帧载体的调用开流 |
| `host_stream_write` | 写一帧（seq 由宿主铸） |
| `host_stream_grant` | 补充流的有界 byte credit（背压窗口，接收方消费后补给，见 `authz::COMMANDS` 登记） |
| `host_stream_close` | 发终帧并使句柄失效 |
| `host_call_plugin` | 跨主体调用：宿主调插件或插件调插件（caller / target 显式，见[跨主体调用](#跨主体调用04-a1)） |
| `host_call_result` | 执行方回填一次调用的结果（仅 target 可回填） |
| `host_call_take` | 发起方取走一次已结算的结果（仅 caller 可取） |

> **"self 档 = 只能作用于调用者自己"的三条落地点**（改这些命令时不要只看表）：
> `host_call_end` / `host_cancel` 会先取 pending 条目并比对**归属主体**
> （`pending.pluginId`，即发起方；主窗发起的调用归属键为 `"main"`），主体不符一律
> `E_AUTH_DENIED` 且**零副作用**（调用与它的流纹丝不动）；`host_call_result` 只认
> 执行方（`target`），`host_call_take` 只认发起方（`caller`）。
> 唯一例外是 `host_events_unsubscribe`：它的凭据是 `subscribe` 返回的**不可猜
> token**（只回给订阅者本人），属能力 token 模型而非 label 绑定。

**ScopedRead（2 条）**——按身份过滤结果集而非拒绝：

| 命令 | 说明 |
|---|---|
| `host_registry_list` | 列出可见插件（结果按可见性过滤） |
| `host_contributes_list` | 列出贡献表（commands / menus / panels / settings，纯只读） |

### 主窗特权命令（8 条，登记在 `authz::ADMIN_COMMANDS`）

**Privileged（8 条）**——仅主窗 / 宿主 UI：判定都落在 `require_main_window`，
其中 4 条会改状态 / 授权的（`host_registry_admin` / `host_runtime_spawn` /
`host_events_approve` / `host_events_revoke`）经分发唯一咽喉 `admin_gate`
（判定 + `record_admin_audit` 留痕），另 4 条只读、直接判、刻意不入审计表。
**判定只有代码这一层**（Tauri ACL 不按 `host_*` 命令名管辖，见下节更正）：

| 命令 | 说明 |
|---|---|
| `host_registry_admin` | 管理操作：`disable` / `enable` / `uninstall` / `purge` |
| `host_runtime_spawn` | 启动进程插件 sidecar（幂等：已有租约则返回既有 pid / lease） |
| `host_runtime_health` | 按租约查询 sidecar 健康（pid / 崩溃窗口计数；暴露 PID 故同属特权） |
| `host_resource_stats` | 查看全局及逐插件的 pending、流、订阅与通知配额占用 |
| `host_events_approve` | 审批一条「订阅者 × 主题」的 Event 决策（主题必须已被声明，否则 `E_AUTH_DENIED`；审批表上限 4096 条，满则 `E_SUBSCRIPTION_FULL`） |
| `host_events_revoke` | 撤销一条 Event 审批。**撤销即失效**：该 `(subscriber, topic)` 的既有订阅当场全部退订、三个通道队列里该 topic 的待取帧一并作废；返回 `true/false` 只回答「有没有真的删掉一条授权事实」（幂等重试为 `false`，但**效力照走**） |
| `host_events_approvals` | 只读列出当前全部审批事实（宿主审批 UI / 审计对账用） |
| `host_production_doctor` | 读取机器可读的 production readiness 自检报告（部署模式 + 逐项检查 + `productionSafe` 汇总；见下文「生产就绪自检」） |

### 插件安装命令（2 条，feature-gated 且为 **opt-in**）

`host_registry_install_preview` / `host_registry_install` 挂在 `plugin-install` feature 下，
而该 feature 是 **opt-in**：`crates/tauron-adapter/Cargo.toml` 的 `default = []`（V4
minimal-substrate 规则——只依赖 `tauron-adapter` 的底座接入方不得被拉进
market/signature/archive 依赖）。因此**默认装配只有 83 条**，接入方显式
`features = ["plugin-install"]` 才是 85 条；示例应用
（`examples/minimal-app/src-tauri/Cargo.toml` 的
`default = ["plugin-install", "runtime-wasm-broker"]`）就属于显式开启那一类。

> 1.0-W6 曾把该 feature 放进默认特性，1.1 的 V4 合并按 minimal-substrate 规则改回
> opt-in。看到旧文档写「已进默认特性」时以 `Cargo.toml` 为准。

| 命令 | 说明 |
|---|---|
| `host_registry_install_preview(packagePath)` | 预览安装：解析包、校验签名与 hash、列出需用户逐条确认的权限；**不改动注册表** |
| `host_registry_install(packagePath, approvedPermissions)` | 执行安装：在**显式权限批准**后落盘并注册；`packagePath` 必须落在 `AdapterConfig.plugin_install_dir` 之下 |

> **两条都不在 `authz::COMMANDS` / `ADMIN_COMMANDS` 表内**：它们由 `plugin-install`
> feature 门控 + `plugin_install_dir` 配置门控 + 主窗身份判定三重把关。
> **`plugin_install_dir` 未配置时安装明确不可用**（返回带原因的失败，不静默成功）。
> 它们既不是插件可触达的命令面，也不参与 TS `CAPABILITIES` 的 1:1 镜像。

**「落在 `plugin_install_dir` 之下」怎么判**（轮 16/17 定的口径，接入方传路径时要按它写）：
装配期对 `plugin_install_dir` 取 canonical 副本，取径侧（`host_window_create` 反推插件
UI 资产相对路径）在比较点再 canonicalize 一次，两侧同形才比。因此 `..` 段、符号链接、
Windows 8.3 短名这类**真的换目录**的写法会被拒（`E_INSTALL_FAILED`，不静默回退）；
`<root>/./plugins` 这种**多一个 `.` 段**的写法不算失配——`Path` 按组件比较会忽略 `.`。
目录在装配时还不存在（首装）则按原样保留该值，判定仍由取径侧的 canonicalize 兜住。

### 底座命令（主窗专属）

其余命令（`host_window_*` / `host_settings_*` / `host_notify` / `host_recover_*` /
`host_market_*` / `host_i18n_*` / `host_brand_info` / `host_registry_list_all` 等）
**不在档位表内**——它们是主窗 / 宿主 UI 专属面，判定落在**代码层**：需要判定的那些
在 wire 入口调 `require_main_window`（会改状态的特权族再包一层咽喉 `admin_gate`，判定 +
审计同点），
`host_settings_*` / `host_notify` 等按身份绑键空间或过滤可见集合。

> ⚠️ **旧口径在此更正（轮 12）**：本节曾写「由 Tauri capability 的 `windows` 字段限制
> （只授予 `main`）」——示例工程的能力文件 `windows` 是 `["main", "plugin-*"]`、
> `permissions` 只有 `core:default`，而 `host_*` 是 **root 注册**（裸名），Tauri ACL
> 并不按命令名管辖它们；`origin_gate` 判的也只是 label + origin 的**形态与来源**，
> 不是「这条命令谁能调」。**代码层判定是按命令的唯一权威**，因此这条面上漏判定
> 就是真越权：轮 12 实测把 9 条宣称「主窗专属」、实则零判定的命令补成
> `require_main_window`（`host_window_quit`、`host_clipboard_read` / `_write`、
> `host_dialog_open` / `_save` / `_message` / `_confirm`、`host_recover_boot`、
> `host_i18n_stats`）。
>
> 另有 **9 条**经复核**刻意不带**身份判定（`host_brand_info`、`host_i18n_t` /
> `_t_params`、`host_window_close` / `_maximize` / `_minimize` / `_restore` /
> `_set_position` / `_set_size`）：前 3 条只读且只返回调用方自身可见的信息（几何族的
> 目标窗口由注入的 caller label 决定，插件只能作用在自己窗口），逐条理由写在各命令的
> `///` 注释里，并由 `wire-gate` 按**名字**钉住——新增命令想悄悄加入这一清单即红。

> **全量对照看 [`docs/api/command-surface.md`](./command-surface.md)**：85 条命令的
> 业务形参、返回类型、feature 门、函数体里**实际执行**的 `require_*` 判定、以及
> 前端落点（哪个 package 调它）由 `pnpm command-surface:gen` 从代码生成，
> CI 用 `pnpm command-surface:check` 复算——「代码加了命令、文档没跟上」会被直接拦下。
> 本节的表只列**档位表内**的命令（插件面 22 + 特权 8 + 安装 2），
> 关键命令的线格式与错误契约在 `docs/architecture/app-layer-wire.md` 第 3 节
> （那一节按口径只写**关键**命令，不是全量清单）。

> **`host_call_begin` / `host_grant_request` 是故意移除的命令**，回归会被
> `packages/tauron-host/src/gates.test.ts` 拦下。

### 生产就绪自检与 Event 审批（宿主 UI 侧，1.1 新增）

这 4 条（1.1 新增）特权命令的 TS 入口都在 `@tauron/host` 的 `AdminClient`（与
`registryAdmin` 同一个类），**插件侧不可调用**（非主窗一律 `E_AUTH_DENIED`）：

```ts
import { AdminClient } from '@tauron/host';

const admin = new AdminClient({ backend });

// 1) 生产就绪自检（A109）：机器可读的逐项 readiness 报告。
//    wire JSON 为 camelCase；deploymentMode 取 'development' | 'test' | 'production'。
const report = await admin.productionDoctor();
if (!report.productionSafe) {
  // 只列 requiredInProduction 且未通过的项即可定位缺哪块配置
  console.warn(report.checks.filter((c) => !c.pass).map((c) => `${c.id}: ${c.message}`));
}

// 2) Event 审批三件套：审批事实的唯一落点是宿主，插件只能声明/订阅。
await admin.eventsApprove('com.example.viewer', 'plugin:com.example.formatter:tick');
const rows = await admin.eventsApprovals(); // → [{ subscriber, topic }, …]
const revoked = await admin.eventsRevoke('com.example.viewer', 'plugin:com.example.formatter:tick');
```

语义与边界：

- `eventsApprove` 的 `topic` **必须已被某个插件声明**，否则 `E_AUTH_DENIED`——
  审批不会为不存在的主通制造孤儿事实；
- 审批表有上限（`MAX_APPROVALS = 4096`），达限返回 `E_SUBSCRIPTION_FULL`
  （不确定失败，不可自动重试），先 `eventsRevoke` 失效项再审批；
- `eventsRevoke` 的**效力**（A81，轮 11 起）不止于「删一行授权」：对「他人声明且非公共」
  档，它同时 ① 退订该 `(subscriber, topic)` 的全部既有订阅、② 作废 pending/stream/event
  三类通道队列里该 topic 的待取帧。公共 / 自属 topic 的订阅不归审批表管，撤销一条冗余审批
  **不会**掐掉它们（与 `subscribe` 的授权判定同一口径）。返回的布尔值只表示是否真删了
  一行——幂等重放（返回 `false`）仍会再作废一次队列，管理面因此**可以用重放封住并发尾巴**：
  与撤销并发、且在撤销前就已把 token 解析成订阅者的那次 `publish`，仍可能在作废之后落一帧，
  再调一次 `eventsRevoke` 即无残留。要在发布侧封死该窗口需把审批重检放进队列临界区，
  已作为代价登记在 `docs/architecture/v4-industrial-gap-closure-plan.md` A81 行；
- `productionDoctor` 是**只读**诊断：不写状态、不落盘，可在启动期安全轮询；
  报告内容由 `AdapterConfig` 的实际配置推导（fail-closed 与启动门同源，
  不会出现「自检通过但启动拒绝」）。
- `productionSafe` **只在 `deploymentMode === 'production'` 且全部检查项通过时才为
  `true`**——开发态/测试态即使项项全绿也如实报 `false`，别把它当「没问题」读。

#### `productionDoctor().checks` 的 9 个检查项（全部 `requiredInProduction: true`）

`id` 是稳定契约（wire camelCase），逐项来自
`tauron_host::production::doctor`，宿主无法自行声明其中任何一位：

| `id` | 通过条件（由配置推导） | 对应的启动门 |
|---|---|---|
| `caller-identity` | 声明了身份策略，**或** origin 清单非空 | `CALLER_IDENTITY_POLICY_REQUIRED` |
| `origin-gate` | **仅** `origin_allowlist` 非空 | `ORIGIN_GATE_ARMED_REQUIRED` |
| `recovery-durability` | 配了持久化恢复，或显式声明不支持 | `RECOVERY_DURABILITY_REQUIRED` |
| `install-trust` | 未开 `plugin-install` 即视为通过；开了就必须配齐信任材料 | `INSTALL_TRUST_REQUIRED` |
| `trusted-time` | 同上：开了安装才要求可信时间源 | `TRUSTED_TIME_REQUIRED` |
| `admin-audit` | 审计 sink 的 `AdminAuditFacts::healthy()`（落盘 ∧ 链完整 ∧ 零写失败） | `ADMIN_AUDIT_REQUIRED` |
| `durable-data-dir` | 可写的持久数据目录 | `DATA_DIR_REQUIRED` |
| `process-sandbox` | 未接进程运行时即视为通过；接了就要 `Hard` 级沙箱 | `PROCESS_SANDBOX_HARD_REQUIRED` |
| `no-mock-provider` | 没有启用 mock/测试 provider | `MOCK_PROVIDER_FORBIDDEN` |

> 想核对某一项为什么红：`message` 是固定英文短句，`pass` 之外还要读
> `requiredInProduction`。**不要**按检查项的个数写死逻辑（清单会随能力增长变长），
> 按 `id` 取。

#### `admin-audit` 检查项与审计事实（轮 11 / F3 新增）

`productionDoctor()` 除 `checks` 外还带一个 `adminAudit: AdminAuditFacts | null`
快照（`packages/tauron-host/src/host.ts`），它是**读取侧的独立证据**：
`admin-audit` 检查项不是宿主自报的布尔位，而是从活的审计 sink 现算出来的。

```ts
const report = await admin.productionDoctor();
const audit = report.adminAudit; // null = 宿主根本没配 sink（Production 会被拒启）
if (audit && !audit.durable) {
  // 记录仍在内存里，进程退出即丢：检查项红，productionSafe 随之为 false
}
if (audit && audit.writeFailures > 0) {
  console.error(audit.lastWriteError, audit.lastCommand, audit.lastOutcome);
}
```

语义与边界：

- 被打审计的是 `AUDITED_ADMIN_COMMANDS` 这 8 条特权命令：`host_events_approve`、
  `host_events_revoke`、`host_registry_admin`、`host_registry_install`、
  `host_registry_install_preview`、`host_runtime_spawn`、`host_market_download`、
  `host_market_install`（后两条为轮 40 增补：装配了真实 `UpgradeInstaller` 时带真实副作用；
  `eventsApprovals` 是只读列出，**不**落审计），**允许与拒绝都留痕**
  （`lastOutcome` 取 `'allowed' | 'denied'`）；
  新增特权写操作若忘记登记，wire-gate 会与判定代码对账报红；
- 记录是**哈希链**（每条带 `prevHash`），落盘文件为 `admin-audit.json`，环形上限
  `MAX_ADMIN_AUDIT_RECORDS = 512`；`records` 是当前保留条数、`totalRecorded` 含被裁剪的；
- 离线复核用 `tauron_host::admin_audit::verify_file`（运维/取证读，不经宿主），
  宿主侧只暴露事实快照；
- 因此 `with_admin_audit(bool)` 这类「声明我有审计」的开关**已删除**——唯一满足方式是
  在 `AdapterConfig.admin_audit_dir` 给一个真实可写目录，缺它则 Production 启动即
  `ADMIN_AUDIT_REQUIRED`。

#### `origin-gate` 检查项（1.1 / 轮 10 新增）

`productionDoctor().checks` 里有一项 `id: 'origin-gate'`，它问的**不是**
「宿主有没有声明身份策略」，而是「命令分发处的 origin 门有没有真的装弹」——
即 `AdapterConfig.origin_allowlist` 是否非空。两者是**两个独立事实**：

| 事实 | 由什么满足 | 是否给 origin 门装弹 |
|---|---|---|
| `caller-identity` | `caller_identity_policy_enabled`，**或**非空 `origin_allowlist` | ❌ 显式声明不算（非 origin 传输也能声明它） |
| `origin-gate` | 仅 `origin_allowlist` 非空 | ✅ 唯一装弹途径 |

所以「声明了 caller identity 但清单为空」的 Production 宿主：`caller-identity`
绿、`origin-gate` 红、`productionSafe: false`，且启动被
`ORIGIN_GATE_ARMED_REQUIRED` 拒绝。这不是文档口径，是
`AdapterConfig::production_readiness()` 里由 `!origin_allowlist.is_empty()`
直接推导的（宿主**无法**伪造这一位）。

#### Production 下 origin 门的拒绝原因（命令面行为，插件作者需要知道）

`DeploymentMode::Production` 时，每一条 `host_*` 命令在进入 handler 前都要过
`tauron_host::authz::production_caller_allowed`。它只会以下面四种原因拒绝，全部
`E_AUTH_DENIED`、`retryable: false`、message 里点名原因：

| 原因串 | 触发条件 | 宿主侧修法 |
|---|---|---|
| `ORIGIN_GATE_NOT_ARMED` | `origin_allowlist` 为空 | 配清单（见下） |
| `CALLER_IDENTITY_INVALID` | webview label 形态非法（如 `plugin-` 前缀后缺 id） | 别自造 label |
| `MAIN_WINDOW_LABEL_NOT_DECLARED` | 调用方不是插件、其 label 又不在 `main_window_labels` 里 | 声明主窗 label |
| `ORIGIN_NOT_ALLOWED` | label 合法但 webview 真实 origin 不在清单内 | 把该 origin 写进清单 |

关键差别（相对开发态）：**未声明的 label 不再自动等同主窗**。开发态/测试态保持
原有的兼容语义（空清单 = 不启用，非插件 label = 主窗）。主窗 label 集合由
`AdapterConfig::main_window_labels` 声明，留空时装配方展开为 Tauri 约定缺省
`["main"]`——因此多窗宿主（`editor`/`settings` 之类的窗口想调宿主命令）**必须**
显式声明，否则在 Production 会被 `MAIN_WINDOW_LABEL_NOT_DECLARED` 拒掉。

```rust
// src-tauri 侧装配（宿主特权，插件看不到也改不了）
let cfg = tauron_adapter::AdapterConfig {
    // 非空即装弹：这一位同时推导 readiness 的 origin_gate_armed。
    origin_allowlist: vec!["tauri://localhost".into()],
    ..tauron_adapter::AdapterConfig::production()
};
let cfg = cfg.with_main_window_labels(vec!["main".into(), "editor".into()]);
tauron_adapter::init_with_adapter_config(cfg);
```

> 判定素材全部取自宿主侧（`Invoke` 由 Tauri 在分发时构造的真实 command 名、
> 真实 webview label、真实 URL 的 origin）。**前端自报的任何 origin/label 都不参与
> 判定**，`origin` 字段在插件侧参数里出现也不被读取。

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

### 应用层 `E_*`（24 个，`tauron-host`）

变体名即**跨 IPC 线协议名**（改名即破坏兼容）。TS 侧 `HOST_ERROR_CODES` 与 Rust
枚举、canonical 注册表按**码名集合**比对——**声明顺序不属于协议**（V4 A69，
门禁 `线名集合与 canonical registry 一致；source declaration order 不属于协议`）。

| 码 | 触发点 |
|---|---|
| `E_HOST_PANIC` | handler panic，经 `catch_unwind` 归一化（`retryClass: never`——panic 可能已留下部分副作用，自动重放不安全） |
| `E_UNKNOWN_PLUGIN` | 未知 `plugin_id` |
| `E_AUTH_DENIED` | 档位不满足，或身份被伪造 |
| `E_INVALID_MANIFEST` | **入参/声明不合格**（宿主的通用"改载荷即可"码，见下方说明）：清单未知字段、非法 id、权限表外字符串、缺 `framework`、`abi` 字段缺失/非法；**同样用于**非清单入口——JS 插件缺 `entry.ui`、UI 路径非法、进程 spawn 配置不合、设置文档非对象或违反 schema；**轮 30 起**还用于进程调用**帧本身发不出去**——超过 `MAX_FRAME_BYTES` 上界或序列化失败（改载荷才有用，退避重投无用，所以不是 `E_CALL_PENDING_FULL`） |
| `E_STATE_INVALID_TRANSITION` | 状态机无匹配规则（非法迁移）。**轮 30 起不再用于进程投递的写侧失败**——那条路径上状态机已经放行，报本码是对系统的假陈述（分流见 `E_CALL_PENDING_FULL` / `E_LEASE_EXPIRED` / `E_INVALID_MANIFEST` 三行） |
| `E_CALL_NOT_FOUND` | pending call 不存在或已结束 |
| `E_CALL_TIMEOUT` | pending call 超时（`retryClass: manual`——人工决定是否重发，宿主不自动重放） |
| `E_FORBIDDEN_PERMISSION` | 申请了禁止授予清单内的权限 |
| `E_ABI_MISMATCH` | `host_runtime_spawn` 的 ABI 契约比对失败（见下节） |
| `E_PLUGIN_DISABLED` | 插件已禁用（含崩溃预算耗尽） |
| `E_REGISTRY_FULL` | 注册表达到活跃身份上限（缺省 8） |
| `E_CALL_PENDING_FULL` | pending call 表达到上限；**或** sidecar 的 pid 写队列已满（32 帧未消化，轮 30）。两道界对调用方是同一处置——"额度满了，退避后重投"，所以同码；具体是哪一道由 message 点名（`pending call 容量…` / `写队列已满…`），排查方向不同：前者要取件腾位，后者要等 sidecar 追上 stdin |
| `E_SUBSCRIPTION_FULL` | 订阅表达到上限 |
| `E_PLUGIN_EXISTS` | 插件已存在（重复注册） |
| `E_INSTALL_FAILED` | 安装期失败：验签 / hash / 解包 / range |
| `E_PLUGIN_FILTERED` | 被配置过滤器排除（改配置后人工重试，`retryClass: manual`） |
| `E_PLUGIN_TYPE_NO_RUNTIME` | 该插件类型没有运行期执行器（如对 js/wasm 插件调 `host_runtime_spawn`） |
| `E_LEASE_EXPIRED` | 运行时租约不存在或已失效（`retryClass: after-reconnect`：先重建会话再重新 spawn）。**轮 30 起**还包括投递时 stdin 通路已不在（`NotFound` / `BrokenPipe`：sidecar 已退出或写线程因管道断裂收摊）——与"回帧来自旧代次"是同一件事，所以同码 |
| `E_STREAM_FULL` | 流句柄数达到上限（宿主 `MAX_STREAMS = 256`，单插件 `MAX_STREAMS_PER_PLUGIN = 32`）——先 `host_stream_close` 再开 |
| `E_CALL_ALREADY_SETTLED` | 执行方对同一次跨主体调用重复回填（0.4-A1；重复回填显式拒绝，不覆盖） |
| `E_CONTRIBUTES_DRIFT` | 贡献对账分叉：manifest 声明的扩展点与 activate 期实际注册的不一致（0.4-W3；报错并点名缺哪条） |
| `E_STREAM_BACKPRESSURE` | 生产方用尽了接收方授予的 byte credit 窗口（`retryClass: manual`）——**被拒的那一帧不会投递，也不消耗 seq**，补给 credit 后可原帧重发 |
| `E_CALL_CYCLE` | 同步调用图会成环、重入同一主体，或超出有界的跳数预算 |
| `E_EVENT_CAUSATION_LIMIT` | 事件因果链超出有界的深度预算 |

**重试语义：`retryClass`，不是 `retryable`。** 四档 kebab-case 线名
`never` / `manual` / `auto-idempotent` / `after-reconnect`，与 Rust
`ErrorCode::retry_class()` 逐项同集合，TS 侧用 `HOST_RETRY_CLASS` /
`retryClassOf(code)` 查。今天的真实分布只有三档有用：
`manual`（`E_CALL_TIMEOUT` / `E_PLUGIN_FILTERED` / `E_STREAM_BACKPRESSURE`）、
`after-reconnect`（`E_LEASE_EXPIRED`）、其余一律 `never`；
`auto-idempotent` **一个码都没落**——它是留给"已证明幂等的操作"的位置，不等于许可。

遗留兼容位 `retryable` 恒为 `false`（`retryable() == retry_class() == auto-idempotent`），
TS 的 `RETRYABLE_HOST_ERROR_CODES` 同为空集（两侧由门禁锁定）。
V4 刻意不把 panic / 超时标成可自动重放：`catch_unwind` 只是**遏制**，
不是"没有部分副作用"的证明。**分流请读 `retryClass`。**

**`E_INVALID_MANIFEST` 的复用约定**：适配器**不新增错误码**——
新增一个 `E_*` 必须同时改 Rust 枚举、`Display`、TS `HOST_ERROR_CODES`
与 canonical 注册表 `contracts/error/error-codes.json`（四处按**码名集合**比对，
声明顺序不属于协议，V4 A69），而"入参不合格"这一族失败对调用方的动作完全相同
（**改载荷，别重试**）。因此 spawn 配置、JS entry、设置文档等非清单入口
也复用本码；差异放在 `message` 里（面向日志，不作分支依据）。

### 跨 iframe 桥的保码行为（轮 35）

插件在 iframe 里 `await ctx.invoke(...)` 失败时拿到的是 **`PluginBridgeError`**
（`@tauron/plugin-sdk` 导出），不是裸 `Error`：

| 字段 | 含义 |
|---|---|
| `code` | **线上原样**的码：宿主能力抛的 `E_*`，或插件 handler 自己抛的 `SC-####` |
| `message` | 桥送回来的原文（面向日志，**不得**作为分支依据） |
| `retryable` | 只对 `SC-####` 按 `RETRYABLE_ERROR_CODES` 判定；`E_*` 一律 `false` |
| `fallbackApplied` | `true` = 原始失败**没有任何可用码**（或码不是合法形态），本错误落到 `SC-9001` |

写侧（宿主窗口里的 `PluginBridge`）取码顺序：① 错误对象自带的 `code`——**形态合法才算**，
`code: 500` 这类业务字段不会被当成码；② 消息文本里的码形态；③ 两处都没有才落 `SC-9001`。
本地发起的失败同样带码：超时 `SC-2001`、取消 `SC-2002`、上下文销毁 `SC-2004`、
权限未授予 `SC-1002`。

形态判据（`E_` 前缀 / `^SC-\d{4}$`）**只有一份**，定义在 `@tauron/types`
（`isCodeLike` / `isAppLayerErrorCode` / `extractCodeLike` /
`APP_LAYER_ERROR_CODE_PATTERN`），`@tauron/host` 侧只做再导出。第二份正则会随时间
漂移（一侧放宽一侧没放宽），那才是真断链。符号名里的 `APP_LAYER_` 前缀是 1.0 公开面
遗留命名：按 [架构总览](../architecture/overview.md) 的层口径，`SC-####` 属**框架层**
（`tauron-shell` ↔ `@tauron/types`），`E_*` 才是**应用层**宿主底座（`tauron-host`）。

> **为什么 `E_*` 的 `retryable` 保持 `false`**：`@tauron/plugin-sdk` **不**依赖
> `@tauron/host`（依赖方向——否则整座宿主客户端会打进 iframe 产物），所以它拿不到
> `HOST_RETRY_CLASS` 那份表。这里宁可返回 `false` 也不猜。要按宿主 `retryClass` 四档
> 分流，两种做法：宿主侧判好后把结论放进返回给插件的字段，或插件自带 `@tauron/host`。
> `retryable` 只是框架层词表的便捷位，`code` 才是分流依据。

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

### 词表：哪份状态名在线上

仓库里有**三份**插件生命周期词表，只有一份是线上事实：

| 词表 | 位置 | 名字形态 | 能不能喂给宿主 |
|---|---|---|---|
| **线名（唯一权威）** | Rust `tauron-host::lifecycle::State::as_str()`，TS 镜像 `@tauron/host` 的 `LIFECYCLE_STATES` | `RUNNING` / `ERRORED_RETRYABLE` / `ERRORED_USER_CONFIRM` / `INSTALL_FAILED` / `UNINSTALLED` … | ✅ 宿主上报/回读都用它 |
| §4.3 设计模型名 | `@tauron/types` 的 `PluginState` / `TRANSITIONS` | 与线名只重合 5 个；`ENABLING` / `DISABLING` / `ERRORED` / `UPGRADING` / `UNINSTALLING` **不在线上** | ❌ 只是设计模型，宿主按线名反序列化，写上去必失败 |
| mock 小写名 | `@tauron/app-contract-kit` 的 `MockRegistry.getState()` | `'installed'` / `'enabled'` / `'disabled'` / `'errored'` | ❌ 仅测试替身内部用；要线名请调 `getWireState()`（映射表 `MOCK_STATE_TO_WIRE`） |

后两份都已发布（1.0.x），收敛它们与线名的分歧属破坏性变更，需单独批准；在此之前，
`@tauron/contract-tests` 的门禁把**三份词表的重合与分歧逐名钉死**——任一侧改名、或把
mock 的映射表指到一个不存在的线名，CI 立即红（`wire-gate.test.ts` 的「轮 60」用例）。

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
| `publish` | 校验清单形状 | **不上传**、不返回商城 URL（`published: false` / `simulated: true`），**退出码非 0** |

`publish` 的三条分支（缺 `tauron.plugin.json` / 缺 `.tgz` / 有包但无上传客户端）
在轮 16 之后**一律给非 0 退出码**：此前前两条返回 `success: true`（exit 0），
`tauron plugin publish && 下一步` 会在什么都没发布时继续往下走。

需要完整发布链路时，请用你自己的流水线打包，再对产物跑 `sign`，最后交给
`tauron-app` 侧的商城客户端（`@tauron/market`，Ed25519 验签）。
