// ──────────────────────────────────────────────────────────────────────────
// 客户端配置类型（与 Rust `ClientConfig` 镜像，§4.24）。
//
// 设计原则：
// - 所有字段可选（`Partial`），缺省由 Rust 侧 `Default` 提供；
// - `PluginFilter` 与 Rust `PluginFilter` 字段一致；
// - 提供 `validate()` 纯函数（无 IO），与 Rust `ClientConfig::validate()` 同逻辑；
// - 提供三个预置配置模板（`fullLoad` / `minimal` / `template`），与 Rust 侧一致。
// ──────────────────────────────────────────────────────────────────────────

/** 插件加载过滤器（与 Rust `PluginFilter` 镜像）。 */
export interface PluginFilter {
  /** 允许加载的插件 ID 列表（空 = 允许所有）。 */
  allow?: string[];
  /** 禁止加载的插件 ID 列表（从允许列表中排除）。 */
  deny?: string[];
  /** 仅加载指定类型的插件（空 = 所有类型）。 */
  types?: string[];
  /** 仅加载指定平台的插件（空 = 所有平台）。 */
  platforms?: string[];
  /** 是否加载内置插件（默认 `true`）。 */
  include_builtins?: boolean;
}

/** 注册表配置覆盖（与 Rust `RegistryConfigOverride` 镜像）。 */
export interface RegistryConfigOverride {
  plugin_filter?: PluginFilter;
  max_plugins?: number;
  max_active_identities?: number;
  max_pending_calls?: number;
  pending_ttl_secs?: number;
}

/** 客户端配置（与 Rust `ClientConfig` 镜像，§4.24）。 */
export interface ClientConfig {
  /** 注册表配置（含插件过滤器、容量上限等）。 */
  registry?: RegistryConfigOverride;
  /** 日志级别。 */
  log_level?: 'error' | 'warn' | 'info' | 'debug' | 'trace';
  /** 数据目录路径。 */
  data_dir?: string;
  /** 是否启用插件自动更新检查。 */
  auto_update?: boolean;
  /** 更新检查间隔（秒）。 */
  update_check_interval_secs?: number;
  /** 是否启用崩溃报告。 */
  crash_report_enabled?: boolean;
  /** 自定义品牌标识。 */
  brand_id?: string;
  /** 额外的环境变量注入。 */
  env_overrides?: Record<string, string>;
  /** 插件发现路径列表。 */
  plugin_paths?: string[];
  /** 是否启用性能监控。 */
  performance_monitoring?: boolean;
}

const VALID_LOG_LEVELS = ['error', 'warn', 'info', 'debug', 'trace'] as const;
export type LogLevel = (typeof VALID_LOG_LEVELS)[number];

/** 校验配置合法性（纯函数，无 IO）。 */
export function validateClientConfig(config: ClientConfig): string[] {
  const errors: string[] = [];

  if (config.log_level && !VALID_LOG_LEVELS.includes(config.log_level)) {
    errors.push(`log_level "${config.log_level}" 非法，必须是 ${VALID_LOG_LEVELS.join('/')}`);
  }

  if (config.update_check_interval_secs !== undefined && config.update_check_interval_secs <= 0) {
    errors.push('update_check_interval_secs 必须大于 0');
  }

  if (config.registry) {
    const r = config.registry;
    // TS 数值是 double，负数必须与 0 一起拒绝（Rust 侧为无符号类型，
    // 负数在反序列化即失败，故只查 0——两侧语义对齐：数量必须为正）。
    if (r.max_plugins !== undefined && r.max_plugins <= 0) {
      errors.push('registry.max_plugins 必须大于 0');
    }
    if (r.max_active_identities !== undefined && r.max_active_identities <= 0) {
      errors.push('registry.max_active_identities 必须大于 0');
    }
    if (r.max_pending_calls !== undefined && r.max_pending_calls <= 0) {
      errors.push('registry.max_pending_calls 必须大于 0');
    }
    if (r.pending_ttl_secs !== undefined && r.pending_ttl_secs <= 0) {
      errors.push('registry.pending_ttl_secs 必须大于 0');
    }
    // allow/deny 冲突检查
    if (r.plugin_filter?.allow && r.plugin_filter?.deny) {
      const conflict = r.plugin_filter.allow.find((a) => r.plugin_filter!.deny!.includes(a));
      if (conflict) {
        errors.push(`plugin_filter: "${conflict}" 同时出现在 allow 与 deny 中`);
      }
    }
  }

  return errors;
}

/** 预置配置：全量加载（开发环境）。 */
export function fullLoadConfig(): ClientConfig {
  return {
    log_level: 'debug',
    auto_update: false,
    crash_report_enabled: true,
    performance_monitoring: true,
  };
}

/** 预置配置：最小化（仅内置插件，嵌入式场景）。 */
export function minimalConfig(): ClientConfig {
  return {
    registry: {
      plugin_filter: {
        include_builtins: true,
        allow: [],
      },
    },
    log_level: 'warn',
    auto_update: false,
    crash_report_enabled: false,
  };
}

/** 预置配置：模板（含所有字段，供用户编辑）。 */
export function templateConfig(): ClientConfig {
  return {
    registry: {
      plugin_filter: {
        allow: [],
        deny: [],
        types: [],
        platforms: [],
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
  };
}
