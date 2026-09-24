// ──────────────────────────────────────────────────────────────────────────
// 脚手架单元测试。
//
// 测试 create-tauron 脚手架的核心逻辑。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it } from 'vitest';
import {
  scaffold,
  validateConfig,
  validateName,
  validateFramework,
  validateTargets,
  validateShell,
  validateCapabilities,
  toSlug,
  generatePackageJson,
  generateTauriConfig,
  generateTsconfig,
  generateAppEntry,
  generateCapabilitiesJson,
  generateGitignore,
  generateFiles,
  InvalidTargetError,
  ConfigValidationError,
  SUPPORTED_FRAMEWORKS,
  SUPPORTED_TARGETS,
  SUPPORTED_SHELLS,
  UNSUPPORTED_TARGETS,
  type ScaffoldConfig,
} from '../src/scaffold.js';

// ──────────────────────────────────────────────────────────────────────────
// validateName
// ──────────────────────────────────────────────────────────────────────────

describe('validateName', () => {
  it('接受合法名称', () => {
    expect(validateName('my-app')).toBe('my-app');
    expect(validateName('app123')).toBe('app123');
    expect(validateName('a-b-c')).toBe('a-b-c');
  });

  it('拒绝空名称', () => {
    expect(() => validateName('')).toThrow(ConfigValidationError);
    expect(() => validateName('   ')).toThrow(ConfigValidationError);
  });

  it('拒绝过短名称', () => {
    expect(() => validateName('ab')).toThrow(ConfigValidationError);
  });

  it('拒绝过长名称', () => {
    expect(() => validateName('a'.repeat(51))).toThrow(ConfigValidationError);
  });

  it('拒绝包含大写字母的名称', () => {
    expect(() => validateName('MyApp')).toThrow(ConfigValidationError);
  });

  it('拒绝包含下划线的名称', () => {
    expect(() => validateName('my_app')).toThrow(ConfigValidationError);
  });

  it('拒绝以连字符开头的名称', () => {
    expect(() => validateName('-my-app')).toThrow(ConfigValidationError);
  });

  it('拒绝以连字符结尾的名称', () => {
    expect(() => validateName('my-app-')).toThrow(ConfigValidationError);
  });

  it('拒绝包含连续连字符的名称', () => {
    expect(() => validateName('my--app')).toThrow(ConfigValidationError);
  });

  it('拒绝包含空格的名称', () => {
    expect(() => validateName('my app')).toThrow(ConfigValidationError);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// toSlug
// ──────────────────────────────────────────────────────────────────────────

describe('toSlug', () => {
  it('转换合法名称', () => {
    expect(toSlug('my-app')).toBe('my-app');
    expect(toSlug('app123')).toBe('app123');
  });

  it('替换非法字符为连字符', () => {
    expect(toSlug('My App')).toBe('my-app');
    expect(toSlug('my_app')).toBe('my-app');
    expect(toSlug('My-App')).toBe('my-app'); // 大写字母被替换
  });

  it('合并连续连字符', () => {
    expect(toSlug('my--app')).toBe('my-app');
  });

  it('移除首尾连字符', () => {
    expect(toSlug('-my-app-')).toBe('my-app');
  });

  it('转换中文为连字符', () => {
    // 非 ASCII 字符被替换为连字符，然后移除首尾连字符
    expect(toSlug('我的应用')).toBe(''); // 全部替换后为空
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateFramework
// ──────────────────────────────────────────────────────────────────────────

describe('validateFramework', () => {
  it('接受所有支持的框架', () => {
    for (const fw of SUPPORTED_FRAMEWORKS) {
      expect(validateFramework(fw)).toBe(fw);
    }
  });

  it('拒绝不支持的框架', () => {
    expect(() => validateFramework('angular')).toThrow(ConfigValidationError);
    expect(() => validateFramework('solid')).toThrow(ConfigValidationError);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateTargets
// ──────────────────────────────────────────────────────────────────────────

describe('validateTargets', () => {
  it('接受合法目标', () => {
    expect(validateTargets(['desktop'])).toEqual(['desktop']);
    expect(validateTargets(['mobile'])).toEqual(['mobile']);
    expect(validateTargets(['desktop', 'mobile'])).toEqual(['desktop', 'mobile']);
  });

  it('拒绝空数组', () => {
    expect(() => validateTargets([])).toThrow(ConfigValidationError);
  });

  it('拒绝 ADR-16 排除的目标', () => {
    expect(() => validateTargets(['ios'])).toThrow(InvalidTargetError);
    expect(() => validateTargets(['android'])).toThrow(InvalidTargetError);
    expect(() => validateTargets(['web'])).toThrow(InvalidTargetError);
  });

  it('InvalidTargetError 包含 ADR-16 说明', () => {
    try {
      validateTargets(['web']);
      expect.fail('should have thrown');
    } catch (err) {
      expect(err).toBeInstanceOf(InvalidTargetError);
      expect((err as Error).message).toContain('ADR-16');
      expect((err as Error).message).toContain('Web 目标产物明确排除');
    }
  });

  it('拒绝不支持的目标', () => {
    expect(() => validateTargets(['linux'] as unknown as string[])).toThrow(ConfigValidationError);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateShell
// ──────────────────────────────────────────────────────────────────────────

describe('validateShell', () => {
  it('接受所有支持的壳', () => {
    for (const shell of SUPPORTED_SHELLS) {
      expect(validateShell(shell)).toBe(shell);
    }
  });

  it('拒绝不支持的壳', () => {
    expect(() => validateShell('flutter')).toThrow(ConfigValidationError);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateCapabilities
// ──────────────────────────────────────────────────────────────────────────

describe('validateCapabilities', () => {
  it('接受空数组', () => {
    expect(validateCapabilities([])).toEqual([]);
  });

  it('接受已注册的能力', () => {
    // 使用 CAPABILITIES 中的实际命令名
    const knownCaps = ['host_registry_list', 'host_registry_admin', 'host_lifecycle_report'];
    expect(validateCapabilities(knownCaps)).toEqual(knownCaps);
  });

  it('拒绝未知能力', () => {
    expect(() => validateCapabilities(['unknown_cap'])).toThrow(ConfigValidationError);
    expect(() => validateCapabilities(['host_registry_list', 'fake_cap'])).toThrow(ConfigValidationError);
  });

  it('拒绝非数组', () => {
    expect(() => validateCapabilities('host_registry_list' as unknown as string[])).toThrow(ConfigValidationError);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateConfig
// ──────────────────────────────────────────────────────────────────────────

describe('validateConfig', () => {
  it('完整配置', () => {
    const config = validateConfig({
      name: 'my-app',
      description: 'My Test App',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: ['host_registry_list'],
    });
    expect(config.name).toBe('my-app');
    expect(config.slug).toBe('my-app');
    expect(config.description).toBe('My Test App');
    expect(config.framework).toBe('react');
    expect(config.targets).toEqual(['desktop']);
    expect(config.shell).toBe('tauri');
    expect(config.capabilities).toEqual(['host_registry_list']);
  });

  it('默认值', () => {
    const config = validateConfig({
      name: 'my-app',
    });
    expect(config.framework).toBe('react');
    expect(config.targets).toEqual(['desktop']);
    expect(config.shell).toBe('tauri');
    expect(config.capabilities).toEqual([]);
  });

  it('带品牌配置', () => {
    const config = validateConfig({
      name: 'my-app',
      brand: 'my-brand',
    });
    expect(config.brand).toBe('my-brand');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// scaffold
// ──────────────────────────────────────────────────────────────────────────

describe('scaffold', () => {
  it('成功生成文件', () => {
    const result = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.size).toBeGreaterThan(0);
    expect(result.files.has('package.json')).toBe(true);
    expect(result.files.has('tsconfig.json')).toBe(true);
    expect(result.files.has('.gitignore')).toBe(true);
  });

  it('生成 React 入口', () => {
    const result = scaffold({
      name: 'my-react-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('src/main.ts')).toBe(true);
    const entry = result.files.get('src/main.ts');
    expect(entry).toContain('React');
    expect(entry).toContain('my-react-app');
  });

  it('生成 Vue 入口', () => {
    const result = scaffold({
      name: 'my-vue-app',
      framework: 'vue',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('src/main.ts')).toBe(true);
    const entry = result.files.get('src/main.ts');
    expect(entry).toContain('vue');
    expect(entry).toContain('my-vue-app');
  });

  it('生成 Svelte 入口', () => {
    const result = scaffold({
      name: 'my-svelte-app',
      framework: 'svelte',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('src/main.svelte')).toBe(true);
  });

  it('生成 Vanilla 入口', () => {
    const result = scaffold({
      name: 'my-vanilla-app',
      framework: 'vanilla',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('src/main.ts')).toBe(true);
    const entry = result.files.get('src/main.ts');
    expect(entry).toContain('DOMContentLoaded');
  });

  it('生成 Tauri 配置', () => {
    const result = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('src-tauri/tauri.conf.json')).toBe(true);
    expect(result.files.has('src-tauri/Cargo.toml')).toBe(true);
    expect(result.files.has('src-tauri/src/main.rs')).toBe(true);
  });

  it('非 Tauri 壳不生成 Tauri 文件', () => {
    const result = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'electron',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    expect(result.files.has('src-tauri/tauri.conf.json')).toBe(false);
  });

  it('生成 capabilities.json', () => {
    const result = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: ['host_registry_list', 'host_lifecycle_report'],
    });
    expect(result.ok).toBe(true);
    const caps = result.files.get('src/capabilities.json');
    expect(caps).toContain('host_registry_list');
    expect(caps).toContain('host_lifecycle_report');
  });

  it('生成 HTML 入口', () => {
    const result = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(true);
    const html = result.files.get('src/index.html');
    expect(html).toContain('<!DOCTYPE html>');
    expect(html).toContain('my-app');
    expect(html).toContain('id="root"');
  });

  it('失败时返回错误信息', () => {
    const result = scaffold({
      name: 'invalid name!',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('项目名称');
  });

  it('拒绝 ADR-16 目标', () => {
    const result = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['web'],
      shell: 'tauri',
      capabilities: [],
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('ADR-16');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 文件生成函数
// ──────────────────────────────────────────────────────────────────────────

describe('generatePackageJson', () => {
  it('生成有效的 package.json', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    };
    const pkg = JSON.parse(generatePackageJson(config));
    expect(pkg.name).toBe('my-app');
    expect(pkg.scripts.dev).toBe('tauri dev');
    expect(pkg.dependencies['@tauron/host']).toBe('workspace:*');
    expect(pkg.dependencies.react).toBeDefined();
  });

  it('不同框架生成不同依赖', () => {
    const base: Omit<ScaffoldConfig, 'framework'> = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    };
    const reactPkg = JSON.parse(generatePackageJson({ ...base, framework: 'react' }));
    expect(reactPkg.dependencies.react).toBeDefined();

    const vuePkg = JSON.parse(generatePackageJson({ ...base, framework: 'vue' }));
    expect(vuePkg.dependencies.vue).toBeDefined();

    const sveltePkg = JSON.parse(generatePackageJson({ ...base, framework: 'svelte' }));
    expect(sveltePkg.dependencies.svelte).toBeDefined();

    const vanillaPkg = JSON.parse(generatePackageJson({ ...base, framework: 'vanilla' }));
    expect(vanillaPkg.dependencies.react).toBeUndefined();
    expect(vanillaPkg.dependencies.vue).toBeUndefined();
  });
});

describe('generateTauriConfig', () => {
  it('生成有效的 tauri.conf.json', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    };
    const conf = JSON.parse(generateTauriConfig(config));
    expect(conf.productName).toBe('my-app');
    expect(conf.identifier).toBe('com.tauron.my-app');
    expect(conf.app.windows[0].title).toBe('Test');
  });
});

describe('generateTsconfig', () => {
  it('生成有效的 tsconfig.json', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    };
    const tsconfig = JSON.parse(generateTsconfig(config));
    expect(tsconfig.extends).toBe('@tauron/base-tsconfig');
    expect(tsconfig.compilerOptions.outDir).toBe('dist');
  });
});

describe('generateGitignore', () => {
  it('包含必要条目', () => {
    const gitignore = generateGitignore();
    expect(gitignore).toContain('node_modules/');
    expect(gitignore).toContain('dist/');
    expect(gitignore).toContain('.env');
  });
});

describe('generateFiles', () => {
  it('生成所有必需文件', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: ['host_registry_list'],
    };
    const files = generateFiles(config);
    expect(files.has('package.json')).toBe(true);
    expect(files.has('tsconfig.json')).toBe(true);
    expect(files.has('.gitignore')).toBe(true);
    expect(files.has('src/index.html')).toBe(true);
    expect(files.has('src/main.ts')).toBe(true);
    expect(files.has('src/capabilities.json')).toBe(true);
    expect(files.has('src-tauri/tauri.conf.json')).toBe(true);
    expect(files.has('src-tauri/Cargo.toml')).toBe(true);
    expect(files.has('src-tauri/src/main.rs')).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 常量
// ──────────────────────────────────────────────────────────────────────────

describe('常量', () => {
  it('SUPPORTED_FRAMEWORKS 包含 4 个框架', () => {
    expect(SUPPORTED_FRAMEWORKS).toHaveLength(4);
    expect(SUPPORTED_FRAMEWORKS).toContain('react');
    expect(SUPPORTED_FRAMEWORKS).toContain('vue');
    expect(SUPPORTED_FRAMEWORKS).toContain('svelte');
    expect(SUPPORTED_FRAMEWORKS).toContain('vanilla');
  });

  it('SUPPORTED_TARGETS 包含 2 个目标', () => {
    expect(SUPPORTED_TARGETS).toHaveLength(2);
    expect(SUPPORTED_TARGETS).toContain('desktop');
    expect(SUPPORTED_TARGETS).toContain('mobile');
  });

  it('UNSUPPORTED_TARGETS 包含 ADR-16 排除的目标', () => {
    expect(UNSUPPORTED_TARGETS).toHaveProperty('ios');
    expect(UNSUPPORTED_TARGETS).toHaveProperty('android');
    expect(UNSUPPORTED_TARGETS).toHaveProperty('web');
    for (const reason of Object.values(UNSUPPORTED_TARGETS)) {
      expect(reason).toContain('ADR-16');
    }
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 端到端测试
// ──────────────────────────────────────────────────────────────────────────

describe('端到端：create-tauron', () => {
  it('完整流程：验证 → 生成 → 检查文件', () => {
    const result = scaffold({
      name: 'my-portfolio',
      description: 'My Portfolio App',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: ['host_registry_list', 'host_lifecycle_report'],
    });

    expect(result.ok).toBe(true);
    expect(result.error).toBeUndefined();

    // 检查生成的文件
    const pkg = JSON.parse(result.files.get('package.json')!);
    expect(pkg.name).toBe('my-portfolio');
    expect(pkg.scripts.dev).toBe('tauri dev');

    const tauriConf = JSON.parse(result.files.get('src-tauri/tauri.conf.json')!);
    expect(tauriConf.productName).toBe('my-portfolio');
    expect(tauriConf.identifier).toBe('com.tauron.my-portfolio');

    const caps = JSON.parse(result.files.get('src/capabilities.json')!);
    expect(caps.capabilities).toContain('host_registry_list');
    expect(caps.capabilities).toContain('host_lifecycle_report');

    const html = result.files.get('src/index.html')!;
    expect(html).toContain('my-portfolio');

    const entry = result.files.get('src/main.ts')!;
    expect(entry).toContain('my-portfolio');
  });

  it('全矩阵：4 框架 × 1 目标 × 1 壳 = 4 组合', () => {
    const frameworks: string[] = ['react', 'vue', 'svelte', 'vanilla'];
    for (const fw of frameworks) {
      const result = scaffold({
        name: `app-${fw}`,
        framework: fw as 'react',
        targets: ['desktop'],
        shell: 'tauri',
        capabilities: [],
      });
      expect(result.ok).toBe(true);
      expect(result.files.size).toBeGreaterThan(0);
    }
  });

  it('非法取值路径全部被拒', () => {
    const invalidCases = [
      { name: 'ab' }, // 太短
      { name: 'my_app' }, // 下划线
      { name: 'My App' }, // 空格和大写
      { name: 'my-app', framework: 'angular' }, // 不支持的框架
      { name: 'my-app', targets: ['web'] }, // ADR-16
      { name: 'my-app', targets: ['ios'] }, // ADR-16
      { name: 'my-app', shell: 'flutter' }, // 不支持的壳
      { name: 'my-app', capabilities: ['fake_cap'] }, // 未知能力
    ];

    for (const config of invalidCases) {
      const result = scaffold(config);
      expect(result.ok).toBe(false);
      expect(result.error).toBeDefined();
    }
  });
});
