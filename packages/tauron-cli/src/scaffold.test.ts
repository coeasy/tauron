import { describe, it, expect } from 'vitest';
import { createApp } from './scaffold.js';
import type { AppConfig } from './types.js';

describe('createApp', () => {
  const baseConfig: AppConfig = {
    name: 'test-app',
    template: 'vanilla',
    pluginTypes: ['js'],
  };

  it('creates app with required files', () => {
    const result = createApp(baseConfig, { verbose: false, dryRun: false, force: false });
    expect(result.files).toBeDefined();
    expect(result.files['package.json']).toBeDefined();
    expect(result.files['tauron.config.ts']).toBeDefined();
    expect(result.files['src/index.ts']).toBeDefined();
    expect(result.files['tsconfig.json']).toBeDefined();
    expect(result.files['.gitignore']).toBeDefined();
    expect(result.files['README.md']).toBeDefined();
  });

  it('sets correct app name in package.json', () => {
    const result = createApp(baseConfig, { verbose: false, dryRun: false, force: false });
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.name).toBe('test-app');
  });

  it('includes @tauron/core dependency', () => {
    const result = createApp(baseConfig, { verbose: false, dryRun: false, force: false });
    const pkg = JSON.parse(result.files['package.json']!);
    expect(pkg.dependencies['@tauron/core']).toBeDefined();
  });

  it('generates tauron.config.ts', () => {
    const result = createApp(baseConfig, { verbose: false, dryRun: false, force: false });
    expect(result.files['tauron.config.ts']).toContain('defineConfig');
  });

  it('creates React entry for react template', () => {
    const config = { ...baseConfig, template: 'react' as const };
    const result = createApp(config, { verbose: false, dryRun: false, force: false });
    expect(result.files['src/index.ts']).toContain('React');
    expect(result.files['src/index.ts']).toContain('TauronProvider');
  });

  it('returns correct dirName', () => {
    const result = createApp(baseConfig, { verbose: false, dryRun: false, force: false });
    expect(result.dirName).toBe('test-app');
  });

  it('generates different entry for different templates', () => {
    const vanilla = createApp({ ...baseConfig, template: 'vanilla' }, { verbose: false, dryRun: false, force: false });
    const react = createApp({ ...baseConfig, template: 'react' }, { verbose: false, dryRun: false, force: false });
    expect(vanilla.files['src/index.ts']).not.toEqual(react.files['src/index.ts']);
  });
});
