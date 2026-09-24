// ──────────────────────────────────────────────────────────────────────────
// 品牌构建单元测试。
//
// 测试 brand build 的核心逻辑。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it } from 'vitest';
import {
  brandBuild,
  validateBrandBuildConfig,
  validateBrandName,
  validateIdentifier,
  validateProtocolName,
  validateAutostartName,
  validateDataDirName,
  validateShortcut,
  validateIcons,
  validatePlatforms,
  validateBuildType,
  checkUniqueness,
  generateBrandTauriConfig,
  generateBrandManifest,
  generateCiMatrix,
  generateIconReport,
  generateBrandFiles,
  UniquenessConflictError,
  SUPPORTED_PLATFORMS,
  SUPPORTED_BUILD_TYPES,
  type BrandBuildConfig,
} from '../src/brand.js';

// ──────────────────────────────────────────────────────────────────────────
// 验证函数
// ──────────────────────────────────────────────────────────────────────────

describe('validateBrandName', () => {
  it('接受合法名称', () => {
    expect(validateBrandName('My Brand')).toBe('My Brand');
    expect(validateBrandName('品牌')).toBe('品牌');
  });

  it('拒绝空名称', () => {
    expect(() => validateBrandName('')).toThrow();
    expect(() => validateBrandName('   ')).toThrow();
  });

  it('拒绝过长名称', () => {
    expect(() => validateBrandName('a'.repeat(101))).toThrow();
  });
});

describe('validateIdentifier', () => {
  it('接受合法标识', () => {
    expect(validateIdentifier('my-brand')).toBe('my-brand');
    expect(validateIdentifier('com.example.brand')).toBe('com.example.brand');
    expect(validateIdentifier('brand123')).toBe('brand123');
  });

  it('拒绝空标识', () => {
    expect(() => validateIdentifier('')).toThrow();
    expect(() => validateIdentifier('   ')).toThrow();
  });

  it('拒绝过短标识', () => {
    expect(() => validateIdentifier('ab')).toThrow();
  });

  it('拒绝过长标识', () => {
    expect(() => validateIdentifier('a'.repeat(101))).toThrow();
  });

  it('拒绝包含大写字母', () => {
    expect(() => validateIdentifier('MyBrand')).toThrow();
  });

  it('拒绝包含下划线', () => {
    expect(() => validateIdentifier('my_brand')).toThrow();
  });

  it('拒绝以连字符开头或结尾', () => {
    expect(() => validateIdentifier('-my-brand')).toThrow();
    expect(() => validateIdentifier('my-brand-')).toThrow();
  });

  it('拒绝以点开头或结尾', () => {
    expect(() => validateIdentifier('.my-brand')).toThrow();
    expect(() => validateIdentifier('my-brand.')).toThrow();
  });
});

describe('validateProtocolName', () => {
  it('接受合法协议名', () => {
    expect(validateProtocolName('myapp')).toBe('myapp');
    expect(validateProtocolName('my-app')).toBe('my-app');
    expect(validateProtocolName('app123')).toBe('app123');
  });

  it('拒绝空协议名', () => {
    expect(() => validateProtocolName('')).toThrow();
  });

  it('拒绝过短协议名', () => {
    expect(() => validateProtocolName('ab')).toThrow();
  });

  it('拒绝过长协议名', () => {
    expect(() => validateProtocolName('a'.repeat(51))).toThrow();
  });

  it('拒绝包含大写字母', () => {
    expect(() => validateProtocolName('MyApp')).toThrow();
  });

  it('拒绝以连字符开头或结尾', () => {
    expect(() => validateProtocolName('-myapp')).toThrow();
    expect(() => validateProtocolName('myapp-')).toThrow();
  });
});

describe('validateAutostartName', () => {
  it('接受合法名称', () => {
    expect(validateAutostartName('My App')).toBe('My App');
    expect(validateAutostartName('应用')).toBe('应用');
  });

  it('拒绝空名称', () => {
    expect(() => validateAutostartName('')).toThrow();
  });

  it('拒绝过长名称', () => {
    expect(() => validateAutostartName('a'.repeat(101))).toThrow();
  });
});

describe('validateDataDirName', () => {
  it('接受合法名称', () => {
    expect(validateDataDirName('my-data')).toBe('my-data');
    expect(validateDataDirName('data123')).toBe('data123');
  });

  it('拒绝空名称', () => {
    expect(() => validateDataDirName('')).toThrow();
  });

  it('拒绝过短名称', () => {
    expect(() => validateDataDirName('ab')).toThrow();
  });

  it('拒绝包含大写字母', () => {
    expect(() => validateDataDirName('MyData')).toThrow();
  });

  it('拒绝以连字符开头或结尾', () => {
    expect(() => validateDataDirName('-mydata')).toThrow();
    expect(() => validateDataDirName('mydata-')).toThrow();
  });
});

describe('validateShortcut', () => {
  it('接受合法快捷键', () => {
    expect(validateShortcut('Ctrl+Shift+K')).toBe('Ctrl+Shift+K');
    expect(validateShortcut('Ctrl+A')).toBe('Ctrl+A');
  });

  it('拒绝空快捷键', () => {
    expect(() => validateShortcut('')).toThrow();
  });

  it('拒绝单键（无组合）', () => {
    expect(() => validateShortcut('K')).toThrow();
  });

  it('拒绝包含非法字符', () => {
    expect(() => validateShortcut('Ctrl+K!')).toThrow();
  });
});

describe('validateIcons', () => {
  it('接受完整图标', () => {
    expect(validateIcons({ ico: 'icon.ico', icns: 'icon.icns', png: 'icon.png' })).toEqual({
      ico: 'icon.ico',
      icns: 'icon.icns',
      png: 'icon.png',
    });
  });

  it('接受 universal 图标', () => {
    expect(validateIcons({ universal: 'icon.png' })).toEqual({ universal: 'icon.png' });
  });

  it('拒绝空图标', () => {
    expect(() => validateIcons({})).toThrow();
  });

  it('拒绝包含 .. 的路径', () => {
    expect(() => validateIcons({ ico: '../icon.ico' })).toThrow();
  });

  it('拒绝绝对路径', () => {
    expect(() => validateIcons({ ico: '/usr/local/icon.ico' })).toThrow();
  });
});

describe('validatePlatforms', () => {
  it('接受合法平台', () => {
    expect(validatePlatforms(['windows'])).toEqual(['windows']);
    expect(validatePlatforms(['macos', 'linux'])).toEqual(['macos', 'linux']);
    expect(validatePlatforms(['windows', 'macos', 'linux'])).toEqual(['windows', 'macos', 'linux']);
  });

  it('拒绝空数组', () => {
    expect(() => validatePlatforms([])).toThrow();
  });

  it('拒绝不支持的平台', () => {
    expect(() => validatePlatforms(['android'] as unknown as string[])).toThrow();
  });
});

describe('validateBuildType', () => {
  it('接受合法构建类型', () => {
    expect(validateBuildType('dev')).toBe('dev');
    expect(validateBuildType('release')).toBe('release');
  });

  it('拒绝不支持的构建类型', () => {
    expect(() => validateBuildType('debug')).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// checkUniqueness
// ──────────────────────────────────────────────────────────────────────────

describe('checkUniqueness', () => {
  const baseConfig: BrandBuildConfig = {
    name: 'My Brand',
    identifier: 'my-brand',
    appName: 'My App',
    version: '1.0.0',
    protocolName: 'myapp',
    autostartName: 'My App Autostart',
    dataDirName: 'my-data',
    shortcut: 'Ctrl+Shift+K',
    icons: { universal: 'icon.png' },
    platforms: ['windows'],
    buildType: 'release',
    generateCimatrix: true,
  };

  it('无冲突时通过', () => {
    expect(() => checkUniqueness(baseConfig, [])).not.toThrow();
  });

  it('identifier 冲突', () => {
    expect(() =>
      checkUniqueness(baseConfig, [{ identifier: 'my-brand', protocolName: 'other', autostartName: 'Other', dataDirName: 'other', shortcut: 'Ctrl+Shift+J' }]),
    ).toThrow(UniquenessConflictError);
  });

  it('protocolName 冲突', () => {
    expect(() =>
      checkUniqueness(baseConfig, [{ identifier: 'other-brand', protocolName: 'myapp', autostartName: 'Other', dataDirName: 'other', shortcut: 'Ctrl+Shift+J' }]),
    ).toThrow(UniquenessConflictError);
  });

  it('autostartName 冲突', () => {
    expect(() =>
      checkUniqueness(baseConfig, [{ identifier: 'other-brand', protocolName: 'other', autostartName: 'My App Autostart', dataDirName: 'other', shortcut: 'Ctrl+Shift+J' }]),
    ).toThrow(UniquenessConflictError);
  });

  it('dataDirName 冲突', () => {
    expect(() =>
      checkUniqueness(baseConfig, [{ identifier: 'other-brand', protocolName: 'other', autostartName: 'Other', dataDirName: 'my-data', shortcut: 'Ctrl+Shift+J' }]),
    ).toThrow(UniquenessConflictError);
  });

  it('shortcut 冲突', () => {
    expect(() =>
      checkUniqueness(baseConfig, [{ identifier: 'other-brand', protocolName: 'other', autostartName: 'Other', dataDirName: 'other', shortcut: 'Ctrl+Shift+K' }]),
    ).toThrow(UniquenessConflictError);
  });

  it('冲突错误包含字段名', () => {
    try {
      checkUniqueness(baseConfig, [{ identifier: 'my-brand', protocolName: 'other', autostartName: 'Other', dataDirName: 'other', shortcut: 'Ctrl+Shift+J' }]);
      expect.fail('should have thrown');
    } catch (err) {
      expect((err as Error).message).toContain('identifier');
      expect((err as Error).message).toContain('my-brand');
    }
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateBrandBuildConfig
// ──────────────────────────────────────────────────────────────────────────

describe('validateBrandBuildConfig', () => {
  it('完整配置', () => {
    const config = validateBrandBuildConfig({
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { ico: 'icon.ico', icns: 'icon.icns', png: 'icon.png' },
      platforms: ['windows', 'macos', 'linux'],
      buildType: 'release',
      generateCimatrix: true,
    });
    expect(config.name).toBe('My Brand');
    expect(config.identifier).toBe('my-brand');
    expect(config.appName).toBe('My App');
    expect(config.version).toBe('1.0.0');
    expect(config.protocolName).toBe('myapp');
    expect(config.autostartName).toBe('My App Autostart');
    expect(config.dataDirName).toBe('my-data');
    expect(config.shortcut).toBe('Ctrl+Shift+K');
    expect(config.icons).toEqual({ ico: 'icon.ico', icns: 'icon.icns', png: 'icon.png' });
    expect(config.platforms).toEqual(['windows', 'macos', 'linux']);
    expect(config.buildType).toBe('release');
    expect(config.generateCimatrix).toBe(true);
  });

  it('默认值', () => {
    const config = validateBrandBuildConfig({
      name: 'My Brand',
      identifier: 'my-brand',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
    });
    expect(config.appName).toBe('My Brand'); // 默认等于 name
    expect(config.version).toBe('0.1.0');
    expect(config.platforms).toEqual(['windows', 'macos', 'linux']);
    expect(config.buildType).toBe('release');
    expect(config.generateCimatrix).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// brandBuild
// ──────────────────────────────────────────────────────────────────────────

describe('brandBuild', () => {
  it('成功构建', () => {
    const result = brandBuild({
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { ico: 'icon.ico', icns: 'icon.icns', png: 'icon.png' },
      platforms: ['windows', 'macos', 'linux'],
      buildType: 'release',
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('brand/manifest.json')).toBe(true);
    expect(result.files.has('src-tauri/tauri.conf.json')).toBe(true);
    expect(result.files.has('brand/icon-report.json')).toBe(true);
    expect(result.files.has('brand/ci-matrix.json')).toBe(true);
  });

  it('dev 构建加 .dev 后缀', () => {
    const result = brandBuild({
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows'],
      buildType: 'dev',
    });
    expect(result.ok).toBe(true);
    const tauriConf = JSON.parse(result.files.get('src-tauri/tauri.conf.json')!);
    expect(tauriConf.identifier).toBe('my-brand.dev');
    expect(tauriConf.app.deepLink.protocol).toBe('myapp.dev');
    expect(tauriConf.bundle.autoStart.name).toBe('My App Autostart.dev');
    expect(tauriConf.bundle.dataDir).toBe('my-data.dev');
  });

  it('不生成 CI 矩阵', () => {
    const result = brandBuild({
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows'],
      buildType: 'release',
      generateCimatrix: false,
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('brand/ci-matrix.json')).toBe(false);
  });

  it('唯一性冲突失败', () => {
    const result = brandBuild(
      {
        name: 'My Brand',
        identifier: 'my-brand',
        appName: 'My App',
        version: '1.0.0',
        protocolName: 'myapp',
        autostartName: 'My App Autostart',
        dataDirName: 'my-data',
        shortcut: 'Ctrl+Shift+K',
        icons: { universal: 'icon.png' },
        platforms: ['windows'],
        buildType: 'release',
      },
      [{ identifier: 'my-brand', protocolName: 'other', autostartName: 'Other', dataDirName: 'other', shortcut: 'Ctrl+Shift+J' }],
    );
    expect(result.ok).toBe(false);
    expect(result.error).toContain('唯一性冲突');
  });

  it('验证失败', () => {
    const result = brandBuild({
      name: '',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows'],
      buildType: 'release',
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('品牌名称不能为空');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 文件生成
// ──────────────────────────────────────────────────────────────────────────

describe('generateBrandManifest', () => {
  it('生成有效的 manifest', () => {
    const config: BrandBuildConfig = {
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows'],
      buildType: 'release',
      generateCimatrix: true,
    };
    const manifest = JSON.parse(generateBrandManifest(config));
    expect(manifest.manifestVersion).toBe(1);
    expect(manifest.name).toBe('My Brand');
    expect(manifest.identifier).toBe('my-brand');
    expect(manifest.appName).toBe('My App');
    expect(manifest.version).toBe('1.0.0');
    expect(manifest.protocolName).toBe('myapp');
    expect(manifest.autostartName).toBe('My App Autostart');
    expect(manifest.dataDirName).toBe('my-data');
    expect(manifest.shortcut).toBe('Ctrl+Shift+K');
    expect(manifest.icons).toEqual({ universal: 'icon.png' });
    expect(manifest.platforms).toEqual(['windows']);
    expect(manifest.buildType).toBe('release');
  });
});

describe('generateCiMatrix', () => {
  it('生成 CI 矩阵', () => {
    const config: BrandBuildConfig = {
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows', 'macos', 'linux'],
      buildType: 'release',
      generateCimatrix: true,
    };
    const matrix = JSON.parse(generateCiMatrix(config));
    expect(matrix.version).toBe(1);
    expect(matrix.matrix).toHaveLength(3);
    expect(matrix.matrix[0]).toEqual({ platform: 'windows', buildType: 'release', brand: 'my-brand' });
    expect(matrix.matrix[1]).toEqual({ platform: 'macos', buildType: 'release', brand: 'my-brand' });
    expect(matrix.matrix[2]).toEqual({ platform: 'linux', buildType: 'release', brand: 'my-brand' });
  });

  it('不生成 CI 矩阵', () => {
    const config: BrandBuildConfig = {
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows'],
      buildType: 'release',
      generateCimatrix: false,
    };
    expect(generateCiMatrix(config)).toBe('');
  });
});

describe('generateIconReport', () => {
  it('生成图标报告', () => {
    const config: BrandBuildConfig = {
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { ico: 'icon.ico', icns: 'icon.icns', png: 'icon.png' },
      platforms: ['windows', 'macos', 'linux'],
      buildType: 'release',
      generateCimatrix: true,
    };
    const report = JSON.parse(generateIconReport(config));
    expect(report.version).toBe(1);
    expect(report.reports).toHaveLength(3);
    expect(report.reports[0]).toEqual({ platform: 'windows', status: 'ok', file: 'icon.ico' });
    expect(report.reports[1]).toEqual({ platform: 'macos', status: 'ok', file: 'icon.icns' });
    expect(report.reports[2]).toEqual({ platform: 'linux', status: 'ok', file: 'icon.png' });
  });

  it('universal 图标降级为 warn', () => {
    const config: BrandBuildConfig = {
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { universal: 'icon.png' },
      platforms: ['windows', 'macos', 'linux'],
      buildType: 'release',
      generateCimatrix: true,
    };
    const report = JSON.parse(generateIconReport(config));
    expect(report.reports[0]).toEqual({ platform: 'windows', status: 'warn', file: 'icon.png' });
    expect(report.reports[1]).toEqual({ platform: 'macos', status: 'warn', file: 'icon.png' });
    expect(report.reports[2]).toEqual({ platform: 'linux', status: 'warn', file: 'icon.png' });
  });

  it('缺失图标为 missing', () => {
    const config: BrandBuildConfig = {
      name: 'My Brand',
      identifier: 'my-brand',
      appName: 'My App',
      version: '1.0.0',
      protocolName: 'myapp',
      autostartName: 'My App Autostart',
      dataDirName: 'my-data',
      shortcut: 'Ctrl+Shift+K',
      icons: { ico: 'icon.ico' }, // 只有 ico
      platforms: ['windows', 'macos', 'linux'],
      buildType: 'release',
      generateCimatrix: true,
    };
    const report = JSON.parse(generateIconReport(config));
    expect(report.reports[0]).toEqual({ platform: 'windows', status: 'ok', file: 'icon.ico' });
    expect(report.reports[1]).toEqual({ platform: 'macos', status: 'missing' });
    expect(report.reports[2]).toEqual({ platform: 'linux', status: 'missing' });
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 常量
// ──────────────────────────────────────────────────────────────────────────

describe('常量', () => {
  it('SUPPORTED_PLATFORMS 包含 3 个平台', () => {
    expect(SUPPORTED_PLATFORMS).toHaveLength(3);
    expect(SUPPORTED_PLATFORMS).toContain('windows');
    expect(SUPPORTED_PLATFORMS).toContain('macos');
    expect(SUPPORTED_PLATFORMS).toContain('linux');
  });

  it('SUPPORTED_BUILD_TYPES 包含 2 个类型', () => {
    expect(SUPPORTED_BUILD_TYPES).toHaveLength(2);
    expect(SUPPORTED_BUILD_TYPES).toContain('dev');
    expect(SUPPORTED_BUILD_TYPES).toContain('release');
  });
});
