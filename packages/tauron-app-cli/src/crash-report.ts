// ──────────────────────────────────────────────────────────────────────────
// 崩溃上报摘要文件（§4.33）。
//
// 职责：记录崩溃事件、生成崩溃摘要文件、支持崩溃分析。
// 使用 Node.js 内置 fs 实现（零外部依赖）。
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';

/** 崩溃记录。 */
export interface CrashRecord {
  id: string;
  timestamp: string;
  error: string;
  stack?: string;
  pluginId?: string;
  command?: string;
  context?: Record<string, unknown>;
}

/** 崩溃摘要。 */
export interface CrashSummary {
  total: number;
  unique: number;
  byPlugin: Record<string, number>;
  byCommand: Record<string, number>;
  recent: CrashRecord[];
  generatedAt: string;
}

/** 崩溃报告配置。 */
export interface CrashReportConfig {
  /** 报告文件目录。 */
  reportDir?: string;
  /** 最大记录数。 */
  maxRecords?: number;
}

/** 崩溃报告生成器。 */
export class CrashReporter {
  private _reportDir: string;
  private _maxRecords: number;
  private _records: CrashRecord[] = [];

  constructor(config: CrashReportConfig = {}) {
    this._reportDir = config.reportDir ?? path.join(os.homedir(), '.tauron', 'crashes');
    this._maxRecords = config.maxRecords ?? 100;

    if (!fs.existsSync(this._reportDir)) {
      fs.mkdirSync(this._reportDir, { recursive: true });
    }

    this._loadExistingRecords();
  }

  /** 记录崩溃。 */
  recordCrash(crash: Omit<CrashRecord, 'id' | 'timestamp'>): CrashRecord {
    const record: CrashRecord = {
      id: this._generateId(),
      timestamp: new Date().toISOString(),
      ...crash,
    };

    this._records.push(record);

    // 保持最大记录数
    if (this._records.length > this._maxRecords) {
      this._records = this._records.slice(-this._maxRecords);
    }

    // 写入文件
    this._writeRecord(record);

    return record;
  }

  /** 生成崩溃摘要。 */
  generateSummary(): CrashSummary {
    const byPlugin: Record<string, number> = {};
    const byCommand: Record<string, number> = {};

    for (const record of this._records) {
      if (record.pluginId) {
        byPlugin[record.pluginId] = (byPlugin[record.pluginId] ?? 0) + 1;
      }
      if (record.command) {
        byCommand[record.command] = (byCommand[record.command] ?? 0) + 1;
      }
    }

    return {
      total: this._records.length,
      unique: new Set(this._records.map((r) => r.error)).size,
      byPlugin,
      byCommand,
      recent: this._records.slice(-10),
      generatedAt: new Date().toISOString(),
    };
  }

  /** 获取所有崩溃记录。 */
  getRecords(): CrashRecord[] {
    return [...this._records];
  }

  /** 清除所有崩溃记录。 */
  clearRecords(): void {
    this._records = [];
    this._deleteAllRecords();
  }

  /** 写入崩溃摘要到文件。 */
  writeSummaryFile(filePath?: string): string {
    const outputPath = filePath ?? path.join(this._reportDir, 'crash-summary.json');
    const summary = this.generateSummary();
    fs.writeFileSync(outputPath, JSON.stringify(summary, null, 2));
    return outputPath;
  }

  private _generateId(): string {
    return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  }

  private _writeRecord(record: CrashRecord): void {
    const fileName = `crash-${record.id}.json`;
    const filePath = path.join(this._reportDir, fileName);
    fs.writeFileSync(filePath, JSON.stringify(record, null, 2));
  }

  private _loadExistingRecords(): void {
    try {
      const files = fs.readdirSync(this._reportDir)
        .filter((f) => f.startsWith('crash-') && f.endsWith('.json'));

      for (const file of files.slice(-this._maxRecords)) {
        const content = fs.readFileSync(path.join(this._reportDir, file), 'utf-8');
        this._records.push(JSON.parse(content));
      }
    } catch {
      // 目录不存在或读取失败，忽略
    }
  }

  private _deleteAllRecords(): void {
    try {
      const files = fs.readdirSync(this._reportDir)
        .filter((f) => f.startsWith('crash-') && f.endsWith('.json'));
      for (const file of files) {
        fs.unlinkSync(path.join(this._reportDir, file));
      }
    } catch {
      // 忽略删除失败
    }
  }
}

/** 创建崩溃报告器。 */
export function createCrashReporter(config?: CrashReportConfig): CrashReporter {
  return new CrashReporter(config);
}
