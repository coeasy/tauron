// ──────────────────────────────────────────────────────────────────────────
// Toast 状态管理（开发计划 §4.10 `<oc-toast>`）。
//
// 职责：统一通知出口分发应用内/系统通知 + 历史持久化（未读/分组/清空）。
//
// 关键约束：
// - 历史条目上限 + 环形裁剪
// - 系统通知未授权/不支持时静默降级
// - 插件通知按 `plugin:<id>` 归组并随卸载清理
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 通知级别。 */
export type ToastLevel = 'info' | 'success' | 'warning' | 'error';

/** 通知操作。 */
export type ToastAction = {
  /** 操作 ID。 */
  id: string;
  /** 操作标签。 */
  label: string;
};

/** 单条通知。 */
export interface ToastItem {
  /** 通知 ID。 */
  id: string;
  /** 通知标题。 */
  title: string;
  /** 通知正文。 */
  message: string;
  /** 通知级别。 */
  level: ToastLevel;
  /** 通知操作。 */
  actions: ToastAction[];
  /** 来源插件 ID（可选）。 */
  pluginId?: string;
  /** 是否未读。 */
  unread: boolean;
  /** 创建时间（毫秒时间戳）。 */
  createdAt: number;
  /** 是否已显示。 */
  dismissed: boolean;
  /** 是否自动关闭。 */
  autoDismiss: boolean;
  /** 自动关闭超时（毫秒，0 = 不自动关闭）。 */
  timeout: number;
}

/** Toast 配置。 */
export interface ToastConfig {
  /** 最大历史条目数。 */
  maxHistory: number;
  /** 默认自动关闭超时（毫秒，0 = 不自动关闭）。 */
  defaultTimeout: number;
  /** 是否启用系统通知。 */
  systemNotifications: boolean;
  /** 系统通知是否已授权。 */
  systemAuthorized: boolean;
}

/** Toast 快照。 */
export interface ToastSnapshot {
  items: ToastItem[];
  config: ToastConfig;
  unreadCount: number;
  groupCounts: Record<string, number>;
}

/** Toast 操作结果。 */
export type ToastActionResult =
  | { ok: true }
  | { ok: false; code: string; message: string };

// ──────────────────────────────────────────────────────────────────────────
// ToastStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * Toast 状态存储。
 *
 * 管理通知列表的状态机和操作。
 */
export class ToastStore {
  private _items: ToastItem[] = [];
  private _config: ToastConfig;
  private _nextId = 0;
  private readonly _subscribers = new Set<(snapshot: ToastSnapshot) => void>();

  constructor(config: Partial<ToastConfig> = {}) {
    this._config = {
      maxHistory: config.maxHistory ?? 100,
      defaultTimeout: config.defaultTimeout ?? 5000,
      systemNotifications: config.systemNotifications ?? true,
      systemAuthorized: config.systemAuthorized ?? false,
    };
  }

  /** 当前快照。 */
  get snapshot(): ToastSnapshot {
    return {
      items: [...this._items],
      config: { ...this._config },
      unreadCount: this._items.filter((i) => !i.unread && !i.dismissed).length,
      groupCounts: this._getGroupCounts(),
    };
  }

  /** 当前通知列表。 */
  get items(): ToastItem[] {
    return [...this._items];
  }

  /** 当前配置。 */
  get config(): ToastConfig {
    return { ...this._config };
  }

  /** 未读数量。 */
  get unreadCount(): number {
    return this._items.filter((i) => !i.unread && !i.dismissed).length;
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: ToastSnapshot) => void): () => void {
    this._subscribers.add(fn);
    try {
      fn(this.snapshot);
    } catch {
      // 订阅者异常不影响其他订阅者
    }
    return () => {
      this._subscribers.delete(fn);
    };
  }

  /** 通知所有订阅者。 */
  private _notify(): void {
    const snapshot = this.snapshot;
    for (const fn of this._subscribers) {
      try {
        fn(snapshot);
      } catch {
        // 订阅者异常不影响其他订阅者
      }
    }
  }

  /** 设置配置。 */
  setConfig(config: Partial<ToastConfig>): void {
    this._config = { ...this._config, ...config };
    this._notify();
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 添加通知。
   *
   * 如果超出最大历史，环形裁剪最旧条目。
   */
  push(
    item: Partial<ToastItem> & { title: string; message: string; level: ToastLevel },
  ): ToastActionResult {
    const toastItem: ToastItem = {
      id: `toast-${this._nextId++}`,
      title: item.title,
      message: item.message,
      level: item.level,
      actions: item.actions ?? [],
      unread: false,
      createdAt: Date.now(),
      dismissed: false,
      autoDismiss: item.autoDismiss ?? (item.level !== 'error'),
      timeout: item.timeout ?? this._config.defaultTimeout,
      ...(item.pluginId !== undefined ? { pluginId: item.pluginId } : {}),
    };

    // 环形裁剪
    if (this._items.length >= this._config.maxHistory) {
      this._items.shift();
    }

    this._items.push(toastItem);
    this._notify();
    return { ok: true };
  }

  /**
   * 添加成功通知。
   */
  success(title: string, message: string, opts?: Partial<ToastItem>): ToastActionResult {
    return this.push({ title, message, level: 'success', ...opts });
  }

  /**
   * 添加错误通知。
   */
  error(title: string, message: string, opts?: Partial<ToastItem>): ToastActionResult {
    return this.push({ title, message, level: 'error', autoDismiss: false, ...opts });
  }

  /**
   * 添加警告通知。
   */
  warning(title: string, message: string, opts?: Partial<ToastItem>): ToastActionResult {
    return this.push({ title, message, level: 'warning', ...opts });
  }

  /**
   * 添加信息通知。
   */
  info(title: string, message: string, opts?: Partial<ToastItem>): ToastActionResult {
    return this.push({ title, message, level: 'info', ...opts });
  }

  /**
   * 标记为已读。
   */
  markRead(id: string): ToastActionResult {
    const item = this._items.find((i) => i.id === id);
    if (!item) {
      return { ok: false, code: 'E_NOT_FOUND', message: '通知不存在' };
    }
    if (item.unread) {
      return { ok: false, code: 'E_ALREADY_READ', message: '通知已读' };
    }
    item.unread = true;
    this._notify();
    return { ok: true };
  }

  /**
   * 标记全部为已读。
   */
  markAllRead(): ToastActionResult {
    let changed = false;
    for (const item of this._items) {
      if (!item.unread && !item.dismissed) {
        item.unread = true;
        changed = true;
      }
    }
    if (changed) {
      this._notify();
    }
    return { ok: true };
  }

  /**
   * 关闭通知。
   */
  dismiss(id: string): ToastActionResult {
    const item = this._items.find((i) => i.id === id);
    if (!item) {
      return { ok: false, code: 'E_NOT_FOUND', message: '通知不存在' };
    }
    if (item.dismissed) {
      return { ok: false, code: 'E_ALREADY_DISMISSED', message: '通知已关闭' };
    }
    item.dismissed = true;
    this._notify();
    return { ok: true };
  }

  /**
   * 清空所有通知。
   */
  clear(): ToastActionResult {
    this._items = [];
    this._notify();
    return { ok: true };
  }

  /**
   * 卸载插件时清理该插件的所有通知。
   */
  cleanupPlugin(pluginId: string): ToastActionResult {
    const before = this._items.length;
    this._items = this._items.filter((i) => i.pluginId !== pluginId);
    const removed = before - this._items.length;
    if (removed > 0) {
      this._notify();
    }
    return { ok: true };
  }

  // ──────────────────────────────────────────────────────────────────────
  // Internal
  // ──────────────────────────────────────────────────────────────────────

  /** 获取分组计数。 */
  private _getGroupCounts(): Record<string, number> {
    const counts: Record<string, number> = {};
    for (const item of this._items) {
      if (!item.dismissed) {
        const key = item.pluginId ?? 'system';
        counts[key] = (counts[key] ?? 0) + 1;
      }
    }
    return counts;
  }
}
