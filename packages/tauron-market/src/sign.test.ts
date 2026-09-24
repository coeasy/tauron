import { describe, it, expect } from 'vitest';
import {
  generateKeyPair,
  derivePublicKey,
  signString,
  verifyString,
  getPublicKey,
} from './sign.js';

describe('Ed25519 signing（真非对称：私钥签发 / 公钥验证）', () => {
  it('generates a key pair (DER-encoded)', () => {
    const keyPair = generateKeyPair();
    expect(keyPair.privateKey).toBeDefined();
    expect(keyPair.publicKey).toBeDefined();
    // PKCS#8/SPKI DER 编码后为 hex 字符串（Ed25519 私钥 48B→96 hex，公钥 44B→88 hex）
    expect(keyPair.privateKey).toMatch(/^[0-9a-f]+$/);
    expect(keyPair.publicKey).toMatch(/^[0-9a-f]+$/);
  });

  it('derives public key from private key', () => {
    const keyPair = generateKeyPair();
    const derived = getPublicKey(keyPair.privateKey);
    expect(derived).toBe(keyPair.publicKey);
    expect(derivePublicKey(keyPair.privateKey)).toBe(keyPair.publicKey);
  });

  it('signs with private key and verifies with public key', async () => {
    const keyPair = generateKeyPair();
    const message = 'Hello, tauron market!';
    const signature = await signString(message, keyPair.privateKey);
    expect(signature.signature).toMatch(/^[0-9a-f]{128}$/); // Ed25519 = 64 字节
    expect(signature.signedAt).toBeDefined();

    const valid = await verifyString(message, signature.signature, keyPair.publicKey);
    expect(valid).toBe(true);
  });

  it('private key is NOT a verification key (asymmetric model)', async () => {
    const keyPair = generateKeyPair();
    const message = 'index.json payload';
    const signature = await signString(message, keyPair.privateKey);
    // 旧实现（HMAC）会「验证通过」——真 Ed25519 下拿私钥当公钥必须失败
    const bogus = await verifyString(message, signature.signature, keyPair.privateKey);
    expect(bogus).toBe(false);
  });

  it('fails verification with wrong public key', async () => {
    const keyPair1 = generateKeyPair();
    const keyPair2 = generateKeyPair();
    const message = 'Hello, tauron market!';
    const signature = await signString(message, keyPair1.privateKey);

    const valid = await verifyString(message, signature.signature, keyPair2.publicKey);
    expect(valid).toBe(false);
  });

  it('fails verification with tampered message', async () => {
    const keyPair = generateKeyPair();
    const message = 'Hello, tauron market!';
    const signature = await signString(message, keyPair.privateKey);

    const tampered = 'Hello, tampered!';
    const valid = await verifyString(tampered, signature.signature, keyPair.publicKey);
    expect(valid).toBe(false);
  });

  it('rejects malformed key/signature input without throwing', async () => {
    expect(await verifyString('m', 'zz-not-hex', 'zz-not-a-key')).toBe(false);
    const keyPair = generateKeyPair();
    expect(await verifyString('m', 'aa'.repeat(64), keyPair.publicKey)).toBe(false);
  });

  it('generates deterministic signatures', async () => {
    const keyPair = generateKeyPair();
    const message = 'Hello, tauron market!';
    const sig1 = await signString(message, keyPair.privateKey);
    const sig2 = await signString(message, keyPair.privateKey);
    // Ed25519 是确定性签名
    expect(sig1.signature).toBe(sig2.signature);
  });
});