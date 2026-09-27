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

  it('tauron-app new 落盘一个真接线了 tauron 的工程', async () => {
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

      // 自动探测到的 tauronPath 必须**真的指向检出根**（否则只是换了种写法，
      // 依赖照样解析不了——这正是这次要消灭的缺陷）
      const cargo = fs.readFileSync(path.join(target, 'src-tauri', 'Cargo.toml'), 'utf8');
      const match = /tauron-adapter = \{ path = "([^"]+)"/.exec(cargo);
      expect(match).not.toBeNull();
      const resolved = path.resolve(target, 'src-tauri', match![1]!);
      expect(fs.existsSync(path.join(resolved, 'Cargo.toml'))).toBe(true);

      // 装配必须是真装配，不是裸 Builder
      const mainRs = fs.readFileSync(path.join(target, 'src-tauri', 'src', 'main.rs'), 'utf8');
      expect(mainRs).toContain('tauron_adapter::tauron_generate_handler![]');
      expect(mainRs).toContain('state_init_with_adapter_config');
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
    vi.spyOn(console, 'log').mockImplementation(() => undefined);
    process.exitCode = 0;
    try {
      await main(['node', 'cli.js', 'init', '--dir', project]);
      expect(process.exitCode).toBe(0);

      // 依赖必须写进**目标工程**的 Cargo.toml，且指向真实存在的检出根
      const cargo = fs.readFileSync(path.join(project, 'src-tauri', 'Cargo.toml'), 'utf8');
      const match = /tauron-adapter = \{ path = "([^"]+)"/.exec(cargo);
      expect(match).not.toBeNull();
      const resolved = path.resolve(project, 'src-tauri', match![1]!);
      expect(fs.existsSync(path.join(resolved, 'Cargo.toml'))).toBe(true);
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
});
