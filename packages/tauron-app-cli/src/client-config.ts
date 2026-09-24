// ──────────────────────────────────────────────────────────────────────────
// @tauron/app-cli — 客户端配置生成与校验（§4.17 `client config`）。
//
// 提供：
// - `generateClientConfig` — 从选项生成配置文件
// - `validateClientConfigFile` — 校验配置文件合法性
// - `presets` — 三个预置配置模板
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';

import type { ClientConfig } from '@tauron/host';

/** 配置生成选项。 */
export interface ClientConfigOptions {
  /** 预设类型：`full`（全量）、`minimal`（最小化）、`template`（模板）。 */
  preset?: 'full' | 'minimal' | 'template';
  /** 自定义允许加载的插件 ID 列表。 */
  allow?: string[];
  /** 自定义禁止加载的插件 ID 列表。 */
  deny?: string[];
  /** 仅加载指定类型的插件。 */
  types?: string[];
  /** 仅加载指定平台的插件。 */
  platforms?: string[];
  /** 是否加载内置插件。 */
  includeBuiltins?: boolean;
  /** 日志级别。 */
  logLevel?: 'error' | 'warn' | 'info' | 'debug' | 'trace';
  /** 是否启用自动更新。 */
  autoUpdate?: boolean;
  /** 是否启用崩溃报告。 */
  crashReport?: boolean;
  /** 是否启用性能监控。 */
  performanceMonitoring?: boolean;
  /** 品牌标识。 */
  brandId?: string;
  /** 输出路径（相对于工作目录）。 */
  outputPath?: string;
}

/** 配置生成结果。 */
export interface ClientConfigResult {
  /** 生成的 JSON 配置内容。 */
  content: string;
  /** 输出文件路径。 */
  outputPath: string;
  /** 是否已写入文件。 */
  written: boolean;
  /** 校验错误列表（空 = 通过）。 */
  errors: string[];
}

/**
 * 从选项生成客户端配置。
 *
 * 校验配置合法性后返回 JSON 字符串；不写文件（调用方自行写入）。
 */
export function generateClientConfig(options: ClientConfigOptions): ClientConfigResult {
  const config = buildConfig(options);
  const errors = validateConfig(config);
  const content = JSON.stringify(config, null, 2);
  const outputPath = options.outputPath ?? 'client-config.json';

  return { content, outputPath, written: false, errors };
}

/** 构建配置对象（根据选项）。 */
function buildConfig(options: ClientConfigOptions): ClientConfig {
  const preset = options.preset ?? 'template';

  if (preset === 'full') {
    return {
      log_level: 'debug',
      auto_update: false,
      crash_report_enabled: true,
      performance_monitoring: true,
      ...(options.brandId ? { brand_id: options.brandId } : {}),
    };
  }

  if (preset === 'minimal') {
    return {
      registry: {
        plugin_filter: {
          include_builtins: options.includeBuiltins ?? true,
          allow: options.allow ?? [],
        },
      },
      log_level: 'warn',
      auto_update: false,
      crash_report_enabled: false,
      ...(options.brandId ? { brand_id: options.brandId } : {}),
    };
  }

  // template preset
  return {
    registry: {
      plugin_filter: {
        allow: options.allow ?? [],
        deny: options.deny ?? [],
        types: options.types ?? [],
        platforms: options.platforms ?? [],
        include_builtins: options.includeBuiltins ?? true,
      },
      max_plugins: 8,
      max_active_identities: 8,
      max_pending_calls: 1000,
      pending_ttl_secs: 30,
    },
    log_level: options.logLevel ?? 'info',
    auto_update: options.autoUpdate ?? true,
    update_check_interval_secs: 86400,
    crash_report_enabled: options.crashReport ?? false,
    ...(options.brandId ? { brand_id: options.brandId } : {}),
    performance_monitoring: options.performanceMonitoring ?? false,
  };
}

/** 校验配置合法性（复用 core 的校验逻辑）。 */
function validateConfig(config: ClientConfig): string[] {
  const errors: string[] = [];

  if (config.log_level && !['error', 'warn', 'info', 'debug', 'trace'].includes(config.log_level)) {
    errors.push(`log_level "${config.log_level}" 非法`);
  }

  if (config.update_check_interval_secs !== undefined && config.update_check_interval_secs === 0) {
    errors.push('update_check_interval_secs 必须大于 0');
  }

  if (config.registry) {
    const r = config.registry;
    if (r.max_plugins !== undefined && r.max_plugins === 0) {
      errors.push('registry.max_plugins 必须大于 0');
    }
    if (r.max_active_identities !== undefined && r.max_active_identities === 0) {
      errors.push('registry.max_active_identities 必须大于 0');
    }
    if (r.max_pending_calls !== undefined && r.max_pending_calls === 0) {
      errors.push('registry.max_pending_calls 必须大于 0');
    }
    if (r.pending_ttl_secs !== undefined && r.pending_ttl_secs === 0) {
      errors.push('registry.pending_ttl_secs 必须大于 0');
    }
    if (r.plugin_filter?.allow && r.plugin_filter?.deny) {
      const conflict = r.plugin_filter.allow.find((a) => r.plugin_filter!.deny!.includes(a));
      if (conflict) {
        errors.push(`plugin_filter: "${conflict}" 同时出现在 allow 与 deny 中`);
      }
    }
  }

  return errors;
}

/**
 * 校验配置文件（从 JSON 字符串）。
 *
 * 返回错误列表；空列表表示通过。
 */
export function validateClientConfigFile(json: string): string[] {
  try {
    const config = JSON.parse(json) as ClientConfig;
    return validateConfig(config);
  } catch (e) {
    return [`JSON 解析失败：${e instanceof Error ? e.message : String(e)}`];
  }
}

/** 校验配置文件（从文件路径）。 */
export function validateClientConfigFilePath(filePath: string): string[] {
  try {
    const content = fs.readFileSync(filePath, 'utf-8');
    return validateClientConfigFile(content);
  } catch (e) {
    return [`读取文件失败：${e instanceof Error ? e.message : String(e)}`];
  }
}
