import { describe, it, expect } from 'vitest';
import { validateConfig, type TauronConfig } from './config.js';

describe('config validation', () => {
  const validConfig: TauronConfig = {
    capabilities: ['base', 'updater', 'store'],
    plugins: {
      local: './plugins',
      registry: 'https://git.example.com/tauron-plugins.git',
      autoUpdate: true,
    },
    shell: {
      type: 'local',
      distDir: '../dist',
    },
    updater: {
      endpoints: ['https://github.com/example/my-app/releases/latest.json'],
      pubkey: 'dW50cnVzdGVk...',
    },
    brands: {
      default: './brands/default.json',
    },
    i18n: {
      defaultLocale: 'zh-CN',
      supported: ['zh-CN', 'en'],
    },
    motion: {
      preset: 'standard',
      durations: { fast: 150, normal: 300, slow: 500 },
      easings: { standard: { name: 'standard', cubicBezier: [0.4, 0.0, 0.2, 1.0] as [number, number, number, number], value: 'cubic-bezier(0.4, 0.0, 0.2, 1.0)' } },
      splash: {
        enabled: true,
        minDuration: 1500,
        title: 'Tauron',
        background: '#000',
        progress: 'bar',
        exitAnimation: 'fade',
      },
      exit: {
        animation: 'fade',
        duration: 400,
        savingPrompt: '',
        prompt: '',
      },
      transitions: {
        toast: true,
        dialog: true,
        commandPalette: true,
        pluginList: true,
        themeSwitch: true,
      },
      respectReducedMotion: true,
    },
  };

  it('accepts valid config', () => {
    expect(validateConfig(validConfig)).toEqual([]);
  });

  it('rejects null config', () => {
    expect(validateConfig(null)).toEqual(['Config must be an object']);
  });

  it('rejects missing capabilities', () => {
    const config = { ...validConfig, capabilities: undefined };
    const errors = validateConfig(config);
    expect(errors).toContain('capabilities must be an array');
  });

  it('rejects invalid shell type', () => {
    const config = { ...validConfig, shell: { type: 'invalid', distDir: '../dist' } };
    const errors = validateConfig(config);
    expect(errors.some((e) => e.includes('shell.type'))).toBe(true);
  });

  it('rejects missing plugins.local', () => {
    const config = { ...validConfig, plugins: { local: '', registry: '', autoUpdate: true } };
    const errors = validateConfig(config);
    expect(errors).toContain('plugins.local is required');
  });

  it('rejects empty updater.endpoints', () => {
    const config = { ...validConfig, updater: { endpoints: [], pubkey: 'key' } };
    const errors = validateConfig(config);
    expect(errors.some((e) => e.includes('updater.endpoints'))).toBe(true);
  });

  it('rejects missing i18n.defaultLocale', () => {
    const config = { ...validConfig, i18n: { defaultLocale: '', supported: ['en'] } };
    const errors = validateConfig(config);
    expect(errors).toContain('i18n.defaultLocale is required');
  });

  it('accepts all valid shell types', () => {
    const shells = ['local', 'local-server', 'remote-url', 'sub-webview'];
    for (const type of shells) {
      const config = { ...validConfig, shell: { type: type as any, distDir: '../dist' } };
      expect(validateConfig(config)).toEqual([]);
    }
  });
});
