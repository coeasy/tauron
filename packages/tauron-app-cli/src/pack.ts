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
import { deflateRawSync, inflateRawSync } from 'node:zlib';

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
  pluginManifest?: Record<string, unknown>;
}

/** 已应用默认值并通过校验的打包配置。 */
export interface ResolvedPackConfig extends PackConfig {
  outputPath: string;
  includeSource: boolean;
  compress: boolean;
  pluginConfig: PluginConfig;
}

export interface NativePluginManifest {
  id: string;
  name: string;
  version: string;
  type: PluginConfig['pluginType'];
  framework: string;
  entry: { js?: string; sidecar?: string; wasm?: string };
  permissions: string[];
  platforms: string[];
}

/** Emit the strict manifest consumed by tauron-host and include it in the archive. */
export function createNativePluginManifest(config: PluginConfig): NativePluginManifest {
  const entry: NativePluginManifest['entry'] = {};
  switch (config.pluginType) {
    case 'js': entry.js = 'src/index.js'; break;
    case 'process': entry.sidecar = `bin/${config.id.replace(/[^a-zA-Z0-9]/g, '_')}.exe`; break;
    case 'wasm': entry.wasm = 'dist/plugin.wasm'; break;
  }
  return {
    id: config.id,
    name: config.name,
    version: config.version,
    type: config.pluginType,
    framework: '^0.1.0',
    entry,
    permissions: [...config.permissions],
    platforms: ['win', 'mac', 'linux'],
  };
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
export const MAX_TOTAL_SIZE = 512 * 1024 * 1024; // 512MB unpacked per package

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
  const totalSize = files.reduce((sum, file) => sum + file.size, 0);
  if (totalSize > MAX_TOTAL_SIZE) throw new Error(`解包总大小超过上限 ${MAX_TOTAL_SIZE}`);
  for (const file of files) {
    if (!file.path || file.path.trim() === '') {
      throw new Error('文件路径不能为空');
    }
    if (
      file.path.includes('\\') || file.path.includes('\0') || file.path.includes(':') ||
      file.path.startsWith('/') || /^[a-zA-Z]:/.test(file.path) ||
      path.posix.normalize(file.path) !== file.path ||
      file.path.split('/').some((part) =>
        part === '..' || part === '.' || part === '' || /[<>"|?*\x00-\x1f]/.test(part) ||
        /[ .]$/.test(part) || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(part)
      )
    ) {
      throw new Error(`文件路径不是安全的相对路径：${file.path}`);
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
    outputPath: config.outputPath ?? `${config.dir}/dist/${config.pluginConfig.id}.tpkg`,
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
export function collectPluginFiles(dir: string, excludePaths: string[] = []): PluginScanResult {
  const root = path.resolve(dir);
  const excluded = new Set(excludePaths.map((item) => path.resolve(item)));
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
      if (!entry.isFile() || excluded.has(path.resolve(abs)) || /\.tpkg(?:\.sig)?$/i.test(entry.name)) continue;
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

const crcTable = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = (c & 1) ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(data: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of data) crc = (crcTable[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

/** 生成标准 ZIP 包；写入前重新核对每个文件的大小与 SHA-256，避免打包期间内容漂移。 */
export function createPluginArchive(config: ResolvedPackConfig): Buffer {
  const sourceFiles = config.files.filter((file) => file.path !== 'manifest.json');
  const files = sourceFiles.length === 0 ? [] : validateFiles(sourceFiles);
  const sourceManifest = config.pluginManifest ?? createNativePluginManifest(config.pluginConfig);
  const manifestBytes = Buffer.from(`${JSON.stringify({
    ...sourceManifest,
    id: config.pluginConfig.id,
    name: config.pluginConfig.name,
    version: config.pluginConfig.version,
    type: config.pluginConfig.pluginType,
    permissions: [...config.pluginConfig.permissions],
  }, null, 2)}\n`, 'utf8');
  files.unshift({
    path: 'manifest.json',
    size: manifestBytes.byteLength,
    hash: crypto.createHash('sha256').update(manifestBytes).digest('hex'),
  });
  const entries = files.map((file) => {
    if (file.path === 'manifest.json') {
      const data = manifestBytes;
      const name = Buffer.from(file.path, 'utf8');
      const compressed = config.compress ? deflateRawSync(data) : data;
      return { name, data, compressed, crc: crc32(data), method: config.compress ? 8 : 0 };
    }
    const abs = path.resolve(config.dir, ...file.path.split('/'));
    const root = path.resolve(config.dir) + path.sep;
    if (!abs.startsWith(root)) throw new Error(`文件路径越出插件目录：${file.path}`);
    const data = fs.readFileSync(abs);
    if (data.byteLength !== file.size || crypto.createHash('sha256').update(data).digest('hex') !== file.hash) {
      throw new Error(`文件在扫描后发生变化：${file.path}`);
    }
    const name = Buffer.from(file.path, 'utf8');
    const compressed = config.compress ? deflateRawSync(data) : data;
    return { name, data, compressed, crc: crc32(data), method: config.compress ? 8 : 0 };
  });

  const local: Buffer[] = [];
  const central: Buffer[] = [];
  let offset = 0;
  for (const entry of entries) {
    const header = Buffer.alloc(30 + entry.name.length);
    header.writeUInt32LE(0x04034b50, 0);
    header.writeUInt16LE(20, 4);
    header.writeUInt16LE(0x0800, 6);
    header.writeUInt16LE(entry.method, 8);
    header.writeUInt32LE(entry.crc, 14);
    header.writeUInt32LE(entry.compressed.length, 18);
    header.writeUInt32LE(entry.data.length, 22);
    header.writeUInt16LE(entry.name.length, 26);
    entry.name.copy(header, 30);
    local.push(header, entry.compressed);

    const dir = Buffer.alloc(46 + entry.name.length);
    dir.writeUInt32LE(0x02014b50, 0);
    dir.writeUInt16LE(20, 4);
    dir.writeUInt16LE(20, 6);
    dir.writeUInt16LE(0x0800, 8);
    dir.writeUInt16LE(entry.method, 10);
    dir.writeUInt32LE(entry.crc, 16);
    dir.writeUInt32LE(entry.compressed.length, 20);
    dir.writeUInt32LE(entry.data.length, 24);
    dir.writeUInt16LE(entry.name.length, 28);
    dir.writeUInt32LE(offset, 42);
    entry.name.copy(dir, 46);
    central.push(dir);
    offset += header.length + entry.compressed.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, directory, end]);
}

/** 从 `.tpkg` 读取并校验文件清单；只接受无加密、无 ZIP64 的受限 ZIP 子集。 */
export function readPluginArchive(archive: Buffer): PluginFileInfo[] {
  if (archive.length < 22) throw new Error('安装包 ZIP 目录损坏');
  const minEnd = Math.max(0, archive.length - 22 - 0xffff);
  let endOffset = -1;
  for (let i = archive.length - 22; i >= minEnd; i--) {
    if (archive.readUInt32LE(i) === 0x06054b50) { endOffset = i; break; }
  }
  if (endOffset < 0) throw new Error('安装包 ZIP 目录损坏');
  if (archive.readUInt16LE(endOffset + 4) !== 0 || archive.readUInt16LE(endOffset + 6) !== 0) {
    throw new Error('不支持多卷 ZIP 安装包');
  }
  const count = archive.readUInt16LE(endOffset + 10);
  if (archive.readUInt16LE(endOffset + 8) !== count) throw new Error('安装包 ZIP 条目计数不匹配');
  if (count === 0 || count > MAX_FILE_COUNT) throw new Error(`安装包文件数无效：${count}`);
  const centralSize = archive.readUInt32LE(endOffset + 12);
  const centralOffset = archive.readUInt32LE(endOffset + 16);
  let cursor = centralOffset;
  const centralEnd = cursor + centralSize;
  const commentLength = archive.readUInt16LE(endOffset + 20);
  if (endOffset + 22 + commentLength !== archive.length || centralEnd !== endOffset) throw new Error('安装包 ZIP 目录边界错误');
  const files: PluginFileInfo[] = [];
  const names = new Set<string>();
  const ranges: Array<[number, number]> = [];
  let totalSize = 0;
  for (let i = 0; i < count; i++) {
    if (cursor + 46 > centralEnd || archive.readUInt32LE(cursor) !== 0x02014b50) throw new Error('安装包中央目录损坏');
    const flags = archive.readUInt16LE(cursor + 8);
    const method = archive.readUInt16LE(cursor + 10);
    const expectedCrc = archive.readUInt32LE(cursor + 16);
    const compressedSize = archive.readUInt32LE(cursor + 20);
    const size = archive.readUInt32LE(cursor + 24);
    const nameLength = archive.readUInt16LE(cursor + 28);
    const extraLength = archive.readUInt16LE(cursor + 30);
    const commentLength = archive.readUInt16LE(cursor + 32);
    const localOffset = archive.readUInt32LE(cursor + 42);
    if (archive.readUInt16LE(cursor + 34) !== 0) throw new Error('不支持多卷 ZIP 安装包');
    const rawName = archive.subarray(cursor + 46, cursor + 46 + nameLength);
    let name: string;
    try { name = new TextDecoder('utf-8', { fatal: true }).decode(rawName); }
    catch { throw new Error('安装包包含无效 UTF-8 路径'); }
    validateFiles([{ path: name, size, hash: '0'.repeat(64) }]);
    const collisionKey = name.toLowerCase();
    if (names.has(collisionKey)) throw new Error(`安装包包含重复或大小写冲突路径：${name}`);
    names.add(collisionKey);
    if ((flags & ~0x0800) !== 0 || (method !== 0 && method !== 8)) throw new Error(`安装包使用不支持的 ZIP 标志或压缩方式：${name}`);
    const madeBy = archive.readUInt16LE(cursor + 4);
    const externalAttributes = archive.readUInt32LE(cursor + 38);
    if ((madeBy >>> 8) === 3) {
      const fileType = (externalAttributes >>> 16) & 0xf000;
      if (fileType !== 0 && fileType !== 0x8000) throw new Error(`安装包包含非普通文件：${name}`);
    }
    totalSize += size;
    if (totalSize > MAX_TOTAL_SIZE) throw new Error(`解包总大小超过上限 ${MAX_TOTAL_SIZE}`);
    if (localOffset + 30 > centralOffset || archive.readUInt32LE(localOffset) !== 0x04034b50) throw new Error(`安装包本地条目损坏：${name}`);
    const localFlags = archive.readUInt16LE(localOffset + 6);
    const localMethod = archive.readUInt16LE(localOffset + 8);
    const localCrc = archive.readUInt32LE(localOffset + 14);
    const localCompressed = archive.readUInt32LE(localOffset + 18);
    const localSize = archive.readUInt32LE(localOffset + 22);
    const localNameLength = archive.readUInt16LE(localOffset + 26);
    const localExtraLength = archive.readUInt16LE(localOffset + 28);
    if (
      localFlags !== flags || localMethod !== method || localCrc !== expectedCrc ||
      localCompressed !== compressedSize || localSize !== size ||
      !archive.subarray(localOffset + 30, localOffset + 30 + localNameLength).equals(rawName)
    ) throw new Error(`安装包条目头不匹配：${name}`);
    const dataOffset = localOffset + 30 + localNameLength + localExtraLength;
    const dataEnd = dataOffset + compressedSize;
    if (dataEnd > centralOffset) throw new Error(`安装包条目越界：${name}`);
    ranges.push([localOffset, dataEnd]);
    const compressed = archive.subarray(dataOffset, dataEnd);
    const data = method === 8 ? inflateRawSync(compressed, { maxOutputLength: MAX_TOTAL_SIZE }) : Buffer.from(compressed);
    if (data.length !== size || crc32(data) !== expectedCrc) throw new Error(`安装包条目校验失败：${name}`);
    files.push({ path: name, size, hash: crypto.createHash('sha256').update(data).digest('hex') });
    cursor += 46 + nameLength + extraLength + commentLength;
  }
  if (cursor !== centralEnd) throw new Error('安装包中央目录长度不匹配');
  ranges.sort((left, right) => left[0] - right[0]);
  for (let i = 1; i < ranges.length; i++) {
    if ((ranges[i]?.[0] ?? 0) < (ranges[i - 1]?.[1] ?? 0)) throw new Error('安装包 ZIP 条目范围重叠');
  }
  return validateFiles(files);
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
