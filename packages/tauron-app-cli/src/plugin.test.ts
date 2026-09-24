// ──────────────────────────────────────────────────────────────────────────
// 插件脚手架单元测试。
//
// 测试 plugin new/dev/pack/sign/publish 的核心逻辑。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it } from 'vitest';
import {
  pluginScaffold,
  validatePluginConfig,
  validatePluginId,
  validatePluginName,
  validateVersion,
  validatePluginType,
  validatePermissions,
  generatePluginManifest,
  generatePluginPackageJson,
  generatePluginEntry,
  generatePluginTsconfig,
  generatePluginGitignore,
  generatePluginFiles,
  validateDevWatchConfig,
  isPathWithinDir,
  isIgnored,
  SUPPORTED_PLUGIN_TYPES,
  type PluginConfig,
} from '../src/plugin.js';

// ──────────────────────────────────────────────────────────────────────────
// validatePluginId
// ──────────────────────────────────────────────────────────────────────────

describe('validatePluginId', () => {
  it('接受合法 ID', () => {
    expect(validatePluginId('com.example.plugin')).toBe('com.example.plugin');
    expect(validatePluginId('org.test.my-plugin')).toBe('org.test.my-plugin');
    expect(validatePluginId('a.b')).toBe('a.b');
  });

  it('拒绝空 ID', () => {
    expect(() => validatePluginId('')).toThrow();
    expect(() => validatePluginId('   ')).toThrow();
  });

  it('拒绝过短 ID', () => {
    expect(() => validatePluginId('a')).toThrow();
    expect(() => validatePluginId('ab')).toThrow();
  });

  it('拒绝过长 ID', () => {
    expect(() => validatePluginId('a.'.repeat(50))).toThrow();
  });

  it('拒绝非反域名格式', () => {
    expect(() => validatePluginId('plugin')).toThrow();
    expect(() => validatePluginId('.plugin')).toThrow();
    expect(() => validatePluginId('plugin.')).toThrow();
  });

  it('拒绝包含大写字母', () => {
    expect(() => validatePluginId('com.Example.plugin')).toThrow();
  });

  it('拒绝包含下划线', () => {
    expect(() => validatePluginId('com.example.my_plugin')).toThrow();
  });

  it('拒绝以连字符开头或结尾的段', () => {
    expect(() => validatePluginId('com.-example.plugin')).toThrow();
    expect(() => validatePluginId('com.example-.plugin')).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validatePluginName
// ──────────────────────────────────────────────────────────────────────────

describe('validatePluginName', () => {
  it('接受合法名称', () => {
    expect(validatePluginName('My Plugin')).toBe('My Plugin');
    expect(validatePluginName('插件')).toBe('插件');
  });

  it('拒绝空名称', () => {
    expect(() => validatePluginName('')).toThrow();
    expect(() => validatePluginName('   ')).toThrow();
  });

  it('拒绝过长名称', () => {
    expect(() => validatePluginName('a'.repeat(101))).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateVersion
// ──────────────────────────────────────────────────────────────────────────

describe('validateVersion', () => {
  it('接受 semver 格式', () => {
    expect(validateVersion('1.0.0')).toBe('1.0.0');
    expect(validateVersion('0.1.0-beta.1')).toBe('0.1.0-beta.1');
    expect(validateVersion('2.3.4-rc.1')).toBe('2.3.4-rc.1');
  });

  it('拒绝空版本', () => {
    expect(() => validateVersion('')).toThrow();
    expect(() => validateVersion('   ')).toThrow();
  });

  it('拒绝非 semver 格式', () => {
    expect(() => validateVersion('1.0')).toThrow();
    expect(() => validateVersion('v1.0.0')).toThrow();
    expect(() => validateVersion('1.0.0.0')).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validatePluginType
// ──────────────────────────────────────────────────────────────────────────

describe('validatePluginType', () => {
  it('接受所有支持的类型', () => {
    for (const type of SUPPORTED_PLUGIN_TYPES) {
      expect(validatePluginType(type)).toBe(type);
    }
  });

  it('拒绝不支持的类型', () => {
    expect(() => validatePluginType('native')).toThrow();
    expect(() => validatePluginType('hybrid')).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validatePermissions
// ──────────────────────────────────────────────────────────────────────────

describe('validatePermissions', () => {
  it('接受空数组', () => {
    expect(validatePermissions([])).toEqual([]);
  });

  it('接受合法权限', () => {
    const perms = ['tauri:window:allow-close', 'http:allow-request'];
    expect(validatePermissions(perms)).toEqual(perms);
  });

  it('拒绝包含非法字符的权限', () => {
    expect(() => validatePermissions(['bad perm!'])).toThrow();
    expect(() => validatePermissions(['bad/perm'])).toThrow();
  });

  it('拒绝重复权限', () => {
    expect(() => validatePermissions(['perm1', 'perm1'])).toThrow();
    expect(() => validatePermissions(['perm1', 'perm2', 'perm1'])).toThrow();
  });

  it('拒绝非数组', () => {
    expect(() => validatePermissions('perm1' as unknown as string[])).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validatePluginConfig
// ──────────────────────────────────────────────────────────────────────────

describe('validatePluginConfig', () => {
  it('完整配置', () => {
    const config = validatePluginConfig({
      id: 'com.example.plugin',
      name: 'My Plugin',
      description: 'A test plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: ['tauri:window:allow-close'],
      author: 'John',
      license: 'MIT',
      enabled: true,
    });
    expect(config.id).toBe('com.example.plugin');
    expect(config.name).toBe('My Plugin');
    expect(config.pluginType).toBe('js');
    expect(config.version).toBe('1.0.0');
    expect(config.permissions).toEqual(['tauri:window:allow-close']);
    expect(config.author).toBe('John');
    expect(config.license).toBe('MIT');
    expect(config.enabled).toBe(true);
  });

  it('默认值', () => {
    const config = validatePluginConfig({
      id: 'com.example.plugin',
      name: 'My Plugin',
    });
    expect(config.pluginType).toBe('js');
    expect(config.version).toBe('0.1.0');
    expect(config.permissions).toEqual([]);
    expect(config.enabled).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// pluginScaffold
// ──────────────────────────────────────────────────────────────────────────

describe('pluginScaffold', () => {
  it('成功生成 JS 插件', () => {
    const result = pluginScaffold({
      id: 'com.example.plugin',
      name: 'My Plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('manifest.json')).toBe(true);
    expect(result.files.has('package.json')).toBe(true);
    expect(result.files.has('tsconfig.json')).toBe(true);
    expect(result.files.has('src/index.ts')).toBe(true);
  });

  it('生成 process 插件', () => {
    const result = pluginScaffold({
      id: 'com.example.plugin',
      name: 'My Plugin',
      pluginType: 'process',
      version: '1.0.0',
      permissions: [],
    });
    expect(result.ok).toBe(true);
    const entry = result.files.get('src/index.ts');
    expect(entry).toContain('process.stdin');
  });

  it('生成 wasm 插件', () => {
    const result = pluginScaffold({
      id: 'com.example.plugin',
      name: 'My Plugin',
      pluginType: 'wasm',
      version: '1.0.0',
      permissions: [],
    });
    expect(result.ok).toBe(true);
    const entry = result.files.get('src/index.ts');
    expect(entry).toContain('WASM');
  });

  it('生成有效的 manifest.json', () => {
    const result = pluginScaffold({
      id: 'com.example.plugin',
      name: 'My Plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: ['tauri:window:allow-close'],
    });
    const manifest = JSON.parse(result.files.get('manifest.json')!);
    expect(manifest.id).toBe('com.example.plugin');
    expect(manifest.name).toBe('My Plugin');
    expect(manifest.type).toBe('js');
    expect(manifest.version).toBe('1.0.0');
    expect(manifest.permissions).toContain('tauri:window:allow-close');
    expect(manifest.framework).toBeDefined();
    expect(manifest.entry).toBeDefined();
    expect(manifest.entry.js).toBe('src/index.js');
    expect(manifest.platforms).toContain('win');
  });

  it('失败时返回错误', () => {
    const result = pluginScaffold({
      id: 'invalid-id',
      name: 'My Plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: [],
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('插件 ID');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 目录监听（越界检查）
// ──────────────────────────────────────────────────────────────────────────

describe('isPathWithinDir', () => {
  it('目录内的路径', () => {
    expect(isPathWithinDir('/project/plugins/my-plugin', '/project/plugins')).toBe(true);
    expect(isPathWithinDir('/project/plugins', '/project/plugins')).toBe(true);
  });

  it('目录外的路径', () => {
    expect(isPathWithinDir('/other/path', '/project/plugins')).toBe(false);
    expect(isPathWithinDir('/project/plugins/../other', '/project/plugins')).toBe(false);
  });

  it('Windows 路径', () => {
    expect(isPathWithinDir('C:\\project\\plugins\\my-plugin', 'C:\\project\\plugins')).toBe(true);
    expect(isPathWithinDir('C:\\other', 'C:\\project\\plugins')).toBe(false);
  });
});

describe('validateDevWatchConfig', () => {
  it('合法配置', () => {
    const result = validateDevWatchConfig({
      dir: '/project/plugins/my-plugin',
      rootDir: '/project',
      ignorePatterns: ['node_modules/**', 'dist/**'],
      maxFiles: 1000,
      maxFileSize: 10 * 1024 * 1024,
    });
    expect(result.ok).toBe(true);
  });

  it('拒绝空目录', () => {
    const result = validateDevWatchConfig({
      dir: '',
      rootDir: '/project',
      ignorePatterns: [],
      maxFiles: 1000,
      maxFileSize: 1024,
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('目录路径不能为空');
  });

  it('拒绝包含 .. 的路径', () => {
    const result = validateDevWatchConfig({
      dir: '/project/../other',
      rootDir: '/project',
      ignorePatterns: [],
      maxFiles: 1000,
      maxFileSize: 1024,
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('..');
  });

  it('拒绝越界目录', () => {
    const result = validateDevWatchConfig({
      dir: '/other/path',
      rootDir: '/project',
      ignorePatterns: [],
      maxFiles: 1000,
      maxFileSize: 1024,
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('越界');
  });

  it('拒绝过大的 maxFileSize', () => {
    const result = validateDevWatchConfig({
      dir: '/project/plugins/my-plugin',
      rootDir: '/project',
      ignorePatterns: [],
      maxFiles: 1000,
      maxFileSize: 200 * 1024 * 1024, // 200MB > 100MB
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('100MB');
  });

  it('拒绝无效的 maxFiles', () => {
    const result = validateDevWatchConfig({
      dir: '/project/plugins/my-plugin',
      rootDir: '/project',
      ignorePatterns: [],
      maxFiles: 0,
      maxFileSize: 1024,
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('最大文件数');
  });
});

describe('isIgnored', () => {
  it('匹配单星号模式', () => {
    expect(isIgnored('file.js', ['*.js'])).toBe(true);
    expect(isIgnored('file.ts', ['*.js'])).toBe(false);
  });

  it('匹配双星号模式', () => {
    expect(isIgnored('node_modules/pkg/index.js', ['node_modules/**'])).toBe(true);
    expect(isIgnored('src/index.js', ['node_modules/**'])).toBe(false);
  });

  it('匹配问号模式', () => {
    expect(isIgnored('file1.js', ['file?.js'])).toBe(true);
    expect(isIgnored('file12.js', ['file?.js'])).toBe(false);
  });

  it('不匹配非忽略文件', () => {
    expect(isIgnored('src/index.ts', ['node_modules/**', 'dist/**'])).toBe(false);
  });

  it('空模式数组', () => {
    expect(isIgnored('any/file.js', [])).toBe(false);
  });

  it('精确匹配', () => {
    expect(isIgnored('dist', ['dist'])).toBe(true);
    expect(isIgnored('dist/index.js', ['dist'])).toBe(false);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 文件生成函数
// ──────────────────────────────────────────────────────────────────────────

describe('generatePluginManifest', () => {
  it('生成有效的 manifest', () => {
    const config: PluginConfig = {
      id: 'com.example.plugin',
      name: 'My Plugin',
      description: 'A test plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: ['tauri:window:allow-close'],
      enabled: true,
      author: 'John',
      license: 'MIT',
    };
    const manifest = JSON.parse(generatePluginManifest(config));
    expect(manifest.id).toBe('com.example.plugin');
    expect(manifest.name).toBe('My Plugin');
    expect(manifest.type).toBe('js');
    expect(manifest.framework).toBeDefined();
    expect(manifest.entry).toBeDefined();
  });
});

describe('generatePluginPackageJson', () => {
  it('生成有效的 package.json', () => {
    const config: PluginConfig = {
      id: 'com.example.plugin',
      name: 'My Plugin',
      description: 'A test plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: [],
      enabled: true,
    };
    const pkg = JSON.parse(generatePluginPackageJson(config));
    expect(pkg.name).toBe('com.example.plugin');
    expect(pkg.scripts.dev).toBe('tauron plugin dev');
    expect(pkg.scripts.pack).toBe('tauron plugin pack');
  });
});

describe('generatePluginEntry', () => {
  it('生成 JS 插件入口', () => {
    const config: PluginConfig = {
      id: 'com.example.plugin',
      name: 'My Plugin',
      description: 'A test plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: [],
      enabled: true,
    };
    const entry = generatePluginEntry(config);
    expect(entry).toContain('activate');
    expect(entry).toContain('My Plugin');
  });

  it('生成 process 插件入口', () => {
    const config: PluginConfig = {
      id: 'com.example.plugin',
      name: 'My Plugin',
      description: 'A test plugin',
      pluginType: 'process',
      version: '1.0.0',
      permissions: [],
      enabled: true,
    };
    const entry = generatePluginEntry(config);
    expect(entry).toContain('process.stdin');
  });
});

describe('generatePluginFiles', () => {
  it('生成所有必需文件', () => {
    const config: PluginConfig = {
      id: 'com.example.plugin',
      name: 'My Plugin',
      description: 'A test plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: [],
      enabled: true,
    };
    const files = generatePluginFiles(config);
    expect(files.has('manifest.json')).toBe(true);
    expect(files.has('package.json')).toBe(true);
    expect(files.has('tsconfig.json')).toBe(true);
    expect(files.has('.gitignore')).toBe(true);
    expect(files.has('src/index.ts')).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 常量
// ──────────────────────────────────────────────────────────────────────────

describe('SUPPORTED_PLUGIN_TYPES', () => {
  it('包含 3 种类型', () => {
    expect(SUPPORTED_PLUGIN_TYPES).toHaveLength(3);
    expect(SUPPORTED_PLUGIN_TYPES).toContain('js');
    expect(SUPPORTED_PLUGIN_TYPES).toContain('process');
    expect(SUPPORTED_PLUGIN_TYPES).toContain('wasm');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 端到端测试
// ──────────────────────────────────────────────────────────────────────────

describe('端到端：plugin new', () => {
  it('完整流程：验证 → 生成 → 检查文件', () => {
    const result = pluginScaffold({
      id: 'com.example.my-plugin',
      name: 'My Awesome Plugin',
      description: 'A cool plugin',
      pluginType: 'js',
      version: '1.0.0',
      permissions: ['tauri:window:allow-close', 'http:allow-request'],
      author: 'Jane',
      license: 'MIT',
    });

    expect(result.ok).toBe(true);

    // 检查 manifest
    const manifest = JSON.parse(result.files.get('manifest.json')!);
    expect(manifest.id).toBe('com.example.my-plugin');
    expect(manifest.name).toBe('My Awesome Plugin');
    expect(manifest.permissions).toHaveLength(2);

    // 检查 package.json
    const pkg = JSON.parse(result.files.get('package.json')!);
    expect(pkg.name).toBe('com.example.my-plugin');
    expect(pkg.scripts.pack).toBe('tauron plugin pack');

    // 检查入口文件
    const entry = result.files.get('src/index.ts')!;
    expect(entry).toContain('My Awesome Plugin');
    expect(entry).toContain('hello');
  });

  it('全类型矩阵：3 种类型', () => {
    for (const type of SUPPORTED_PLUGIN_TYPES) {
      const result = pluginScaffold({
        id: 'com.example.plugin',
        name: 'My Plugin',
        pluginType: type,
        version: '1.0.0',
        permissions: [],
      });
      expect(result.ok).toBe(true);
      expect(result.files.size).toBeGreaterThan(0);
    }
  });

  it('非法取值路径全部被拒', () => {
    const invalidCases = [
      { id: 'invalid-id', name: 'Plugin' }, // 非反域名
      { id: 'com.example.plugin', name: '' }, // 空名称
      { id: 'com.example.plugin', name: 'Plugin', version: '1.0' }, // 非 semver
      { id: 'com.example.plugin', name: 'Plugin', pluginType: 'native' }, // 不支持的类型
      { id: 'com.example.plugin', name: 'Plugin', permissions: ['bad/perm'] }, // 非法权限
      { id: 'com.example.plugin', name: 'Plugin', permissions: ['perm', 'perm'] }, // 重复权限
    ];

    for (const config of invalidCases) {
      const result = pluginScaffold(config);
      expect(result.ok).toBe(false);
      expect(result.error).toBeDefined();
    }
  });
});
