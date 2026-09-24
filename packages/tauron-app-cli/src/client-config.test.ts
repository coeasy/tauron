import { describe, expect, it } from 'vitest';
import {
  generateClientConfig,
  validateClientConfigFile,
} from './client-config.js';

describe('generateClientConfig', () => {
  it('template 预设生成完整配置', () => {
    const result = generateClientConfig({ preset: 'template' });
    expect(result.errors).toHaveLength(0);
    const config = JSON.parse(result.content);
    expect(config.registry.max_plugins).toBe(8);
    expect(config.registry.max_active_identities).toBe(8);
    expect(config.registry.max_pending_calls).toBe(1000);
    expect(config.registry.pending_ttl_secs).toBe(30);
    expect(config.log_level).toBe('info');
    expect(config.auto_update).toBe(true);
    expect(result.outputPath).toBe('client-config.json');
    expect(result.written).toBe(false);
  });

  it('full 预设生成开发环境配置', () => {
    const result = generateClientConfig({ preset: 'full' });
    expect(result.errors).toHaveLength(0);
    const config = JSON.parse(result.content);
    expect(config.log_level).toBe('debug');
    expect(config.auto_update).toBe(false);
    expect(config.crash_report_enabled).toBe(true);
    expect(config.performance_monitoring).toBe(true);
  });

  it('minimal 预设生成嵌入式配置', () => {
    const result = generateClientConfig({ preset: 'minimal' });
    expect(result.errors).toHaveLength(0);
    const config = JSON.parse(result.content);
    expect(config.registry.plugin_filter.include_builtins).toBe(true);
    expect(config.registry.plugin_filter.allow).toEqual([]);
    expect(config.log_level).toBe('warn');
    expect(config.auto_update).toBe(false);
  });

  it('自定义 allow/deny/types/platforms', () => {
    const result = generateClientConfig({
      preset: 'template',
      allow: ['com.example.a', 'com.example.b'],
      deny: ['com.example.bad'],
      types: ['js'],
      platforms: ['win', 'mac'],
      includeBuiltins: false,
    });
    expect(result.errors).toHaveLength(0);
    const config = JSON.parse(result.content);
    expect(config.registry.plugin_filter.allow).toEqual(['com.example.a', 'com.example.b']);
    expect(config.registry.plugin_filter.deny).toEqual(['com.example.bad']);
    expect(config.registry.plugin_filter.types).toEqual(['js']);
    expect(config.registry.plugin_filter.platforms).toEqual(['win', 'mac']);
    expect(config.registry.plugin_filter.include_builtins).toBe(false);
  });

  it('allow/deny 冲突报错', () => {
    const result = generateClientConfig({
      preset: 'template',
      allow: ['com.example.a'],
      deny: ['com.example.a'],
    });
    expect(result.errors.length).toBe(1);
    expect(result.errors[0]).toContain('allow');
  });

  it('自定义 logLevel/autoUpdate/crashReport', () => {
    const result = generateClientConfig({
      preset: 'template',
      logLevel: 'debug',
      autoUpdate: false,
      crashReport: true,
      performanceMonitoring: true,
    });
    expect(result.errors).toHaveLength(0);
    const config = JSON.parse(result.content);
    expect(config.log_level).toBe('debug');
    expect(config.auto_update).toBe(false);
    expect(config.crash_report_enabled).toBe(true);
    expect(config.performance_monitoring).toBe(true);
  });

  it('自定义 brandId', () => {
    const result = generateClientConfig({
      preset: 'full',
      brandId: 'my-brand',
    });
    const config = JSON.parse(result.content);
    expect(config.brand_id).toBe('my-brand');
  });

  it('自定义 outputPath', () => {
    const result = generateClientConfig({
      preset: 'template',
      outputPath: 'config/client.json',
    });
    expect(result.outputPath).toBe('config/client.json');
  });

  it('默认预设是 template', () => {
    const result = generateClientConfig({});
    const config = JSON.parse(result.content);
    expect(config.registry.max_plugins).toBe(8);
    expect(config.log_level).toBe('info');
  });

  it('JSON 格式正确（可解析）', () => {
    const result = generateClientConfig({ preset: 'template' });
    // 应该能成功解析
    const parsed = JSON.parse(result.content);
    expect(parsed).toBeDefined();
    expect(parsed.registry).toBeDefined();
  });

  it('生成的配置包含 plugin_filter 字段', () => {
    const result = generateClientConfig({ preset: 'template' });
    const config = JSON.parse(result.content);
    expect(config.registry.plugin_filter).toBeDefined();
    expect(config.registry.plugin_filter.allow).toBeDefined();
    expect(config.registry.plugin_filter.deny).toBeDefined();
    expect(config.registry.plugin_filter.types).toBeDefined();
    expect(config.registry.plugin_filter.platforms).toBeDefined();
    expect(config.registry.plugin_filter.include_builtins).toBeDefined();
  });
});

describe('validateClientConfigFile', () => {
  it('合法配置通过', () => {
    const json = JSON.stringify({
      log_level: 'debug',
      registry: {
        max_plugins: 4,
        plugin_filter: {
          allow: ['com.example.a'],
        },
      },
    });
    const errors = validateClientConfigFile(json);
    expect(errors).toHaveLength(0);
  });

  it('非法 JSON 报错', () => {
    const errors = validateClientConfigFile('{ invalid }');
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('JSON 解析失败');
  });

  it('非法 log_level 报错', () => {
    const errors = validateClientConfigFile(JSON.stringify({ log_level: 'verbose' }));
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('log_level');
  });

  it('max_plugins 为 0 报错', () => {
    const errors = validateClientConfigFile(JSON.stringify({ registry: { max_plugins: 0 } }));
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('max_plugins');
  });

  it('allow/deny 冲突报错', () => {
    const errors = validateClientConfigFile(
      JSON.stringify({
        registry: {
          plugin_filter: {
            allow: ['a'],
            deny: ['a'],
          },
        },
      }),
    );
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('allow');
  });

  it('完整配置通过', () => {
    const errors = validateClientConfigFile(
      JSON.stringify({
        registry: {
          plugin_filter: {
            allow: [],
            deny: [],
            types: ['js'],
            platforms: ['win'],
            include_builtins: true,
          },
          max_plugins: 8,
          max_active_identities: 8,
          max_pending_calls: 1000,
          pending_ttl_secs: 30,
        },
        log_level: 'info',
        auto_update: true,
        update_check_interval_secs: 86400,
        crash_report_enabled: false,
        performance_monitoring: false,
      }),
    );
    expect(errors).toHaveLength(0);
  });
});
