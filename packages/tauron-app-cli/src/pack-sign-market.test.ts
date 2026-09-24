// ──────────────────────────────────────────────────────────────────────────
// CLI 签名路径 ↔ `@tauron/market` 一致性测试（P0-3）。
//
// 目标：CLI 的 `pluginSign` **不是**自带实现，而是逐字节委托 `@tauron/market`；
// 且被委托的实现是**标准 Ed25519**（可用 RFC 8032 向量人工复核）。
//
// 三道断言：
//   ① 已知向量（RFC 8032 §7.1 TEST 1）——市场实现 == 标准向量；
//   ② 已知向量（固定文件清单）—— CLI 输出 == 市场直接调用，逐字节相同；
//   ③ 辨别力——改一位输入（一个 hex 字符），签名必须变。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it } from 'vitest';
import { sign as marketSign, verify as marketVerify } from '@tauron/market';
import {
  pluginSign,
  signPayload,
  toPkcs8PrivateKeyHex,
  toSpkiPublicKeyHex,
  verifySignatureCrypto,
  type PluginFileInfo,
} from './pack.js';

// ── 向量 1：RFC 8032 §7.1 TEST 1（Ed25519 标准测试向量）────────────────────
//
//   secret seed : 9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60
//   public key  : d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a
//   message     : ""（空）
//   signature   : e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155
//                 5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b
//
// DER 封装（Node/WebCrypto 的入参形态）：
//   PKCS#8 = 302e020100300506032b657004220420 || seed       （OID 1.3.101.112）
//   SPKI   = 302a300506032b6570032100 || public key
const RFC8032_SEED = '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60';
const RFC8032_PKCS8_HEX = `302e020100300506032b657004220420${RFC8032_SEED}`;
const RFC8032_SPKI_HEX = `302a300506032b6570032100${'d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'}`;
const RFC8032_EMPTY_SIGNATURE =
  'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155' +
  '5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b';

// ── 向量 2：固定文件清单（payload = 哈希按序拼接）─────────────────────────
//
//   payload    = 'aa'×32 + 'bb'×32 （128 个 ASCII 字符）
//   私钥        = 同 RFC 8032 向量 1（seed 9d61b1…7f60）
//   期望签名    = 3d74e24e341612ca59d1190f2caefed1471384e542ef0e843cbe537102e6e865
//                 b295bdb98f3ff21d8ae8aff12bcc730f49c0920f4fc8979e64868545a01aff0b
//
// 复核方式（与实现无关的第二条路径，Node 内置 OpenSSL）：
//   node -e "const c=require('crypto');
//     const k=c.createPrivateKey({key:Buffer.from('302e...7f60','hex'),format:'der',type:'pkcs8'});
//     console.log(c.sign(null,Buffer.from('aa'.repeat(32)+'bb'.repeat(32)),k).toString('hex'))"
const FIXED_FILES: PluginFileInfo[] = [
  { path: 'manifest.json', size: 128, hash: 'aa'.repeat(32) },
  { path: 'dist/index.js', size: 2048, hash: 'bb'.repeat(32) },
];
const FIXED_PAYLOAD = 'aa'.repeat(32) + 'bb'.repeat(32);
const FIXED_SIGNATURE =
  '3d74e24e341612ca59d1190f2caefed1471384e542ef0e843cbe537102e6e865' +
  'b295bdb98f3ff21d8ae8aff12bcc730f49c0920f4fc8979e64868545a01aff0b';

/** 改一位输入：第一个文件哈希的末位 'a' → 'b'。 */
const TAMPERED_FILES: PluginFileInfo[] = [
  { ...FIXED_FILES[0]!, hash: `${'aa'.repeat(31)}ab` },
  FIXED_FILES[1]!,
];

describe('已知向量：@tauron/market 是标准 Ed25519（RFC 8032 §7.1 TEST 1）', () => {
  it('空消息签名 == 标准向量', async () => {
    const signed = await marketSign(new Uint8Array(0), RFC8032_PKCS8_HEX);
    expect(signed.signature).toBe(RFC8032_EMPTY_SIGNATURE);
  });
});

describe('已知向量：CLI 签名 == @tauron/market 直接调用（逐字节）', () => {
  it('固定清单 + 固定密钥 → 期望签名', async () => {
    const result = await pluginSign(
      {
        dir: '/project/plugin',
        algorithm: 'ed25519',
        kid: 'kid-rfc8032',
        privateKey: RFC8032_PKCS8_HEX,
      },
      FIXED_FILES,
    );

    expect(result.ok).toBe(true);
    const signature = result.signature;
    if (signature === undefined) throw new Error('签名失败：结果缺少 signature');

    // ① 期望值（人工可复核的已知向量）
    expect(signature.signature).toBe(FIXED_SIGNATURE);
    expect(signPayload(FIXED_FILES)).toEqual(new TextEncoder().encode(FIXED_PAYLOAD));

    // ② 与 @tauron/market 直接调用逐字节相同
    const direct = await marketSign(signPayload(FIXED_FILES), RFC8032_PKCS8_HEX);
    expect(signature.signature).toBe(direct.signature);
    expect(signature.signature.length).toBe(128); // 64 字节 hex

    // ③ 市场侧可独立验证（公钥验证）
    expect(
      await marketVerify(
        signPayload(FIXED_FILES),
        signature.signature,
        RFC8032_SPKI_HEX,
      ),
    ).toBe(true);

    // ④ CLI 自己的验签回路（同样委托市场实现）
    expect(await verifySignatureCrypto(signature, RFC8032_SPKI_HEX)).toBe(true);
  });

  it('PEM 私钥入参（格式规范化）与 hex 入参签名字节完全相同', async () => {
    const { createPrivateKey } = await import('node:crypto');
    const pem = createPrivateKey({
      key: Buffer.from(RFC8032_PKCS8_HEX, 'hex'),
      format: 'der',
      type: 'pkcs8',
    })
      .export({ type: 'pkcs8', format: 'pem' })
      .toString();

    expect(toPkcs8PrivateKeyHex(pem)).toBe(RFC8032_PKCS8_HEX);

    const viaPem = await pluginSign(
      { dir: '/project/plugin', algorithm: 'ed25519', kid: 'kid-rfc8032', privateKey: pem },
      FIXED_FILES,
    );
    expect(viaPem.signature?.signature).toBe(FIXED_SIGNATURE);
  });

  it('公钥格式规范化：PEM ↔ hex DER 等价', async () => {
    const { createPublicKey } = await import('node:crypto');
    const pem = createPublicKey({
      key: Buffer.from(RFC8032_SPKI_HEX, 'hex'),
      format: 'der',
      type: 'spki',
    })
      .export({ type: 'spki', format: 'pem' })
      .toString();

    expect(toSpkiPublicKeyHex(pem)).toBe(RFC8032_SPKI_HEX);
  });

  it('rsa-* 不被静默降级：显式报错（@tauron/market 只实现 ed25519）', async () => {
    const result = await pluginSign(
      {
        dir: '/project/plugin',
        algorithm: 'rsa-4096',
        kid: 'kid-rfc8032',
        privateKey: RFC8032_PKCS8_HEX,
      },
      FIXED_FILES,
    );
    expect(result.ok).toBe(false);
    expect(result.error).toContain('@tauron/market 仅实现 ed25519');
  });
});

describe('辨别力：改一位输入 → 签名必须变', () => {
  it('仅翻转第一个文件哈希的末位，签名不同且验签失败', async () => {
    const base = await pluginSign(
      { dir: '/project/plugin', algorithm: 'ed25519', kid: 'kid-rfc8032', privateKey: RFC8032_PKCS8_HEX },
      FIXED_FILES,
    );
    const tampered = await pluginSign(
      { dir: '/project/plugin', algorithm: 'ed25519', kid: 'kid-rfc8032', privateKey: RFC8032_PKCS8_HEX },
      TAMPERED_FILES,
    );

    const baseSig = base.signature?.signature;
    const tamperedSig = tampered.signature?.signature;
    expect(baseSig).toBe(FIXED_SIGNATURE);
    expect(tamperedSig).toBeDefined();
    expect(tamperedSig).not.toBe(baseSig);

    // 用期望签名去验证被篡改的载荷 → 必须失败（不是「都变了」就够）
    expect(
      await marketVerify(
        signPayload(TAMPERED_FILES),
        FIXED_SIGNATURE,
        RFC8032_SPKI_HEX,
      ),
    ).toBe(false);

    // 篡改后的载荷用原签名走 CLI 验签同样失败
    await expect(
      verifySignatureCrypto({ ...base.signature!, files: TAMPERED_FILES }, RFC8032_SPKI_HEX),
    ).resolves.toBe(false);
  });
});
