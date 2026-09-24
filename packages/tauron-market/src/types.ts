/**
 * @tauron/market 共享类型
 */

import type { PluginManifest } from '@tauron/types';

/** 插件包信息 */
export interface PluginPackage {
  /** 插件 ID */
  id: string;
  /** 版本 */
  version: string;
  /** 包文件名 */
  filename: string;
  /** 包大小（字节） */
  size: number;
  /** SHA-256 哈希 */
  sha256: string;
  /** 签名 */
  signature: string;
  /** 上传者 */
  uploadedBy: string;
  /** 上传时间 */
  uploadedAt: string;
  /** 是否已验证 */
  verified: boolean;
}

/** 索引条目 */
export interface IndexEntry {
  /** 插件 ID */
  id: string;
  /** 最新版本 */
  version: string;
  /** 包信息 */
  package: PluginPackage;
  /** manifest */
  manifest: PluginManifest;
  /** 更新时间 */
  updatedAt: string;
}

/** 索引文件 */
export interface IndexFile {
  /** 索引版本 */
  version: '1.0';
  /** 生成时间 */
  generatedAt: string;
  /** 插件列表 */
  plugins: IndexEntry[];
}

/** 签名密钥对 */
export interface KeyPair {
  /** 私钥（hex） */
  privateKey: string;
  /** 公钥（hex） */
  publicKey: string;
}

/** 签名结果 */
export interface SignatureResult {
  /** 签名（hex） */
  signature: string;
  /** 签名时间 */
  signedAt: string;
}
