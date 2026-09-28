# @tauron/app-cli

tauron 应用 CLI（`tauron-app` 命令）：配置生成、主题生成、插件脚手架/打包/签名/发布、权限审计。

## 安装

> npm 与 crates.io 发布完成前，可在仓库内直接运行：

```bash
node packages/tauron-app-cli/dist/cli.js --help
```

下文用 `tauron-app` 代指它。

## 命令总览

```text
tauron-app new      创建新的 tauron 应用工程（真装配的 src-tauri + capability）；create 为别名
tauron-app init     一键接入现有 Tauri 项目
tauron-app doctor   环境自检
tauron-app client   config — 客户端本地配置
tauron-app theme    generate [--output <file>] [--css] 生成主题 JSON / CSS 变量
tauron-app plugin   new | create | dev | pack | sign | check | audit
```

所有命令支持 `--key=value` 与 `--key value` 两种参数语法。

### new / create —— 一键脚手架

```bash
tauron-app new ./my-app --name my-app                       # 也支持 create-tauron-app ./my-app
tauron-app new ./my-app --framework react --shell tauri
tauron-app new ./my-app --tauron-path ../tauron --dry-run   # 只报告产物、不落盘
```

正式发布后也可直接运行：

```bash
npm create tauron-app@1.0.1 -- ./my-app --framework react
```

生成的工程包含 `src-tauri/{Cargo.toml,src/main.rs,build.rs,capabilities/default.json,tauri.conf.json}`
与前端入口、`package.json`、`tsconfig.json`、`vite.config.ts`、根 `index.html`。
`src-tauri` 是 `examples/minimal-app` 同形态的**真装配**（`state_init_with_adapter_config` +
`tauron_generate_handler![]`，默认 80 条命令；`--features substrate-only` 走 57 条底座），
capability 覆盖 `main` 与 `plugin-*` 窗。

「一键」到 `npm run tauri dev` 的链路已闭合：`tauri.conf.json` 带 `beforeDevCommand` /
`beforeBuildCommand`（拉起 vite），`vite.config.ts` 的 `server.port` 与 `devUrl` 一致、
`outDir` 与 `frontendDist` 一致，`index.html` 位于工程根（vite 的 root 约定）。

依赖坐标：默认使用精确的 `1.0.1` registry 版本；贡献 Tauron 源码时，显式使用
`--tauron-path <相对路径|绝对路径>` 生成 `path` / `file:` 本地依赖。发布前，registry
安装暂不可用；发布状态见[安装与使用](../../docs/installation.md)。

> **需自行替换的部分**：`src-tauri/icons/` 是**纯色占位图**（6 个文件，覆盖 Windows / macOS /
> Linux 打包所需）——**发布前必须换成品牌图标**。不给这组文件连 `cargo check` 都过不去：
> `tauri-build` 在 Windows 上要 `icons/icon.ico` 才能生成资源文件。
>
> **实测（2026-09-27）**：生成物（含占位图标与 vite 配置）开箱即可编译——默认形态与
> `--features substrate-only` 两档 `cargo check` 均通过。
>
> **这条实测只覆盖 Rust 侧**：`npm install` / `npm run tauri dev` 未在本机验证（要从
> registry 取 `vite` / `@tauri-apps/cli`，沙箱无网络）。前置两条：本机 tauron 检出已
> `pnpm install`、且 `packages/*/dist` 已构建——`file:` 依赖是软链，`@tauron/host` 解析
> 自己的 `workspace:*` 依赖走的是检出根自己的 `node_modules`。

### init —— 接入现有 Tauri 项目

`tauron-app init --dir <你的项目>` 面向 Tauri 2，会加固定版本依赖、接线 Builder 链、补
Tauron capability 与缺少的 `build.rs` / `tauri-build` 依赖。现有权限配置与 client config 会保留。
若目标项目**已有** `.invoke_handler(..)`，它**不会**改动源码——Tauri 的 `invoke_handler` 是
**覆盖语义**，自动追加会丢掉你原有的命令——而是把需要手工合并的那一行如实打印出来。

常用参数：`--dir <path>`（目标项目根，缺省为当前目录）、`--tauron-path <path>`
（明确启用本地源码依赖）、`--config <file>`、`--dry-run`。支持边界见
[Tauron v1 支持范围](../../docs/integration/support-boundary.md)。

### plugin pack / sign

```bash
# 打包插件目录，产出标准 ZIP 格式 .tpkg 安装包
tauron-app plugin pack --dir ./my-plugin --output ./dist/my-plugin.tpkg

# 对安装包中的文件清单签名，产出 <package>.sig
# 私钥文件支持：PEM 文本（PKCS#8）或 hex 编码的 PKCS#8 DER
tauron-app plugin sign --file ./dist/my-plugin.tpkg \
  --key ./private-key.pem --algorithm ed25519 --kid tauron-001
```

签名**不在本包实现**：`pluginSign` / `verifySignatureCrypto` 调用同生态的
`@tauron/market`（其 `sign` / `verify` 是 Node `node:crypto` 的原生 Ed25519 实现），
因此 CLI 与市场签名算法不可能漂移。编码约定以 `@tauron/market` 为准：
密钥 = DER hex（CLI 可额外接受 PEM，仅做格式规范化），**签名 = hex**。

> 算法面诚实收窄：`@tauron/market` 当前只实现 `ed25519`；`rsa-2048` / `rsa-4096`
> 仍在配置校验白名单里（保持兼容），但签名会**显式失败**而不是伪造 `algorithm` 字段。

两个 API 都是 **async**（底层市场的 `sign` / `verify` 为 Promise）：

```typescript
import { verifySignature, verifySignatureCrypto, pluginSign } from '@tauron/app-cli';

verifySignature(sig);                              // 结构校验（同步；字段完整性/格式）
await verifySignatureCrypto(sig, publicKeyPem);    // 密码学校验（公钥，异步）
const result = await pluginSign(config, files);    // 签名为 hex（Ed25519）
```

> 密钥生成示例（Node REPL）：
> `crypto.generateKeyPairSync('ed25519', { privateKeyEncoding: { type:'pkcs8', format:'pem' }, publicKeyEncoding: { type:'spki', format:'pem' } })`

### plugin audit / check

- `audit`：读取 manifest，输出权限清单与高危权限（`*:allow-*` / `*:write`）报告
- `check`：manifest 结构与字段校验

### plugin publish

生成发布请求（目标仓库 URL + token）；网络上传由 CI/发布服务完成（CLI 不直接发起网络请求）。

## 编程式 API

打包/签名/发布逻辑可库式导入（`pack.ts` 模块）：
`pluginPack` `pluginSign` `pluginPublish` `verifySignature` `verifySignatureCrypto`
`validatePackConfig` `validateSignConfig` `collectPluginFiles` 等。

## 相关包

- `@tauron/types` — 配置 schema（`tauron.config.json` 校验）
- `@tauron/market` — 市场索引签名/验证（同为 Ed25519，node:crypto 原生实现）
