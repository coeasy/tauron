/**
 * Ed25519 签名（设计文档 §8）
 *
 * 使用 Node 22 内置 `node:crypto` 的 Ed25519 实现（真非对称签名）：
 * - 私钥：PKCS#8 DER（hex 编码存储）
 * - 公钥：SPKI DER（hex 编码存储）
 * - 签名：64 字节 Ed25519 签名（hex 编码）
 *
 * 语义要点（收敛审计 P1-0f 修复）：
 * - `verify`/`verifyString` 输入为**公钥**——签名可被任何持有公钥的
 *   第三方公开验证（市场索引分发的正确安全模型），私钥仅签发方持有。
 * - Ed25519 是确定性签名：同一私钥对同一消息的签名恒定。
 */

import {
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  sign as nodeSign,
  verify as nodeVerify,
} from 'crypto';
import type { KeyPair, SignatureResult } from './types.js';

/**
 * 生成 Ed25519 密钥对（DER 编码，hex 字符串）。
 */
export function generateKeyPair(): KeyPair {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519', {
    privateKeyEncoding: { type: 'pkcs8', format: 'der' },
    publicKeyEncoding: { type: 'spki', format: 'der' },
  });
  return {
    privateKey: (privateKey as Buffer).toString('hex'),
    publicKey: (publicKey as Buffer).toString('hex'),
  };
}

/**
 * 从私钥（PKCS#8 DER hex）派生公钥（SPKI DER hex）。
 */
export function derivePublicKey(privateKeyHex: string): string {
  const key = createPrivateKey({
    key: Buffer.from(privateKeyHex, 'hex'),
    format: 'der',
    type: 'pkcs8',
  });
  return createPublicKey(key).export({ type: 'spki', format: 'der' }).toString('hex');
}

/**
 * 签名消息（Ed25519）。
 *
 * @param message 待签名字节
 * @param privateKeyHex 私钥（PKCS#8 DER hex）
 */
export async function sign(message: Uint8Array, privateKeyHex: string): Promise<SignatureResult> {
  const key = createPrivateKey({
    key: Buffer.from(privateKeyHex, 'hex'),
    format: 'der',
    type: 'pkcs8',
  });
  const signature = nodeSign(null, Buffer.from(message), key);
  return {
    signature: signature.toString('hex'),
    signedAt: new Date().toISOString(),
  };
}

/**
 * 验证签名（Ed25519，公钥验证——任何第三方可独立验证）。
 *
 * @param message 原始消息字节
 * @param signatureHex 签名（hex）
 * @param publicKeyHex 公钥（SPKI DER hex）
 */
export async function verify(
  message: Uint8Array,
  signatureHex: string,
  publicKeyHex: string,
): Promise<boolean> {
  try {
    const key = createPublicKey({
      key: Buffer.from(publicKeyHex, 'hex'),
      format: 'der',
      type: 'spki',
    });
    return nodeVerify(null, Buffer.from(message), key, Buffer.from(signatureHex, 'hex'));
  } catch {
    // 密钥/签名格式非法 → 视为验证失败（不抛出，保持 API 契约布尔语义）
    return false;
  }
}

/**
 * 签名字符串
 */
export async function signString(message: string, privateKeyHex: string): Promise<SignatureResult> {
  const messageBytes = new TextEncoder().encode(message);
  return sign(messageBytes, privateKeyHex);
}

/**
 * 验证字符串签名（公钥）
 */
export async function verifyString(
  message: string,
  signatureHex: string,
  publicKeyHex: string,
): Promise<boolean> {
  const messageBytes = new TextEncoder().encode(message);
  return verify(messageBytes, signatureHex, publicKeyHex);
}

/**
 * 获取公钥（从私钥派生）
 */
export function getPublicKey(privateKeyHex: string): string {
  return derivePublicKey(privateKeyHex);
}