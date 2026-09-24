// ──────────────────────────────────────────────────────────────────────────
// 插件打包/签名/发布逻辑（开发计划 §4.17 `plugin pack/sign/publish`）。
//
// 职责：将插件目录打包为 zip 格式、签名、发布到仓库。
//
// 签名**不在本包实现**：`pluginSign` / `verifySignatureCrypto` 委托同生态的
// `@tauron/market`（`sign` / `verify`，见 `packages/tauron-market/src/sign.ts`）——
// 零新依赖、单一实现源（P0-3：CLI 签名路径与市场签名算法不可能再漂移）。
// 本包只负责：载荷定义、密钥格式规范化（PEM/hex → DER hex）、签名信封。
// 编码约定**以 `@tauron/market` 为准**：密钥 = DER hex，签名 = hex（非 base64）。
// 打包/扫描/收集仍用 Node 内置（zlib/crypto 的 SHA-256）。
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as crypto from 'node:crypto';

import { sign as marketSign, verify as marketVerify } from '@tauron/market';

// ── 类型 ──

export interface PluginFileInfo {
  path: string;
  size: number;
  hash: string;
}

export interface PluginConfig {
  id: string;
  name: string;
  description?: string;
  pluginType: 'js' | 'process' | 'wasm';
  version: string;
  permissions: string[];
  enabled: boolean;
}

/**
 * 支持的签名算法（**声明面**）。
 *
 * 注意：声明面 ≠ 实现面。`@tauron/market` 当前只实现 `ed25519`；
 * `rsa-2048` / `rsa-4096` 保留在声明面（配置校验兼容），签名时显式报错。
 */
export type SignAlgorithm = 'ed25519' | 'rsa-2048' | 'rsa-4096';

export interface PluginSignature {
  algorithm: SignAlgorithm;
  kid: string;
  issuedAt: string;
  /** 签名字节串：**hex**（`@tauron/market` 的编码约定；历史版本为 base64）。 */
  signature: string;
  files: PluginFileInfo[];
}

/**
 * 打包配置（调用方输入）。
 *
 * `outputPath` / `includeSource` / `compress` 缺省时由 {@link validatePackConfig}
 * 填充默认值；`pluginConfig` 缺失视为非法配置。
 */
export interface PackConfig {
  dir: string;
  outputPath?: string;
  includeSource?: boolean;
  compress?: boolean;
  files: PluginFileInfo[];
  pluginConfig?: PluginConfig;
}

/** 已应用默认值并通过校验的打包配置。 */
export interface ResolvedPackConfig extends PackConfig {
  outputPath: string;
  includeSource: boolean;
  compress: boolean;
  pluginConfig: PluginConfig;
}

export interface PackResult {
  ok: boolean;
  outputPath?: string;
  files?: PluginFileInfo[];
  totalSize?: number;
  error?: string;
}

/**
 * 签名配置（调用方输入）。
 *
 * `algorithm` 保持为原始字符串，经 {@link validateSignConfig} 校验后收窄为
 * {@link SignAlgorithm}；`includeSource` / `frameworkRange` 缺省时填充默认值。
 */
export interface SignConfig {
  dir: string;
  /** 签名算法（原始输入，如来自 CLI 参数）。 */
  algorithm: string;
  kid: string;
  privateKey: string;
  includeSource?: boolean;
  frameworkRange?: string;
}

/** 已通过校验并应用默认值的签名配置。 */
export interface ResolvedSignConfig extends SignConfig {
  algorithm: SignAlgorithm;
  includeSource: boolean;
  frameworkRange: string;
}

export interface SignResult {
  ok: boolean;
  signature?: PluginSignature;
  error?: string;
}

/**
 * 发布配置（调用方输入）。
 *
 * `overwrite` / `includeSignature` 缺省时由 {@link validatePublishConfig} 填充默认值。
 */
export interface PublishConfig {
  dir: string;
  targetUrl: string;
  token: string;
  overwrite?: boolean;
  includeSignature?: boolean;
}

/** 已应用默认值并通过校验的发布配置。 */
export interface ResolvedPublishConfig extends PublishConfig {
  overwrite: boolean;
  includeSignature: boolean;
}

export interface PublishResult {
  ok: boolean;
  url?: string;
  error?: string;
}

// ── 常量 ──

export const SUPPORTED_SIGN_ALGORITHMS: readonly SignAlgorithm[] = ['ed25519', 'rsa-2048', 'rsa-4096'];
export const MAX_FILE_SIZE = 100 * 1024 * 1024; // 100MB
export const MAX_FILE_COUNT = 2000;

// ── 验证函数 ──

/** 判断字符串是否为受支持的签名算法（类型守卫）。 */
export function isSignAlgorithm(algo: string): algo is SignAlgorithm {
  return SUPPORTED_SIGN_ALGORITHMS.some((supported) => supported === algo);
}

export function validateSignAlgorithm(algo: string): SignAlgorithm {
  if (!isSignAlgorithm(algo)) {
    throw new Error(`不支持的签名算法：${algo}`);
  }
  return algo;
}

export function validateKid(kid: string): string {
  if (!kid || kid.trim() === '') {
    throw new Error('密钥 ID 不能为空');
  }
  if (kid.length > 100) {
    throw new Error(`密钥 ID 长度 ${kid.length} 超过 100`);
  }
  if (!/^[a-zA-Z0-9._-]+$/.test(kid)) {
    throw new Error(`密钥 ID 包含非法字符：${kid}`);
  }
  return kid;
}

export function validateFiles(files: PluginFileInfo[]): PluginFileInfo[] {
  if (files.length === 0) {
    throw new Error('文件列表不能为空');
  }
  if (files.length > MAX_FILE_COUNT) {
    throw new Error(`文件数 ${files.length} 超过上限 ${MAX_FILE_COUNT}`);
  }
  for (const file of files) {
    if (!file.path || file.path.trim() === '') {
      throw new Error('文件路径不能为空');
    }
    if (file.path.includes('..')) {
      throw new Error(`文件路径包含 ..：${file.path}`);
    }
    if (path.isAbsolute(file.path)) {
      throw new Error(`文件路径为绝对路径：${file.path}`);
    }
    if (file.size < 0) {
      throw new Error(`文件大小不能为负数：${file.size}`);
    }
    if (file.size > MAX_FILE_SIZE) {
      throw new Error(`文件大小 ${file.size} 超过上限 ${MAX_FILE_SIZE}`);
    }
    if (!/^[a-f0-9]{64}$/.test(file.hash)) {
      throw new Error(`哈希格式错误：${file.hash}`);
    }
  }
  return files;
}

export function validateFrameworkRange(range: string): string {
  if (!range || range.trim() === '') {
    throw new Error('框架版本范围不能为空');
  }
  const parts = range.split(',').map((p) => p.trim());
  for (const part of parts) {
    if (!/^(<=|>=|<|>|=|\^|~)\s*\d+\.\d+\.\d+$/.test(part)) {
      throw new Error(`框架版本范围格式错误：${part}`);
    }
  }
  return range;
}

export function frameworkMatchesRange(version: string, range: string): boolean {
  const parts = range.split(',').map((p) => p.trim());
  for (const part of parts) {
    const match = part.match(/^(<=|>=|<|>|=|\^|~)\s*(\d+)\.(\d+)\.(\d+)$/);
    if (!match) return false;
    const op = match[1];
    const rMajor = parseInt(match[2] ?? '0', 10);
    const rMinor = parseInt(match[3] ?? '0', 10);
    const rPatch = parseInt(match[4] ?? '0', 10);
    const [vMajor = 0, vMinor = 0, vPatch = 0] = version.split('.').map((n) => parseInt(n, 10));
    const cmp = compareVersions([vMajor, vMinor, vPatch], [rMajor, rMinor, rPatch]);
    switch (op) {
      case '>=': if (cmp < 0) return false; break;
      case '<=': if (cmp > 0) return false; break;
      case '>': if (cmp <= 0) return false; break;
      case '<': if (cmp >= 0) return false; break;
      case '=': if (cmp !== 0) return false; break;
      case '^': // caret: >=version, <next major
        if (cmp < 0) return false;
        if (vMajor > rMajor) return false;
        break;
      case '~': // tilde: >=version, <next minor
        if (cmp < 0) return false;
        if (vMajor > rMajor) return false;
        if (vMajor === rMajor && vMinor > rMinor) return false;
        break;
      default:
        // 正则已保证 op 属于上述运算符，此处仅为穷尽保护。
        return false;
    }
  }
  return true;
}

function compareVersions(a: readonly number[], b: readonly number[]): number {
  for (let i = 0; i < 3; i++) {
    const left = a[i] ?? 0;
    const right = b[i] ?? 0;
    if (left > right) return 1;
    if (left < right) return -1;
  }
  return 0;
}

export function validatePackConfig(config: PackConfig): ResolvedPackConfig {
  if (!config.dir || config.dir.trim() === '') {
    throw new Error('目录不能为空');
  }
  if (!config.files || config.files.length === 0) {
    throw new Error('文件列表不能为空');
  }
  if (!config.pluginConfig) {
    throw new Error('插件配置不能为空');
  }
  return {
    dir: config.dir,
    outputPath: config.outputPath ?? `${config.dir}/dist/${config.pluginConfig.id}.zip`,
    includeSource: config.includeSource ?? false,
    compress: config.compress ?? true,
    files: config.files,
    pluginConfig: config.pluginConfig,
  };
}

export function validateSignConfig(config: SignConfig): ResolvedSignConfig {
  if (!config.dir || config.dir.trim() === '') {
    throw new Error('目录不能为空');
  }
  if (!config.algorithm || config.algorithm.trim() === '') {
    throw new Error('签名算法不能为空');
  }
  const algorithm = validateSignAlgorithm(config.algorithm);
  if (!config.kid || config.kid.trim() === '') {
    throw new Error('密钥 ID 不能为空');
  }
  if (!config.privateKey || config.privateKey.trim() === '') {
    throw new Error('私钥不能为空');
  }
  return {
    dir: config.dir,
    algorithm,
    kid: config.kid,
    privateKey: config.privateKey,
    includeSource: config.includeSource ?? false,
    frameworkRange: config.frameworkRange ?? '>=0.1.0',
  };
}

export function validatePublishConfig(config: PublishConfig): ResolvedPublishConfig {
  if (!config.dir || config.dir.trim() === '') {
    throw new Error('目录不能为空');
  }
  if (!config.targetUrl || config.targetUrl.trim() === '') {
    throw new Error('目标 URL 不能为空');
  }
  if (!config.targetUrl.startsWith('https://')) {
    throw new Error('目标 URL 必须使用 HTTPS');
  }
  if (!config.token || config.token.trim() === '') {
    throw new Error('Token 不能为空');
  }
  return {
    dir: config.dir,
    targetUrl: config.targetUrl,
    token: config.token,
    overwrite: config.overwrite ?? false,
    includeSignature: config.includeSignature ?? true,
  };
}

// ── 打包 ──

/** 插件目录扫描结果。 */
export interface PluginScanResult {
  ok: boolean;
  files: PluginFileInfo[];
  error?: string;
}

/** 扫描插件目录时跳过的目录（依赖与版本控制元数据）。 */
const SKIPPED_DIRS: readonly string[] = ['node_modules', '.git'];

/**
 * 递归扫描插件目录，计算每个文件的相对路径、大小与 SHA-256。
 *
 * 用于 `plugin pack` / `plugin sign` 生成文件清单；扫描结果同样受
 * {@link validateFiles} 的约束（文件数 / 单文件大小 / 路径合法性）。
 */
export function collectPluginFiles(dir: string): PluginScanResult {
  const root = path.resolve(dir);
  if (!fs.existsSync(root)) {
    return { ok: false, files: [], error: `目录不存在：${dir}` };
  }

  const files: PluginFileInfo[] = [];
  const walk = (absDir: string): void => {
    for (const entry of fs.readdirSync(absDir, { withFileTypes: true })) {
      const abs = path.join(absDir, entry.name);
      if (entry.isDirectory()) {
        if (!SKIPPED_DIRS.includes(entry.name)) walk(abs);
        continue;
      }
      if (!entry.isFile()) continue;
      const content = fs.readFileSync(abs);
      files.push({
        path: path.relative(root, abs).split(path.sep).join('/'),
        size: content.byteLength,
        hash: crypto.createHash('sha256').update(content).digest('hex'),
      });
    }
  };

  try {
    walk(root);
    return { ok: true, files: validateFiles(files) };
  } catch (err) {
    return { ok: false, files: [], error: err instanceof Error ? err.message : String(err) };
  }
}

export function pluginPack(config: PackConfig): PackResult {
  try {
    const validated = validatePackConfig(config);
    const totalSize = validated.files.reduce((sum, f) => sum + f.size, 0);
    return {
      ok: true,
      outputPath: validated.outputPath,
      files: validated.files,
      totalSize,
    };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

export function generatePackManifest(config: ResolvedPackConfig): string {
  const totalSize = config.files.reduce((sum, f) => sum + f.size, 0);
  return JSON.stringify({
    version: 1,
    pluginId: config.pluginConfig.id,
    pluginName: config.pluginConfig.name,
    pluginVersion: config.pluginConfig.version,
    pluginType: config.pluginConfig.pluginType,
    files: config.files,
    totalSize,
    includeSource: config.includeSource,
    compress: config.compress,
  });
}

// ── 签名 ──
//
// 真正的签名/验签调用 `@tauron/market`（同生态消费，零新依赖）。
// 本包内**不存在**任何 HMAC / Ed25519 / RSA 的密码学实现——只有：
//   · `signPayload()`  载荷定义（文件哈希按清单顺序拼接的 UTF-8 字节）
//   · `toPkcs8PrivateKeyHex()` / `toSpkiPublicKeyHex()`  密钥格式规范化
// 规范化的原因：`@tauron/market` 的入参是 **DER hex 字符串**，而 CLI 历史上
// 也接受 PEM 文本（`--key` 指向 .pem 文件）。转换仅做编码切换，不参与签名的
// 数学计算——摘要与签名字节完全由 `@tauron/market` 产出。

/**
 * 签名载荷：文件哈希按清单顺序拼接后的 UTF-8 字节。
 *
 * 该定义是本包与市场之间的**唯一**载荷约定（改名/改序即协议破坏）。
 */
export function signPayload(files: PluginFileInfo[]): Uint8Array {
  return new TextEncoder().encode(files.map((f) => f.hash).join(''));
}

/**
 * 把 PEM 文本或 hex 私钥规范化为 PKCS#8 DER hex（`@tauron/market` 的入参形态）。
 *
 * 纯编码转换（PEM→DER / 大小写归一），不做任何密码学运算。
 */
export function toPkcs8PrivateKeyHex(privateKey: string): string {
  const trimmed = privateKey.trim();
  if (trimmed.startsWith('-----BEGIN')) {
    const key = crypto.createPrivateKey({ key: trimmed, format: 'pem' });
    return key.export({ type: 'pkcs8', format: 'der' }).toString('hex');
  }
  if (/^[0-9a-fA-F]+$/.test(trimmed) && trimmed.length % 2 === 0) {
    return trimmed.toLowerCase();
  }
  throw new Error('私钥格式无效：需为 PEM 文本或 hex 编码的 PKCS#8 DER');
}

/** 把 PEM 文本或 hex 公钥规范化为 SPKI DER hex（`@tauron/market` 的入参形态）。 */
export function toSpkiPublicKeyHex(publicKey: string): string {
  const trimmed = publicKey.trim();
  if (trimmed.startsWith('-----BEGIN')) {
    const key = crypto.createPublicKey({ key: trimmed, format: 'pem' });
    return key.export({ type: 'spki', format: 'der' }).toString('hex');
  }
  if (/^[0-9a-fA-F]+$/.test(trimmed) && trimmed.length % 2 === 0) {
    return trimmed.toLowerCase();
  }
  throw new Error('公钥格式无效：需为 PEM 文本或 hex 编码的 SPKI DER');
}

/**
 * 签名插件文件清单（Ed25519，实现来自 `@tauron/market`）。
 *
 * 异步：底层 `@tauron/market` 的 `sign()` 是 `async` 函数（内部用 Node
 * `crypto.sign`），因此本函数也必须是 Promise。签名值为 **hex**。
 *
 * 算法面**诚实收窄**：`@tauron/market` 只实现 Ed25519，`rsa-2048` / `rsa-4096`
 * 仍在 {@link validateSignConfig} 的白名单里（配置校验保持兼容），但签名会显式
 * 失败——不静默降级为摘要、也不伪造 `algorithm` 字段。
 */
export async function pluginSign(config: SignConfig, files: PluginFileInfo[]): Promise<SignResult> {
  try {
    const validated = validateSignConfig(config);
    if (validated.algorithm !== 'ed25519') {
      throw new Error(
        `签名算法 ${validated.algorithm} 未接线：@tauron/market 仅实现 ed25519`,
      );
    }
    const signed = await marketSign(
      signPayload(files),
      toPkcs8PrivateKeyHex(validated.privateKey),
    );
    const signature: PluginSignature = {
      algorithm: validated.algorithm,
      kid: validated.kid,
      issuedAt: signed.signedAt,
      signature: signed.signature,
      files,
    };
    return { ok: true, signature };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

/**
 * 密码学验证签名（公钥；任何第三方可独立验证）。
 *
 * 实现同样来自 `@tauron/market`（`verify`，Ed25519）。密钥/签名格式错误一律
 * 抛异常——调用方决定是否降级为结构验证；算法不受支持（非 ed25519）也抛异常，
 * 因为「无法验证」与「验证失败」必须可区分。
 */
export async function verifySignatureCrypto(
  signature: PluginSignature,
  publicKey: string,
): Promise<boolean> {
  if (signature.algorithm !== 'ed25519') {
    throw new Error(`@tauron/market 仅实现 ed25519，无法验证 ${signature.algorithm} 签名`);
  }
  return marketVerify(
    signPayload(signature.files),
    signature.signature,
    toSpkiPublicKeyHex(publicKey),
  );
}

/**
 * 结构验证签名信封（字段完整性/格式）。
 *
 * 注意：本函数**不做**密码学验证（它没有公钥输入）。
 * 需要验证签名真伪时使用 {@link verifySignatureCrypto}（传入签发方公钥）。
 */
export function verifySignature(signature: PluginSignature): { ok: boolean; error?: string } {
  if (!signature.algorithm || !SUPPORTED_SIGN_ALGORITHMS.includes(signature.algorithm)) {
    return { ok: false, error: `不支持的签名算法：${signature.algorithm}` };
  }
  if (!signature.kid || signature.kid.trim() === '') {
    return { ok: false, error: '密钥 ID 不能为空' };
  }
  if (!signature.issuedAt || signature.issuedAt.trim() === '') {
    return { ok: false, error: '签发时间不能为空' };
  }
  if (!signature.signature || signature.signature.trim() === '') {
    return { ok: false, error: '签名内容不能为空' };
  }
  if (!signature.files || signature.files.length === 0) {
    return { ok: false, error: '文件列表不能为空' };
  }
  for (const file of signature.files) {
    if (!/^[a-f0-9]{64}$/.test(file.hash)) {
      return { ok: false, error: `哈希格式错误：${file.hash}` };
    }
  }
  return { ok: true };
}

// ── 发布 ──

export function pluginPublish(config: PublishConfig): PublishResult {
  try {
    const validated = validatePublishConfig(config);
    const url = `${validated.targetUrl.replace(/\/$/, '')}/plugin`;
    return { ok: true, url };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
