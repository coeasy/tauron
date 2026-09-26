# @tauron/app-cli

tauron 应用 CLI（`tauron-app` 命令）：配置生成、主题生成、插件脚手架/打包/签名/发布、权限审计。

## 安装

> ⚠️ **尚未发布到 npm**（本包当前 `private: true`）。在仓库内直接跑：

```bash
node packages/tauron-app-cli/dist/cli.js --help
```

下文用 `tauron-app` 代指它。

## 命令总览

```text
tauron-app init     生成 tauron.config.json 应用配置
tauron-app doctor   环境自检
tauron-app client   config — 客户端本地配置
tauron-app theme    generate [--output <file>] [--css] 生成主题 JSON / CSS 变量
tauron-app plugin   new | create | dev | pack | sign | check | audit
```

所有命令支持 `--key=value` 与 `--key value` 两种参数语法。

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
