# tauron 插件开发指南

本文档提供 tauron 插件开发的完整指南。

## 目录

- [概述](#概述)
- [环境准备](#环境准备)
- [创建插件](#创建插件)
- [插件类型](#插件类型)
- [插件结构](#插件结构)
- [插件 SDK](#插件-sdk)
- [权限系统](#权限系统)
- [测试插件](#测试插件)
- [打包与发布](#打包与发布)

---

## 概述

tauron 插件系统在**清单层**定义了四种插件类型。下表第三列是**今天真实成立**的隔离手段，
不是设计意图——本仓库的规矩是"没接线就写没接线"（方案 §9-9）：

| 类型 | 语言 | 今天的真实隔离手段 | 今天能跑吗 | 适用场景 |
|---|---|---|---|---|
| **JS** | JavaScript/TypeScript | **身份 Webview**：插件跑在自己的 webview 里，label 恒为 `plugin-<插件 id>`；宿主命令按身份判定（`Caller`），越权调用的主体在**副作用之前**被拒 | ✅ 能（这是 B 类的默认形态） | UI 扩展、轻量逻辑 |
| **WASM** | WebAssembly | **无**（未接线） | ❌ **不能**：以 WASM 类型启动运行时返回 `E_PLUGIN_TYPE_NO_RUNTIME`（**诚实失败**，不会静默降级成"跑起来了"） | 计算密集、跨平台（**待接线**） |
| **Process** | Rust/C/C++/Node.js | **独立进程**：sidecar spawn 有健康检查与崩溃事件 | ⚠️ 能（有边界，见下） | 系统操作、原生集成 |
| **B+** | JS + Host | **无**（`@tauron/dual-world` 的进程内 JS 沙箱未接运行时，调用一律 fail closed） | ❌ 不能：`SANDBOX_UNAVAILABLE`，**不会**伪报 `executed: true` | 需要宿主能力的插件（**待接线**） |

**必须说清的两点**（此前本文件在这里写成"QuickJS-WASM 沙箱 / WASM 沙箱 / 双世界隔离"，
是**未兑现的宣称**，轮 12 改判）：

1. **JS 插件的隔离来自 webview 边界 + 身份判定，不是来自进程内沙箱**。宿主对 54 条命令
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

---

## 插件类型

### JS 插件

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

---

## 插件结构

### package.json

```json
{
  "name": "com.example.myplugin",
  "version": "1.0.0",
  "type": "js",
  "main": "src/index.js",
  "permissions": ["store:read", "http:fetch"],
  "devDependencies": {
    "@tauron/plugin-sdk": "^0.1.0"
  }
}
```

---

## 权限系统

| 权限 | 说明 |
|---|---|
| `store:read` | 读取存储 |
| `store:write` | 写入存储 |
| `http:fetch` | 发起 HTTP 请求 |
| `fs:read-app-dirs` | 读取应用目录 |
| `fs:write-app-dirs` | 写入应用目录 |
| `clipboard:read` | 读取剪贴板 |
| `clipboard:write` | 写入剪贴板 |
| `shell:open` | 打开外部应用 |
| `notification:send` | 发送通知 |

---

## 测试插件

```bash
tauron plugin test    # ⚠️ 未实现：不执行任何测试，也不给出通过/失败计数
tauron plugin dev     # ⚠️ 未实现：不启动 dev server、不监听端口、不做 watch
```

**这两条命令目前都会如实返回 `success: false`**，并说明"未实现"。它们此前会返回
`success: true` 并声称"已启动 dev server（watch mode，port 8080）"、
"Running tests … passed = 测试文件个数"——**什么都没做**。测试请直接用 vitest/jest：

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
| `pack` | 读清单、按 `includes` 列文件、算总字节数 | **不写任何归档文件**（`package: null`） |
| `sign` | 读取产物、算真实 SHA-256 摘要、把 `.sig` **真的写出来** | 不做 Ed25519 非对称签名（要真签名用 `tauron-app plugin sign`，走 `@tauron/market`） |
| `publish` | 校验清单形状 | **不上传**、不返回商城 URL（`published: false` / `simulated: true`） |

需要完整发布链路时，请用你自己的流水线打包，再对产物跑 `sign`，最后交给
`tauron-app` 侧的商城客户端（`@tauron/market`，Ed25519 验签）。
