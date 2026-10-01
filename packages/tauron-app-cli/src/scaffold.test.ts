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
  generateViteConfig,
  generateIconFiles,
  generateTsconfig,
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
import {
  placeholderPng,
  placeholderIco,
  placeholderIcns,
  placeholderIconFiles,
} from '../src/icon-assets.js';

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
    expect(() => validateCapabilities(['host_registry_list', 'fake_cap'])).toThrow(
      ConfigValidationError,
    );
  });

  it('拒绝非数组', () => {
    expect(() => validateCapabilities('host_registry_list' as unknown as string[])).toThrow(
      ConfigValidationError,
    );
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
    // 图标是二进制，走单独一张表（文本与二进制写入路径不同）。
    expect(result.binaryFiles.size).toBe(6);
    expect(result.binaryFiles.has('src-tauri/icons/icon.ico')).toBe(true);
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
    // HTML 入口在工程根：vite 的默认 root 是工程根，放 `src/` 下 rollup 找不到入口。
    const html = result.files.get('index.html');
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
    // 脚本取 Tauri 官方模板规范形状：`dev` 起前端、`tauri` 起 Tauri CLI。
    expect(pkg.scripts.dev).toBe('vite');
    expect(pkg.scripts.build).toBe('vite build');
    expect(pkg.scripts.tauri).toBe('tauri');
    // 普通使用固定 registry 版本，避免无范围浮动到破坏性更新。
    expect(pkg.dependencies['@tauron/host']).toBe('1.0.2');
    const cargo = scaffold({
      name: 'my-app',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    }).files.get('src-tauri/Cargo.toml');
    expect(cargo).toContain('tauron-shell = { version = "=1.0.2"');
    expect(cargo).toContain('tauron-adapter = { version = "=1.0.2"');
    expect(cargo).not.toContain('path =');
    // `tauri dev` / `tauri build` 两个 script 的执行体必须来自 devDependencies。
    expect(pkg.devDependencies['@tauri-apps/cli']).toBeDefined();
    // before*Command 会调到 `npm run dev` / `npm run build`，执行体是 vite。
    expect(pkg.devDependencies.vite).toBeDefined();
    expect(pkg.dependencies.react).toBeDefined();
  });

  it('给了 tauronPath 时生成可解析的 file: 依赖', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
      tauronPath: '../..',
    };
    const pkg = JSON.parse(generatePackageJson(config));
    expect(pkg.dependencies['@tauron/host']).toBe('file:../../packages/tauron-host');
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

    // svelte 编译进产物、不进运行时依赖，因此只在 devDependencies。
    const sveltePkg = JSON.parse(generatePackageJson({ ...base, framework: 'svelte' }));
    expect(sveltePkg.dependencies.svelte).toBeUndefined();
    expect(sveltePkg.devDependencies.svelte).toBeDefined();
    expect(sveltePkg.devDependencies['@sveltejs/vite-plugin-svelte']).toBeDefined();

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

  it('build 段带 before*Command 钩子（一键起前端的关键）', () => {
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
    // 缺 beforeDevCommand 时 `tauri dev` 会去连没人监听的 devUrl；缺
    // beforeBuildCommand 时 `tauri build` 会去读不存在的 dist/。
    expect(conf.build.beforeDevCommand).toBe('npm run dev');
    expect(conf.build.beforeBuildCommand).toBe('npm run build');
    expect(conf.build.frontendDist).toBe('../dist');
    expect(conf.build.devUrl).toBe('http://localhost:1420');
  });

  it('bundle.icon 声明脚手架实际产出的占位图标', () => {
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
    expect(conf.bundle.icon).toContain('icons/icon.ico');
    expect(conf.bundle.icon).toContain('icons/icon.icns');
    expect(conf.bundle.icon).toContain('icons/32x32.png');
  });
});

describe('generateViteConfig', () => {
  const base: Omit<ScaffoldConfig, 'framework'> = {
    name: 'my-app',
    slug: 'my-app',
    description: 'Test',
    targets: ['desktop'],
    shell: 'tauri',
    capabilities: [],
  };

  it('端口与 devUrl 一致，outDir 与 frontendDist 一致', () => {
    const vite = generateViteConfig({ ...base, framework: 'react' });
    // port 必须等于 tauri.conf.json 的 devUrl；strictPort 让占用变成显式失败。
    expect(vite).toContain('port: 1420');
    expect(vite).toContain('strictPort: true');
    // outDir 必须等于 tauri.conf.json 的 frontendDist（../dist）。
    expect(vite).toContain("outDir: 'dist'");
    // 排除 src-tauri：否则 Rust 编译产物会触发前端热重载风暴。
    expect(vite).toContain('**/src-tauri/**');
    // Tauri 自定义协议下绝对路径会 404。
    expect(vite).toContain("base: './'");
  });

  it('React 带 plugin-react', () => {
    const vite = generateViteConfig({ ...base, framework: 'react' });
    expect(vite).toContain('@vitejs/plugin-react');
    expect(vite).toContain('plugins: [react()]');
  });

  it('Vue 带 plugin-vue', () => {
    const vite = generateViteConfig({ ...base, framework: 'vue' });
    expect(vite).toContain('@vitejs/plugin-vue');
    expect(vite).toContain('plugins: [vue()]');
  });

  it('Svelte 带 vite-plugin-svelte', () => {
    const vite = generateViteConfig({ ...base, framework: 'svelte' });
    expect(vite).toContain('@sveltejs/vite-plugin-svelte');
    expect(vite).toContain('plugins: [svelte()]');
  });

  it('vanilla 无插件 import、plugins 为空数组', () => {
    const vite = generateViteConfig({ ...base, framework: 'vanilla' });
    expect(vite).toContain('plugins: []');
    expect(vite).not.toContain('@vitejs/plugin-');
    expect(vite).not.toContain('@sveltejs/vite-plugin-svelte');
  });
});

describe('generateIconFiles', () => {
  const base: Omit<ScaffoldConfig, 'shell'> = {
    name: 'my-app',
    slug: 'my-app',
    description: 'Test',
    framework: 'react',
    targets: ['desktop'],
    capabilities: [],
  };

  it('Tauri 壳产出 6 个占位图标，路径带 src-tauri/icons/ 前缀', () => {
    const icons = generateIconFiles({ ...base, shell: 'tauri' });
    expect(icons.size).toBe(6);
    for (const key of icons.keys()) {
      expect(key.startsWith('src-tauri/icons/')).toBe(true);
    }
    // Windows 资源文件要 .ico、macOS 打包要 .icns、Linux 要两个 PNG。
    expect(icons.has('src-tauri/icons/icon.ico')).toBe(true);
    expect(icons.has('src-tauri/icons/icon.icns')).toBe(true);
    expect(icons.has('src-tauri/icons/32x32.png')).toBe(true);
    expect(icons.has('src-tauri/icons/128x128.png')).toBe(true);
    // 缺这组文件 `tauri-build` 直接失败（不是「打包才需要」）。
    expect(icons.get('src-tauri/icons/icon.ico')!.length).toBeGreaterThan(0);
  });

  it('非 Tauri 壳不产图标（Electron 图标不在 src-tauri/ 下）', () => {
    expect(generateIconFiles({ ...base, shell: 'electron' }).size).toBe(0);
  });
});

describe('icon-assets（占位图标字节）', () => {
  const startsWith = (bytes: Uint8Array, prefix: number[]): boolean =>
    prefix.every((b, i) => bytes[i] === b);

  it('PNG 带合法签名与 IHDR / IEND 块', () => {
    const png = placeholderPng(32);
    // 8 字节 PNG 签名——`tauri-bundler` / 图片库据此识别格式。
    expect(startsWith(png, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])).toBe(true);
    const text = new TextDecoder('latin1').decode(png);
    expect(text).toContain('IHDR');
    expect(text).toContain('IDAT');
    expect(text).toContain('IEND');
  });

  it('PNG 逐字节确定（同尺寸两次调用完全相等）', () => {
    expect(placeholderPng(32)).toEqual(placeholderPng(32));
  });

  it('ICO 头部合法：reserved=0 / type=1 / 单帧 / 尺寸=32', () => {
    const ico = placeholderIco(32);
    // ICONDIR：reserved(2) + type(2) + count(2)
    expect(ico[0]).toBe(0);
    expect(ico[1]).toBe(0);
    expect(ico[2]).toBe(1); // type = 1（图标，非光标）
    expect(ico[3]).toBe(0);
    expect(ico[4]).toBe(1); // 帧数 = 1
    expect(ico[5]).toBe(0);
    // ICONDIRENTRY 的宽/高字段：32 可直接表示
    expect(ico[6]).toBe(32);
    expect(ico[7]).toBe(32);
  });

  it('ICNS 魔术数与长度字段自洽', () => {
    const icns = placeholderIcns(128);
    const text = new TextDecoder('latin1').decode(icns.slice(0, 4));
    expect(text).toBe('icns');
    // 文件第 4–7 字节是大端总长度，必须等于实际字节数。
    const declared = (icns[4]! << 24) | (icns[5]! << 16) | (icns[6]! << 8) | icns[7]!;
    expect(declared >>> 0).toBe(icns.length);
  });

  it('placeholderIconFiles 覆盖三平台所需的最小集合', () => {
    const icons = placeholderIconFiles();
    expect([...icons.keys()].sort()).toEqual([
      '128x128.png',
      '128x128@2x.png',
      '32x32.png',
      'icon.icns',
      'icon.ico',
      'icon.png',
    ]);
    for (const bytes of icons.values()) {
      expect(bytes.length).toBeGreaterThan(0);
    }
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
    // 不自造 extends：`@tauron/base-tsconfig` 在本仓不存在，extends 它必报错。
    expect(tsconfig.extends).toBeUndefined();
    expect(tsconfig.compilerOptions.outDir).toBe('dist');
    expect(tsconfig.compilerOptions.strict).toBe(true);
    expect(tsconfig.compilerOptions.jsx).toBe('react-jsx');
  });

  it('非 React 框架不带 jsx', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'vue',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    };
    const tsconfig = JSON.parse(generateTsconfig(config));
    expect(tsconfig.compilerOptions.jsx).toBeUndefined();
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
    expect(files.get('pnpm-workspace.yaml')).toContain('esbuild: true');
    expect(files.has('tsconfig.json')).toBe(true);
    expect(files.has('.gitignore')).toBe(true);
    expect(files.has('index.html')).toBe(true);
    expect(files.has('vite.config.ts')).toBe(true);
    expect(files.has('src/main.ts')).toBe(true);
    expect(files.has('src/capabilities.json')).toBe(true);
    expect(files.has('src-tauri/tauri.conf.json')).toBe(true);
    expect(files.has('src-tauri/Cargo.toml')).toBe(true);
    expect(files.has('src-tauri/src/main.rs')).toBe(true);
  });

  it('Tauri 工程完整性：capabilities/default.json 与 build.rs', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'tauri',
      capabilities: [],
    };
    const files = generateFiles(config);

    // Tauri v2：不匹配任何 capability 的 webview 完全没有 IPC 访问权（界面空白）。
    const capability = files.get('src-tauri/capabilities/default.json');
    expect(capability).toBeDefined();
    const parsed = JSON.parse(capability!) as {
      identifier: string;
      windows: string[];
      permissions: string[];
    };
    expect(parsed.identifier).toBe('default');
    // 插件面板窗 label 恒为 `plugin-<插件 id>`，必须一并覆盖。
    expect(parsed.windows).toContain('main');
    expect(parsed.windows).toContain('plugin-*');
    expect(parsed.permissions).toEqual(['core:default']);
    // 不该伪造 tauron 命名空间权限：host_* 走 root 注册，不受插件 ACL 管辖。
    expect(parsed.permissions.some((p) => p.startsWith('core:host_'))).toBe(false);

    // 缺 build.rs 时 `cargo check` 会在 tauri::generate_context!() 处失败。
    expect(files.get('src-tauri/build.rs')).toContain('tauri_build::build()');
  });

  it('非 Tauri 壳不生成 src-tauri 产物', () => {
    const config: ScaffoldConfig = {
      name: 'my-app',
      slug: 'my-app',
      description: 'Test',
      framework: 'react',
      targets: ['desktop'],
      shell: 'electron',
      capabilities: [],
    };
    const files = generateFiles(config);
    expect(files.has('src-tauri/build.rs')).toBe(false);
    expect(files.has('src-tauri/capabilities/default.json')).toBe(false);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// tauron 装配（Cargo.toml / main.rs）——「一键」是否真的接了 tauron
// ──────────────────────────────────────────────────────────────────────────

describe('tauron 装配', () => {
  const config: ScaffoldConfig = {
    name: 'my-app',
    slug: 'my-app',
    description: 'Test',
    framework: 'react',
    targets: ['desktop'],
    shell: 'tauri',
    capabilities: [],
    tauronPath: '../../..',
  };

  it('给了 tauronPath：Cargo.toml 生成可编译的 path 依赖 + 两档 feature', () => {
    const cargo = generateFiles(config).get('src-tauri/Cargo.toml')!;
    // tauronPath 以工程根为基准，Cargo.toml 在 src-tauri/ 下，所以多一层 ..。
    expect(cargo).toContain(
      'tauron-shell = { version = "=1.0.2", path = "../../../../crates/tauron-shell"',
    );
    expect(cargo).toContain(
      'tauron-adapter = { version = "=1.0.2", path = "../../../../crates/tauron-adapter"',
    );
    expect(cargo).toContain('features = ["tauri"]');
    expect(cargo).toContain('default = ["plugin-install"]');
    expect(cargo).toContain('substrate-only = []');
    expect(cargo).toContain('tauron-adapter/plugin-install');
    // 本地 checkout 仍固定版本，避免路径源码与发布版本错配。
    expect(cargo).toContain('version = "=1.0.2"');
  });

  it('没给 tauronPath：使用精确 registry 版本，不写本地路径', () => {
    const withoutPath: ScaffoldConfig = { ...config };
    delete withoutPath.tauronPath;
    const cargo = generateFiles(withoutPath).get('src-tauri/Cargo.toml')!;
    expect(cargo).toContain('tauron-adapter = { version = "=1.0.2"');
    expect(cargo).not.toContain('path =');
  });

  it('main.rs 生成真装配，不是裸 Builder', () => {
    const mainRs = generateFiles(config).get('src-tauri/src/main.rs')!;
    // 默认档：底座 + 插件运行时（85 条）
    expect(mainRs).toContain('tauron_adapter::tauri::state_init_with_adapter_config');
    expect(mainRs).toContain('tauron_adapter::tauron_generate_handler![]');
    expect(mainRs).toContain('tauron_adapter::tauri::cleanup_closed_window');
    expect(mainRs).toContain('on_window_event');
    expect(mainRs).toContain('TAURON_CLIENT_CONFIG');
    // substrate-only 档：61 条底座命令
    expect(mainRs).toContain('tauron_adapter::SubstrateState::with_adapter_config');
    expect(mainRs).toContain('tauron_adapter::tauron_substrate_handler![]');
    // 生成物自身不是裸 Builder（裸 Builder 与 tauron 零关联，等于没接）
    expect(mainRs).not.toContain('tauri::Builder::default()\n    .run(');
  });

  it('tauronPath 透传：前后多余空白被裁掉', () => {
    const padded = validateConfig({ name: 'my-app', tauronPath: '  ../..  ' });
    expect(padded.tauronPath).toBe('../..');
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
    // dev / build 交给 vite（Tauri 官方模板形状）；`npm run tauri dev` 由
    // tauri.conf.json 的 beforeDevCommand 拉起 vite，因此这里断言的是 vite。
    expect(pkg.scripts.dev).toBe('vite');
    expect(pkg.scripts.build).toBe('vite build');

    const tauriConf = JSON.parse(result.files.get('src-tauri/tauri.conf.json')!);
    expect(tauriConf.productName).toBe('my-portfolio');
    expect(tauriConf.identifier).toBe('com.tauron.my-portfolio');

    const caps = JSON.parse(result.files.get('src/capabilities.json')!);
    expect(caps.capabilities).toContain('host_registry_list');
    expect(caps.capabilities).toContain('host_lifecycle_report');

    // vite 的 root 是工程根，入口 `index.html` 必须在根（放 src/ 下 rollup 报
    // "Could not resolve entry"）。
    const html = result.files.get('index.html')!;
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
