import { describe, it, expect } from 'vitest';
import { pluginNew, generatePluginManifest } from './plugin.js';
import type { PluginConfig } from './types.js';

describe('pluginNew', () => {
  const baseConfig: PluginConfig = {
    name: 'test-plugin',
    type: 'js',
    permissions: ['store:read', 'http:fetch'],
  };

  it('creates plugin with required files', () => {
    const result = pluginNew(baseConfig, { verbose: false, dryRun: false, force: false });
    expect(result.files).toBeDefined();
    expect(result.files['package.json']).toBeDefined();
    expect(result.files['src/index.js']).toBeDefined();
    expect(result.files['README.md']).toBeDefined();
  });

  it('sets correct plugin name in manifest', () => {
    const result = pluginNew(baseConfig, { verbose: false, dryRun: false, force: false });
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.name).toBe('test-plugin');
  });

  it('generates JS plugin entry for js type', () => {
    const result = pluginNew(baseConfig, { verbose: false, dryRun: false, force: false });
    expect(result.files['src/index.js']).toContain('registerPlugin');
    expect(result.files['src/index.js']).toContain("name: 'test-plugin'");
  });

  it('generates Cargo.toml for wasm type', () => {
    const config = { ...baseConfig, type: 'wasm' as const };
    const result = pluginNew(config, { verbose: false, dryRun: false, force: false });
    expect(result.files['Cargo.toml']).toBeDefined();
    expect(result.files['src/lib.rs']).toBeDefined();
  });

  it('generates main.js for process type', () => {
    const config = { ...baseConfig, type: 'process' as const };
    const result = pluginNew(config, { verbose: false, dryRun: false, force: false });
    expect(result.files['main.js']).toBeDefined();
    expect(result.files['main.js']).toContain('createServer');
  });

  it('includes permissions in manifest', () => {
    const result = pluginNew(baseConfig, { verbose: false, dryRun: false, force: false });
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.permissions).toContain('store:read');
    expect(pkg.permissions).toContain('http:fetch');
  });

  it('returns correct dirName', () => {
    const result = pluginNew(baseConfig, { verbose: false, dryRun: false, force: false });
    expect(result.dirName).toBe('test-plugin');
  });

  it('generates methods from permissions', () => {
    const result = pluginNew(baseConfig, { verbose: false, dryRun: false, force: false });
    const entry = result.files['src/index.js']!;
    expect(entry).toContain('store_read');
    expect(entry).toContain('http_fetch');
  });
});

describe('generatePluginManifest', () => {
  const baseConfig: PluginConfig = {
    name: 'test-plugin',
    type: 'js',
    permissions: ['store:read', 'http:fetch'],
  };

  it('generates valid manifest', () => {
    const manifest = generatePluginManifest(baseConfig);
    expect(manifest.name).toBe('test-plugin');
    expect(manifest.version).toBe('0.1.0');
    expect(manifest.type).toBe('js');
    expect(manifest.main).toBe('src/index.js');
    expect(manifest.permissions).toContain('store:read');
  });

  it('uses custom description', () => {
    const config = { ...baseConfig, description: 'Custom description' };
    const manifest = generatePluginManifest(config);
    expect(manifest.description).toBe('Custom description');
  });

  it('uses default description when not provided', () => {
    const config: PluginConfig = { name: 'test-plugin', type: 'js', permissions: ['store:read'] };
    const manifest = generatePluginManifest(config);
    expect(manifest.description).toContain('test-plugin');
  });
});
