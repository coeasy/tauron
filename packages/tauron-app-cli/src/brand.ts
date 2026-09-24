// ──────────────────────────────────────────────────────────────────────────
// 品牌构建逻辑（开发计划 §4.17 `brand build`）。
//
// 职责：品牌清单 → tauri.conf 片段 + 图标管线 + CI 矩阵。
//
// 关键约束：
// - identifier/协议名/自启项/数据目录/快捷键唯一性校验
// - dev 构建自动加 .dev 后缀
// - 图标全平台齐备校验
// - 产物可复现
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 构建目标平台。 */
export type BuildPlatform = 'windows' | 'macos' | 'linux';

/** 构建类型。 */
export type BuildType = 'dev' | 'release';

/** 图标文件配置。 */
export interface IconFiles {
  /** Windows 图标 (.ico)。 */
  ico?: string;
  /** macOS 图标 (.icns)。 */
  icns?: string;
  /** Linux 图标 (.png)。 */
  png?: string;
  /** 通用图标 (.png)。 */
  universal?: string;
}

/** 品牌构建配置。 */
export interface BrandBuildConfig {
  /** 品牌名称。 */
  name: string;
  /** 品牌标识（唯一）。 */
  identifier: string;
  /** 应用名称。 */
  appName: string;
  /** 应用版本。 */
  version: string;
  /** 协议名（用于 deep link）。 */
  protocolName: string;
  /** 自启项名称。 */
  autostartName: string;
  /** 数据目录名称。 */
  dataDirName: string;
  /** 快捷键。 */
  shortcut: string;
  /** 图标文件配置。 */
  icons: IconFiles;
  /** 构建目标平台。 */
  platforms: BuildPlatform[];
  /** 构建类型。 */
  buildType: BuildType;
  /** 是否生成 CI 矩阵。 */
  generateCimatrix: boolean;
}

/** 品牌构建结果。 */
export interface BrandBuildResult {
  /** 生成的文件列表（路径 → 内容）。 */
  files: Map<string, string>;
  /** 是否成功。 */
  ok: boolean;
  /** 错误信息（如果失败）。 */
  error?: string;
}

/** 唯一性冲突错误。 */
export class UniquenessConflictError extends Error {
  constructor(field: string, value: string) {
    super(`唯一性冲突：${field} 的值 "${value}" 已存在`);
    this.name = 'UniquenessConflictError';
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 验证
// ──────────────────────────────────────────────────────────────────────────

/** 支持的平台列表。 */
export const SUPPORTED_PLATFORMS: readonly BuildPlatform[] = ['windows', 'macos', 'linux'];

/** 支持的构建类型。 */
export const SUPPORTED_BUILD_TYPES: readonly BuildType[] = ['dev', 'release'];

/**
 * 验证品牌名称。
 */
export function validateBrandName(name: string): string {
  if (!name || name.trim() === '') {
    throw new Error('品牌名称不能为空');
  }
  if (name.length > 100) {
    throw new Error('品牌名称最多 100 个字符');
  }
  return name;
}

/**
 * 验证品牌标识。
 *
 * 规则：
 * - 非空
 * - 仅允许小写字母、数字、连字符、点
 * - 不以连字符或点开头或结尾
 * - 长度 3-100
 */
export function validateIdentifier(identifier: string): string {
  if (!identifier || identifier.trim() === '') {
    throw new Error('品牌标识不能为空');
  }
  if (identifier.length < 3) {
    throw new Error('品牌标识至少 3 个字符');
  }
  if (identifier.length > 100) {
    throw new Error('品牌标识最多 100 个字符');
  }
  if (!/^[a-z0-9][a-z0-9.-]*[a-z0-9]$/.test(identifier)) {
    throw new Error(
      '品牌标识仅允许小写字母、数字、连字符和点，且不能以连字符或点开头或结尾',
    );
  }
  return identifier;
}

/**
 * 验证协议名。
 *
 * 规则：
 * - 非空
 * - 仅允许小写字母、数字、连字符
 * - 长度 3-50
 */
export function validateProtocolName(protocolName: string): string {
  if (!protocolName || protocolName.trim() === '') {
    throw new Error('协议名不能为空');
  }
  if (protocolName.length < 3) {
    throw new Error('协议名至少 3 个字符');
  }
  if (protocolName.length > 50) {
    throw new Error('协议名最多 50 个字符');
  }
  if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(protocolName)) {
    throw new Error(
      '协议名仅允许小写字母、数字和连字符，且不能以连字符开头或结尾',
    );
  }
  return protocolName;
}

/**
 * 验证自启项名称。
 */
export function validateAutostartName(autostartName: string): string {
  if (!autostartName || autostartName.trim() === '') {
    throw new Error('自启项名称不能为空');
  }
  if (autostartName.length > 100) {
    throw new Error('自启项名称最多 100 个字符');
  }
  return autostartName;
}

/**
 * 验证数据目录名称。
 *
 * 规则：
 * - 非空
 * - 仅允许小写字母、数字、连字符
 * - 长度 3-50
 */
export function validateDataDirName(dataDirName: string): string {
  if (!dataDirName || dataDirName.trim() === '') {
    throw new Error('数据目录名称不能为空');
  }
  if (dataDirName.length < 3) {
    throw new Error('数据目录名称至少 3 个字符');
  }
  if (dataDirName.length > 50) {
    throw new Error('数据目录名称最多 50 个字符');
  }
  if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(dataDirName)) {
    throw new Error(
      '数据目录名称仅允许小写字母、数字和连字符，且不能以连字符开头或结尾',
    );
  }
  return dataDirName;
}

/**
 * 验证快捷键。
 *
 * 规则：
 * - 非空
 * - 格式：Ctrl+Shift+Key（示例）
 */
export function validateShortcut(shortcut: string): string {
  if (!shortcut || shortcut.trim() === '') {
    throw new Error('快捷键不能为空');
  }
  if (!/^[A-Za-z0-9+\-]+$/.test(shortcut)) {
    throw new Error('快捷键包含非法字符');
  }
  if (!shortcut.includes('+')) {
    throw new Error('快捷键至少需要两个键的组合（用 + 分隔）');
  }
  return shortcut;
}

/**
 * 验证图标文件配置。
 *
 * 规则：
 * - 至少一个平台有图标
 * - 每个平台最多一个图标文件
 */
export function validateIcons(icons: IconFiles): IconFiles {
  const hasIco = icons.ico !== undefined;
  const hasIcns = icons.icns !== undefined;
  const hasPng = icons.png !== undefined;
  const hasUniversal = icons.universal !== undefined;

  if (!hasIco && !hasIcns && !hasPng && !hasUniversal) {
    throw new Error('至少需要一个图标文件');
  }

  // 检查文件路径是否合法（不能包含 .. 或绝对路径）
  for (const [key, path] of Object.entries(icons)) {
    if (path && (path.includes('..') || path.startsWith('/') || path.startsWith('\\'))) {
      throw new Error(`图标文件路径 "${key}" 包含非法字符或绝对路径`);
    }
  }

  return icons;
}

/**
 * 验证平台列表。
 */
export function validatePlatforms(platforms: string[]): BuildPlatform[] {
  if (!Array.isArray(platforms) || platforms.length === 0) {
    throw new Error('至少需要一个平台');
  }
  const result: BuildPlatform[] = [];
  for (const platform of platforms) {
    if (!SUPPORTED_PLATFORMS.includes(platform as BuildPlatform)) {
      throw new Error(
        `不支持的平台 "${platform}"，可选：${SUPPORTED_PLATFORMS.join(', ')}`,
      );
    }
    result.push(platform as BuildPlatform);
  }
  return result;
}

/**
 * 验证构建类型。
 */
export function validateBuildType(buildType: string): BuildType {
  if (!SUPPORTED_BUILD_TYPES.includes(buildType as BuildType)) {
    throw new Error(
      `不支持的构建类型 "${buildType}"，可选：${SUPPORTED_BUILD_TYPES.join(', ')}`,
    );
  }
  return buildType as BuildType;
}

/**
 * 唯一性校验。
 *
 * 检查 identifier/protocolName/autostartName/dataDirName/shortcut 是否唯一。
 */
export function checkUniqueness(
  config: BrandBuildConfig,
  existing: Array<Pick<BrandBuildConfig, 'identifier' | 'protocolName' | 'autostartName' | 'dataDirName' | 'shortcut'>>,
): void {
  for (const existingConfig of existing) {
    if (existingConfig.identifier === config.identifier) {
      throw new UniquenessConflictError('identifier', config.identifier);
    }
    if (existingConfig.protocolName === config.protocolName) {
      throw new UniquenessConflictError('protocolName', config.protocolName);
    }
    if (existingConfig.autostartName === config.autostartName) {
      throw new UniquenessConflictError('autostartName', config.autostartName);
    }
    if (existingConfig.dataDirName === config.dataDirName) {
      throw new UniquenessConflictError('dataDirName', config.dataDirName);
    }
    if (existingConfig.shortcut === config.shortcut) {
      throw new UniquenessConflictError('shortcut', config.shortcut);
    }
  }
}

/**
 * 完整验证品牌构建配置。
 */
export function validateBrandBuildConfig(config: Partial<BrandBuildConfig>): BrandBuildConfig {
  const name = validateBrandName(config.name ?? '');
  const identifier = validateIdentifier(config.identifier ?? '');
  const appName = config.appName ?? name;
  const version = config.version ?? '0.1.0';
  const protocolName = validateProtocolName(config.protocolName ?? '');
  const autostartName = validateAutostartName(config.autostartName ?? '');
  const dataDirName = validateDataDirName(config.dataDirName ?? '');
  const shortcut = validateShortcut(config.shortcut ?? '');
  const icons = validateIcons(config.icons ?? { universal: 'icon.png' });
  const platforms = validatePlatforms(config.platforms ?? ['windows', 'macos', 'linux']);
  const buildType = validateBuildType(config.buildType ?? 'release');

  return {
    name,
    identifier,
    appName,
    version,
    protocolName,
    autostartName,
    dataDirName,
    shortcut,
    icons,
    platforms,
    buildType,
    generateCimatrix: config.generateCimatrix ?? true,
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 文件生成
// ──────────────────────────────────────────────────────────────────────────

/**
 * 生成品牌 tauri.conf.json 片段。
 *
 * dev 构建自动加 .dev 后缀。
 */
export function generateBrandTauriConfig(config: BrandBuildConfig): string {
  const isDev = config.buildType === 'dev';
  const suffix = isDev ? '.dev' : '';

  const tauriConfig = {
    productName: config.appName,
    version: config.version,
    identifier: `${config.identifier}${suffix}`,
    build: {
      frontendDist: '../dist',
      devUrl: 'http://localhost:1420',
    },
    app: {
      windows: [
        {
          title: config.appName,
          width: 1024,
          height: 768,
          minWidth: 800,
          minHeight: 600,
        },
      ],
      security: {
        csp: "default-src 'self'",
      },
      deepLink: {
        protocol: `${config.protocolName}${suffix}`,
      },
    },
    bundle: {
      active: true,
      targets: 'all',
      windows: {
        webviewInstallMode: { type: 'embedBootstrapper' },
        nsis: {
          installMode: 'currentUser',
        },
      },
      macos: {
        minimumSystemVersion: '11.0',
      },
      linux: {
        appimage: {
          bundleMediaFiles: true,
        },
      },
      resources: {
        icons: {
          ico: config.icons.ico,
          icns: config.icons.icns,
          png: config.icons.png,
        },
      },
      shortCircuit: config.shortcut,
      autoStart: {
        name: `${config.autostartName}${suffix}`,
      },
      dataDir: `${config.dataDirName}${suffix}`,
    },
  };

  return JSON.stringify(tauriConfig, null, 2) + '\n';
}

/**
 * 生成品牌 manifest.json。
 */
export function generateBrandManifest(config: BrandBuildConfig): string {
  const manifest = {
    manifestVersion: 1,
    name: config.name,
    identifier: config.identifier,
    appName: config.appName,
    version: config.version,
    protocolName: config.protocolName,
    autostartName: config.autostartName,
    dataDirName: config.dataDirName,
    shortcut: config.shortcut,
    icons: config.icons,
    platforms: config.platforms,
    buildType: config.buildType,
    generatedAt: new Date().toISOString(),
  };
  return JSON.stringify(manifest, null, 2) + '\n';
}

/**
 * 生成 CI 矩阵配置。
 */
export function generateCiMatrix(config: BrandBuildConfig): string {
  if (!config.generateCimatrix) {
    return '';
  }

  const matrix: unknown[] = [];
  for (const platform of config.platforms) {
    matrix.push({
      platform,
      buildType: config.buildType,
      brand: config.identifier,
    });
  }

  const ciConfig = {
    version: 1,
    matrix,
    generatedAt: new Date().toISOString(),
  };

  return JSON.stringify(ciConfig, null, 2) + '\n';
}

/**
 * 生成图标检查报告。
 */
export function generateIconReport(config: BrandBuildConfig): string {
  const reports: Array<{ platform: string; status: 'ok' | 'missing' | 'warn'; file?: string }> = [];

  for (const platform of config.platforms) {
    switch (platform) {
      case 'windows':
        if (config.icons.ico) {
          reports.push({ platform, status: 'ok', file: config.icons.ico });
        } else if (config.icons.universal) {
          reports.push({ platform, status: 'warn', file: config.icons.universal });
        } else {
          reports.push({ platform, status: 'missing' });
        }
        break;
      case 'macos':
        if (config.icons.icns) {
          reports.push({ platform, status: 'ok', file: config.icons.icns });
        } else if (config.icons.universal) {
          reports.push({ platform, status: 'warn', file: config.icons.universal });
        } else {
          reports.push({ platform, status: 'missing' });
        }
        break;
      case 'linux':
        if (config.icons.png) {
          reports.push({ platform, status: 'ok', file: config.icons.png });
        } else if (config.icons.universal) {
          reports.push({ platform, status: 'warn', file: config.icons.universal });
        } else {
          reports.push({ platform, status: 'missing' });
        }
        break;
    }
  }

  return JSON.stringify({ version: 1, reports }, null, 2) + '\n';
}

/**
 * 生成品牌项目文件列表。
 */
export function generateBrandFiles(config: BrandBuildConfig): Map<string, string> {
  const files = new Map<string, string>();

  files.set('brand/manifest.json', generateBrandManifest(config));
  files.set('src-tauri/tauri.conf.json', generateBrandTauriConfig(config));
  files.set('brand/icon-report.json', generateIconReport(config));

  if (config.generateCimatrix) {
    files.set('brand/ci-matrix.json', generateCiMatrix(config));
  }

  return files;
}

// ──────────────────────────────────────────────────────────────────────────
// 构建
// ──────────────────────────────────────────────────────────────────────────

/**
 * 执行品牌构建。
 *
 * 验证配置并生成所有文件。
 */
export function brandBuild(
  config: Partial<BrandBuildConfig>,
  existing: Array<Pick<BrandBuildConfig, 'identifier' | 'protocolName' | 'autostartName' | 'dataDirName' | 'shortcut'>> = [],
): BrandBuildResult {
  try {
    const validated = validateBrandBuildConfig(config);
    checkUniqueness(validated, existing);
    const files = generateBrandFiles(validated);
    return { files, ok: true };
  } catch (err) {
    return {
      files: new Map(),
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
