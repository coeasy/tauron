import { describe, expect, it } from 'vitest';
import {
  validateClientConfig,
  fullLoadConfig,
  minimalConfig,
  templateConfig,
  type ClientConfig,
} from './client-config.js';

describe('validateClientConfig', () => {
  it('空配置通过', () => {
    expect(validateClientConfig({})).toHaveLength(0);
  });

  it('全量加载配置通过', () => {
    expect(validateClientConfig(fullLoadConfig())).toHaveLength(0);
  });

  it('最小化配置通过', () => {
    expect(validateClientConfig(minimalConfig())).toHaveLength(0);
  });

  it('模板配置通过', () => {
    expect(validateClientConfig(templateConfig())).toHaveLength(0);
  });

  it('非法 log_level 报错', () => {
    const errors = validateClientConfig({ log_level: 'verbose' as any });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('log_level');
  });

  it('update_check_interval_secs 为 0 报错', () => {
    const errors = validateClientConfig({ update_check_interval_secs: 0 });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('update_check_interval_secs');
  });

  it('registry.max_plugins 为 0 报错', () => {
    const errors = validateClientConfig({ registry: { max_plugins: 0 } });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('max_plugins');
  });

  it('registry.max_active_identities 为 0 报错', () => {
    const errors = validateClientConfig({ registry: { max_active_identities: 0 } });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('max_active_identities');
  });

  it('registry.max_pending_calls 为 0 报错', () => {
    const errors = validateClientConfig({ registry: { max_pending_calls: 0 } });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('max_pending_calls');
  });

  it('registry.pending_ttl_secs 为 0 报错', () => {
    const errors = validateClientConfig({ registry: { pending_ttl_secs: 0 } });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('pending_ttl_secs');
  });

  it('负数数量同样报错（TS 数值是 double，负数必须拒绝；Rust 侧无符号类型在反序列化即拒）', () => {
    expect(validateClientConfig({ update_check_interval_secs: -5 }).length).toBe(1);
    const r = validateClientConfig({
      registry: {
        max_plugins: -1,
        max_active_identities: -2,
        max_pending_calls: -3,
        pending_ttl_secs: -4,
      },
    });
    expect(r.length).toBe(4);
    for (const field of ['max_plugins', 'max_active_identities', 'max_pending_calls', 'pending_ttl_secs']) {
      expect(r.some((e) => e.includes(field)), `${field} 必须报错`).toBe(true);
    }
  });

  it('plugin_filter allow/deny 冲突报错', () => {
    const errors = validateClientConfig({
      registry: {
        plugin_filter: {
          allow: ['com.example.a'],
          deny: ['com.example.a'],
        },
      },
    });
    expect(errors.length).toBe(1);
    expect(errors[0]).toContain('allow');
    expect(errors[0]).toContain('deny');
  });

  it('plugin_filter allow/deny 无冲突通过', () => {
    const errors = validateClientConfig({
      registry: {
        plugin_filter: {
          allow: ['com.example.a'],
          deny: ['com.example.b'],
        },
      },
    });
    expect(errors).toHaveLength(0);
  });

  it('多个错误同时报告', () => {
    const errors = validateClientConfig({
      log_level: 'verbose' as any,
      update_check_interval_secs: 0,
      registry: {
        max_plugins: 0,
        plugin_filter: {
          allow: ['a'],
          deny: ['a'],
        },
      },
    });
    expect(errors.length).toBe(4);
  });

  it('合法配置通过', () => {
    const config: ClientConfig = {
      log_level: 'debug',
      update_check_interval_secs: 3600,
      registry: {
        max_plugins: 4,
        max_active_identities: 4,
        max_pending_calls: 500,
        pending_ttl_secs: 60,
        plugin_filter: {
          allow: ['com.example.a'],
          types: ['js'],
          platforms: ['win'],
          include_builtins: true,
        },
      },
      env_overrides: { OC_DEBUG: '1' },
      plugin_paths: ['plugins', 'extensions'],
    };
    expect(validateClientConfig(config)).toHaveLength(0);
  });
});

describe('fullLoadConfig', () => {
  it('返回开发环境配置', () => {
    const config = fullLoadConfig();
    expect(config.log_level).toBe('debug');
    expect(config.auto_update).toBe(false);
    expect(config.crash_report_enabled).toBe(true);
    expect(config.performance_monitoring).toBe(true);
  });
});

describe('minimalConfig', () => {
  it('返回嵌入式最小配置', () => {
    const config = minimalConfig();
    expect(config.registry?.plugin_filter?.include_builtins).toBe(true);
    expect(config.registry?.plugin_filter?.allow).toEqual([]);
    expect(config.log_level).toBe('warn');
    expect(config.auto_update).toBe(false);
  });
});

describe('templateConfig', () => {
  it('返回完整模板配置', () => {
    const config = templateConfig();
    expect(config.registry?.max_plugins).toBe(8);
    expect(config.registry?.max_active_identities).toBe(8);
    expect(config.registry?.max_pending_calls).toBe(1000);
    expect(config.registry?.pending_ttl_secs).toBe(30);
    expect(config.log_level).toBe('info');
    expect(config.auto_update).toBe(true);
    expect(config.update_check_interval_secs).toBe(86400);
  });
});
