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
    fs.writeFileSync(path.join(pluginDir, 'manifest.json'), JSON.stringify({
      id: 'com.example.plugin', name: 'Example', version: '1.0.0', type: 'js', permissions: [],
    }));
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

      await main(['node', 'cli.js', 'plugin', 'sign', '--file', output, '--key', keyPath, '--kid', 'test-key']);
      expect(process.exitCode).toBe(0);
      const signature = JSON.parse(fs.readFileSync(`${output}.sig`, 'utf8')) as { files: Array<{ path: string; hash: string }> };
      expect(signature.files.map((file) => file.path).sort()).toEqual(['manifest.json', 'src/index.js']);
      expect(signature.files.every((file) => /^[a-f0-9]{64}$/.test(file.hash))).toBe(true);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
});
