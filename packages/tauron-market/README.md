# @tauron/market

插件市场 —— HMAC-SHA256 签名、注册表管理。

## 安装

```bash
pnpm add @tauron/market
```

## 使用

```typescript
import { generateKeyPair, sign, verify, createRegistry } from '@tauron/market';

const { privateKey, publicKey } = generateKeyPair();
const { signature } = await sign(data, privateKey);
const isValid = await verify(data, signature, privateKey);
```
