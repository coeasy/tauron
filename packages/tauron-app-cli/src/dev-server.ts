// ──────────────────────────────────────────────────────────────────────────
// Plugin Dev 热重载服务器（§4.17 `plugin dev`）。
//
// 职责：监听插件目录变化，通知宿主热重载。
// 使用 Node.js 内置 fs.watch() 实现（零外部依赖）。
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';

/** 热重载事件类型。 */
export type ReloadEventType = 'add' | 'change' | 'unlink' | 'addDir' | 'unlinkDir';

/** 热重载事件。 */
export interface ReloadEvent {
  type: ReloadEventType;
  filePath: string;
  timestamp: number;
}

/** 监听器配置。 */
export interface WatcherConfig {
  /** 监听目录。 */
  dir: string;
  /** 忽略的文件模式（glob 简写）。 */
  ignorePatterns?: string[];
  /** 最大监听文件数。 */
  maxFiles?: number;
  /** 变化回调。 */
  onChange?: (event: ReloadEvent) => void;
}

/** 文件监听器。 */
export class PluginDevWatcher {
  private _watchers: Map<string, fs.FSWatcher> = new Map();
  private _config: WatcherConfig;
  private _isWatching = false;
  private _fileCount = 0;
  private readonly _maxFiles: number;
  private readonly _ignorePatterns: string[];

  constructor(config: WatcherConfig) {
    this._config = config;
    this._maxFiles = config.maxFiles ?? 1000;
    this._ignorePatterns = config.ignorePatterns ?? ['node_modules', '.git', 'dist', '.DS_Store'];
  }

  /** 开始监听。 */
  async start(): Promise<void> {
    if (this._isWatching) return;
    if (!fs.existsSync(this._config.dir)) {
      throw new Error(`目录不存在：${this._config.dir}`);
    }
    this._watchDir(this._config.dir);
    this._isWatching = true;
  }

  /** 停止监听。 */
  stop(): void {
    for (const watcher of this._watchers.values()) {
      watcher.close();
    }
    this._watchers.clear();
    this._isWatching = false;
  }

  /** 是否正在监听。 */
  get isWatching(): boolean {
    return this._isWatching;
  }

  /** 已监听文件数。 */
  get fileCount(): number {
    return this._fileCount;
  }

  private _watchDir(dir: string): void {
    try {
      const watcher = fs.watch(dir, (eventType, filename) => {
        if (!filename) return;
        const filePath = path.join(dir, filename);
        if (this._isIgnored(filePath)) return;
        if (this._fileCount >= this._maxFiles) return;

        const type = this._mapEventType(eventType, filePath);
        this._fileCount++;
        this._config.onChange?.({
          type,
          filePath,
          timestamp: Date.now(),
        });
      });
      this._watchers.set(dir, watcher);
    } catch {
      // 目录不可监听（权限等），静默跳过
    }

    // 递归监听子目录
    try {
      const entries = fs.readdirSync(dir, { withFileTypes: true });
      for (const entry of entries) {
        if (entry.isDirectory() && !this._isIgnored(path.join(dir, entry.name))) {
          this._watchDir(path.join(dir, entry.name));
        }
      }
    } catch {
      // 读取失败，跳过
    }
  }

  private _isIgnored(filePath: string): boolean {
    const name = path.basename(filePath);
    return this._ignorePatterns.some((pattern) => {
      if (pattern.endsWith('/')) return name.startsWith(pattern.slice(0, -1));
      if (pattern.startsWith('!')) return name.startsWith(pattern.slice(1));
      return name.includes(pattern);
    });
  }

  private _mapEventType(eventType: string, filePath: string): ReloadEventType {
    if (eventType === 'rename') {
      if (fs.existsSync(filePath)) return 'add';
      return 'unlink';
    }
    return 'change';
  }
}

/**
 * 创建并启动监听器。
 */
export async function createDevWatcher(config: WatcherConfig): Promise<PluginDevWatcher> {
  const watcher = new PluginDevWatcher(config);
  await watcher.start();
  return watcher;
}
