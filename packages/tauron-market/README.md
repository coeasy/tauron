# @tauron/market

插件市场 —— Ed25519 签名、注册表管理。

## 安装

```bash
pnpm add @tauron/market
```

## 使用

```typescript
import { generateKeyPair, sign, verify, createRegistry } from '@tauron/market';

const { privateKey, publicKey } = generateKeyPair();
const { signature } = await sign(data, privateKey);
// 验证用**公钥**（Ed25519 非对称签名：私钥签发、公钥公开验证）
const isValid = await verify(data, signature, publicKey);
```
