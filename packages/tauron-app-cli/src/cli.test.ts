import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { main } from './cli.js';

describe('plugin pack CLI', () => {
  const previousExitCode = process.exitCode;
  afterEach(() => {
    process.exitCode = previousExitCode;
    vi.restoreAllMocks();
  });

  it('creates a real .tpkg and signs the files read back from that archive', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-pack-'));
    const pluginDir = path.join(root, 'plugin');
    fs.mkdirSync(pluginDir);
    fs.writeFileSync(
      path.join(pluginDir, 'manifest.json'),
      JSON.stringify({
        id: 'com.example.plugin',
        name: 'Example',
        version: '1.0.0',
        type: 'js',
        permissions: [],
      }),
    );
    fs.mkdirSync(path.join(pluginDir, 'src'));
    fs.writeFileSync(path.join(pluginDir, 'src', 'index.js'), 'export const value = 7;');
    const output = path.join(root, 'plugin.tpkg');
    const keyPath = path.join(root, 'private.pem');
    const keys = crypto.generateKeyPairSync('ed25519', {
      privateKeyEncoding: { type: 'pkcs8', format: 'pem' },
      publicKeyEncoding: { type: 'spki', format: 'pem' },
    });
    fs.writeFileSync(keyPath, keys.privateKey);
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'plugin', 'pack', '--dir', pluginDir, '--output', output]);
      expect(process.exitCode).toBe(0);
      expect(fs.readFileSync(output).readUInt32LE(0)).toBe(0x04034b50);

      await main([
        'node',
        'cli.js',
        'plugin',
        'sign',
        '--file',
        output,
        '--key',
        keyPath,
        '--kid',
        'test-key',
      ]);
      expect(process.exitCode).toBe(0);
      const signature = JSON.parse(fs.readFileSync(`${output}.sig`, 'utf8')) as {
        files: Array<{ path: string; hash: string }>;
      };
      expect(signature.files.map((file) => file.path).sort()).toEqual([
        'manifest.json',
        'src/index.js',
      ]);
      expect(signature.files.every((file) => /^[a-f0-9]{64}$/.test(file.hash))).toBe(true);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
});

describe('app scaffold CLI（一键路径）', () => {
  const previousExitCode = process.exitCode;
  afterEach(() => {
    process.exitCode = previousExitCode;
    vi.restoreAllMocks();
  });

  it('tauron-app new 默认使用固定 registry 版本并生成已接线工程', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-new-'));
    const target = path.join(root, 'my-app');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'new', target, '--name', 'my-app']);
      expect(process.exitCode).toBe(0);

      // Tauri 工程完整性：capability 与 build.rs 必须落下来
      expect(fs.existsSync(path.join(target, 'src-tauri', 'capabilities', 'default.json'))).toBe(
        true,
      );
      expect(fs.readFileSync(path.join(target, 'src-tauri', 'build.rs'), 'utf8')).toContain(
        'tauri_build::build()',
      );

      const cargo = fs.readFileSync(path.join(target, 'src-tauri', 'Cargo.toml'), 'utf8');
      expect(cargo).toContain('tauron-adapter = { version = "=1.1.0"');
      expect(cargo).not.toContain('path =');
      const frontend = JSON.parse(fs.readFileSync(path.join(target, 'package.json'), 'utf8')) as {
        dependencies: Record<string, string>;
      };
      expect(frontend.dependencies['@tauron/host']).toBe('1.1.0');

      // 装配必须是真装配，不是裸 Builder
      const mainRs = fs.readFileSync(path.join(target, 'src-tauri', 'src', 'main.rs'), 'utf8');
      expect(mainRs).toContain('tauron_adapter::tauron_generate_handler![]');
      expect(mainRs).toContain('state_init_with_adapter_config');
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('--tauron-path 显式启用本地源码依赖', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-new-local-'));
    const target = path.join(root, 'local-app');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main([
        'node',
        'cli.js',
        'new',
        target,
        '--tauron-path',
        path.resolve(process.cwd(), '../..'),
      ]);
      expect(process.exitCode).toBe(0);
      const cargo = fs.readFileSync(path.join(target, 'src-tauri', 'Cargo.toml'), 'utf8');
      expect(cargo).toContain('version = "=1.1.0", path =');
      const pkg = JSON.parse(fs.readFileSync(path.join(target, 'package.json'), 'utf8')) as {
        dependencies: Record<string, string>;
      };
      expect(pkg.dependencies['@tauron/host']).toMatch(/^file:/);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('落盘 vite 配置与占位图标（否则连 cargo check 都过不去）', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-assets-'));
    const target = path.join(root, 'assets-app');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'new', target, '--name', 'assets-app']);
      expect(process.exitCode).toBe(0);

      // vite 配置 + 根 index.html：`npm run tauri dev` 的前置
      expect(fs.existsSync(path.join(target, 'vite.config.ts'))).toBe(true);
      expect(fs.existsSync(path.join(target, 'index.html'))).toBe(true);
      const conf = JSON.parse(
        fs.readFileSync(path.join(target, 'src-tauri', 'tauri.conf.json'), 'utf8'),
      ) as { build: { beforeDevCommand?: string } };
      expect(conf.build.beforeDevCommand).toBe('npm run dev');

      // Windows 上 tauri-build 生成资源文件要读它——缺了 cargo check 直接失败
      const ico = fs.readFileSync(path.join(target, 'src-tauri', 'icons', 'icon.ico'));
      expect(ico.length).toBeGreaterThan(0);
      expect(ico[2]).toBe(1); // ICONDIR.type = 1（图标）
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('create-tauron-app <dir> 是同一个实现（无子命令写法）', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-cta-'));
    const target = path.join(root, 'cta-app');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'create-tauron-app', target]);
      expect(process.exitCode).toBe(0);
      expect(fs.existsSync(path.join(target, 'package.json'))).toBe(true);
      expect(fs.existsSync(path.join(target, 'src-tauri', 'src', 'main.rs'))).toBe(true);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('Windows 的 .cmd shim 也支持 create-tauron-app <dir>', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-cta-cmd-'));
    const target = path.join(root, 'cta-cmd-app');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'C:\\Users\\runner\\create-tauron-app.cmd', target]);
      expect(process.exitCode).toBe(0);
      expect(fs.existsSync(path.join(target, 'package.json'))).toBe(true);
      expect(fs.existsSync(path.join(target, 'src-tauri', 'src', 'main.rs'))).toBe(true);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('--dry-run 只报告、不落盘', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-dry-'));
    const target = path.join(root, 'dry-app');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'new', target, '--dry-run']);
      expect(process.exitCode).toBe(0);
      expect(fs.existsSync(target)).toBe(false);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
});

describe('tauron-app init（接入现有项目）', () => {
  const previousExitCode = process.exitCode;
  afterEach(() => {
    process.exitCode = previousExitCode;
    vi.restoreAllMocks();
  });

  /** 造一个最小 Tauri 2 工程（有 src-tauri/Cargo.toml 与 lib.rs）。 */
  function makeTauriProject(root: string): string {
    const project = path.join(root, 'existing-app');
    fs.mkdirSync(path.join(project, 'src-tauri', 'src'), { recursive: true });
    fs.writeFileSync(
      path.join(project, 'src-tauri', 'Cargo.toml'),
      '[package]\nname = "existing-app"\nversion = "0.1.0"\n\n[dependencies]\ntauri = { version = "2", features = ["wry"] }\n',
    );
    fs.writeFileSync(
      path.join(project, 'src-tauri', 'src', 'lib.rs'),
      'pub fn run() {\n  tauri::Builder::default()\n    .run(tauri::generate_context!())\n    .expect("error");\n}\n',
    );
    return project;
  }

  it('--dir 真的作用在目标工程上（而不是当前目录）', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-init-'));
    const project = makeTauriProject(root);
    fs.writeFileSync(path.join(project, 'package.json'), JSON.stringify({ name: 'existing-app' }));
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'init', '--dir', project]);
      expect(process.exitCode).toBe(0);

      const cargo = fs.readFileSync(path.join(project, 'src-tauri', 'Cargo.toml'), 'utf8');
      expect(cargo).toContain('tauron-adapter = { version = "=1.1.0"');
      expect(cargo).not.toContain('path =');

      const frontend = JSON.parse(fs.readFileSync(path.join(project, 'package.json'), 'utf8')) as {
        dependencies: Record<string, string>;
      };
      expect(frontend.dependencies['@tauron/host']).toBe('1.1.0');
      expect(frontend.dependencies['@tauron/ui']).toBe('1.1.0');
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('重复 init 幂等且保留已有 handler、client config 和 capability', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-init-repeat-'));
    const project = makeTauriProject(root);
    const mainRsPath = path.join(project, 'src-tauri', 'src', 'lib.rs');
    const originalMain =
      'pub fn run() {\n  tauri::Builder::default()\n    .invoke_handler(tauri::generate_handler![existing_command])\n    .run(tauri::generate_context!())\n    .expect("error");\n}\n';
    fs.writeFileSync(mainRsPath, originalMain);
    fs.writeFileSync(path.join(project, 'client-config.json'), '{"custom":true}\n');
    const capabilitiesPath = path.join(project, 'src-tauri', 'capabilities', 'default.json');
    fs.mkdirSync(path.dirname(capabilitiesPath), { recursive: true });
    fs.writeFileSync(capabilitiesPath, '{"identifier":"custom","permissions":[]}\n');
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'init', '--dir', project]);
      expect(process.exitCode).toBe(2);
      await main(['node', 'cli.js', 'init', '--dir', project]);
      expect(process.exitCode).toBe(2);

      const resultingMain = fs.readFileSync(mainRsPath, 'utf8');
      expect(resultingMain).toContain(
        '.invoke_handler(tauri::generate_handler![existing_command])',
      );
      expect(resultingMain).toContain('.run(tauri::generate_context!())');
      expect(resultingMain.match(/\.invoke_handler\s*\(/g)).toHaveLength(1);
      expect(fs.readFileSync(path.join(project, 'client-config.json'), 'utf8')).toBe(
        '{"custom":true}\n',
      );
      expect(fs.readFileSync(capabilitiesPath, 'utf8')).toBe(
        '{"identifier":"custom","permissions":[]}\n',
      );
      expect(fs.existsSync(path.join(project, 'src-tauri', 'capabilities', 'tauron.json'))).toBe(
        true,
      );
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('--tauron-path 指向非检出根时如实失败（不写解析不了的坐标）', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-init-bad-'));
    const project = makeTauriProject(root);
    const notACheckout = path.join(root, 'empty');
    fs.mkdirSync(notACheckout);
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'init', '--dir', project, '--tauron-path', notACheckout]);
      expect(process.exitCode).toBe(1);
      // 失败时不得留下半成品依赖坐标
      const cargo = fs.readFileSync(path.join(project, 'src-tauri', 'Cargo.toml'), 'utf8');
      expect(cargo).not.toContain('tauron-adapter');
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('拒绝 Tauri 1 项目，避免生成 v2 capability 后误报接入成功', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-init-tauri1-'));
    const project = makeTauriProject(root);
    const cargoPath = path.join(project, 'src-tauri', 'Cargo.toml');
    fs.writeFileSync(
      cargoPath,
      fs.readFileSync(cargoPath, 'utf8').replace('version = "2"', 'version = "1"'),
    );
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'init', '--dir', project]);
      expect(process.exitCode).toBe(1);
      expect(fs.readFileSync(cargoPath, 'utf8')).not.toContain('tauron-adapter');
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  it('Cargo.toml 没有 dependencies 段时会创建依赖段', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tauron-cli-init-cargo-empty-'));
    const project = makeTauriProject(root);
    fs.writeFileSync(
      path.join(project, 'src-tauri', 'Cargo.toml'),
      '[package]\nname = "existing-app"\nversion = "0.1.0"\n\n[dependencies.tauri]\nversion = "2"\n\n[lib]\nname = "existing_app"\n',
    );
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'init', '--dir', project]);
      expect(process.exitCode).toBe(0);
      expect(fs.readFileSync(path.join(project, 'src-tauri', 'Cargo.toml'), 'utf8')).toContain(
        '[dependencies]\ntauron-adapter = { version = "=1.1.0"',
      );
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
});
