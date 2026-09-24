/**
 * @tauron/market — tauron 插件市场
 *
 * 设计文档引用：
 * - §8 插件市场
 * - §8.1 Ed25519 签名
 * - §8.2 index.json 生成
 * - §8.3 插件注册表
 */

// ---- Types ----
export type {
  PluginPackage,
  IndexEntry,
  IndexFile,
  KeyPair,
  SignatureResult,
} from './types.js';

// ---- Signing ----
export {
  generateKeyPair,
  derivePublicKey,
  getPublicKey,
  sign,
  verify,
  signString,
  verifyString,
} from './sign.js';

// ---- Registry ----
export { createRegistry, type Registry } from './registry.js';
