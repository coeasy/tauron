// ──────────────────────────────────────────────────────────────────────────
// 结构化日志层（§4.32）。
//
// 职责：提供结构化 JSON 日志输出，支持日志级别、上下文、格式化。
// 使用 Node.js 内置 console + stream 实现（零外部依赖）。
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';

/** 日志级别。 */
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

/** 日志条目。 */
export interface LogEntry {
  timestamp: string;
  level: LogLevel;
  message: string;
  context?: Record<string, unknown>;
}

/** 日志配置。 */
export interface LoggerConfig {
  /** 最低日志级别。 */
  level?: LogLevel;
  /** 日志文件路径（可选）。 */
  filePath?: string;
  /** 是否输出到控制台。 */
  console?: boolean;
}

/** 日志级别优先级。 */
const LEVEL_PRIORITY: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
} as const;

/** 结构化日志记录器。 */
export class Logger {
  private _config: LoggerConfig;
  private _stream: fs.WriteStream | null = null;

  constructor(config: LoggerConfig = {}) {
    this._config = {
      level: config.level ?? 'info',
      console: config.console ?? true,
    };

    if (config.filePath) {
      const dir = path.dirname(config.filePath);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }
      this._stream = fs.createWriteStream(config.filePath, { flags: 'a' });
    }
  }

  /** 记录 debug 日志。 */
  debug(message: string, context?: Record<string, unknown>): void {
    this._log('debug', message, context);
  }

  /** 记录 info 日志。 */
  info(message: string, context?: Record<string, unknown>): void {
    this._log('info', message, context);
  }

  /** 记录 warn 日志。 */
  warn(message: string, context?: Record<string, unknown>): void {
    this._log('warn', message, context);
  }

  /** 记录 error 日志。 */
  error(message: string, context?: Record<string, unknown>): void {
    this._log('error', message, context);
  }

  /** 关闭日志流。 */
  close(): void {
    if (this._stream) {
      this._stream.end();
      this._stream = null;
    }
  }

  private _log(level: LogLevel, message: string, context?: Record<string, unknown>): void {
    if (LEVEL_PRIORITY[level] < LEVEL_PRIORITY[this._config.level!]) {
      return;
    }

    const entry: LogEntry = {
      timestamp: new Date().toISOString(),
      level,
      message,
      ...(context ? { context } : {}),
    };

    const json = JSON.stringify(entry);

    if (this._config.console) {
      const prefix = `[${level.toUpperCase()}]`;
      console.log(`${prefix} ${json}`);
    }

    if (this._stream) {
      this._stream.write(json + '\n');
    }
  }
}

/** 创建默认日志记录器。 */
export function createLogger(config?: LoggerConfig): Logger {
  return new Logger(config);
}

/** 日志级别比较。 */
export function isLevelEnabled(config: LoggerConfig, level: LogLevel): boolean {
  return LEVEL_PRIORITY[level] >= LEVEL_PRIORITY[config.level ?? 'info'];
}
