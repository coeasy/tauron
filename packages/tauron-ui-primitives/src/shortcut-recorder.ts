// ──────────────────────────────────────────────────────────────────────────
// ShortcutRecorder 状态管理（开发计划 §4.10）。
//
// 职责：快捷键录入与冲突检测。
//
// 关键约束：
// - 键盘录入：按下按键组合记录
// - 冲突检测：与已分配快捷键冲突
// - 验证：最少 2 键组合
// - 格式：Ctrl+Shift+Key
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 修饰键。 */
export type ModifierKey = 'Ctrl' | 'Shift' | 'Alt' | 'Meta';

/** 按键信息。 */
export interface KeyInfo {
  /** 键名（如 'A', 'Enter', 'Escape'）。 */
  key: string;
  /** 修饰键列表。 */
  modifiers: ModifierKey[];
}

/** 快捷键格式。 */
export type ShortcutFormat = 'string' | 'object';

/** 快捷键录入状态。 */
export type ShortcutRecorderState =
  | { status: 'idle'; current: string | null }
  | { status: 'recording'; current: string | null }
  | { status: 'error'; current: string | null; error: string };

/** 快捷键录入配置。 */
export interface ShortcutRecorderConfig {
  /** 当前快捷键。 */
  current: string | null;
  /** 已分配的快捷键列表（用于冲突检测）。 */
  assigned: string[];
  /** 最少按键数。 */
  minKeys: number;
  /** 最多按键数。 */
  maxKeys: number;
  /** 是否允许单键。 */
  allowSingleKey: boolean;
  /** 输出格式。 */
  outputFormat: ShortcutFormat;
  /** 修饰键顺序。 */
  modifierOrder: ModifierKey[];
}

/** 快捷键录入快照。 */
export interface ShortcutRecorderSnapshot {
  state: ShortcutRecorderState;
  config: ShortcutRecorderConfig;
}

/** 快捷键录入操作结果。 */
export type ShortcutRecorderActionResult =
  | { ok: true; shortcut?: string }
  | { ok: false; code: string; message: string };

// ──────────────────────────────────────────────────────────────────────────
// ShortcutRecorderStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * ShortcutRecorder 状态存储。
 *
 * 管理快捷键录入的状态机和操作。
 */
export class ShortcutRecorderStore {
  private _state: ShortcutRecorderState = { status: 'idle', current: null };
  private _config: ShortcutRecorderConfig;
  private _pendingModifiers: ModifierKey[] = [];
  private _pendingKey: string | null = null;
  private readonly _subscribers = new Set<(snapshot: ShortcutRecorderSnapshot) => void>();

  constructor(config: Partial<ShortcutRecorderConfig> = {}) {
    this._config = {
      current: config.current ?? null,
      assigned: config.assigned ?? [],
      minKeys: config.minKeys ?? 2,
      maxKeys: config.maxKeys ?? 4,
      allowSingleKey: config.allowSingleKey ?? false,
      outputFormat: config.outputFormat ?? 'string',
      modifierOrder: config.modifierOrder ?? ['Ctrl', 'Alt', 'Shift', 'Meta'],
    };
    if (this._config.current) {
      this._state = { status: 'idle', current: this._config.current };
    }
  }

  /** 当前快照。 */
  get snapshot(): ShortcutRecorderSnapshot {
    return {
      state: this._state,
      config: { ...this._config },
    };
  }

  /** 当前状态。 */
  get state(): ShortcutRecorderState {
    return this._state;
  }

  /** 当前配置。 */
  get config(): ShortcutRecorderConfig {
    return { ...this._config };
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: ShortcutRecorderSnapshot) => void): () => void {
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

  /** 设置状态。 */
  private _setState(state: ShortcutRecorderState): void {
    this._state = state;
    this._notify();
  }

  /** 设置配置。 */
  setConfig(config: Partial<ShortcutRecorderConfig>): void {
    this._config = { ...this._config, ...config };
    this._notify();
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 开始录入。
   */
  start(): ShortcutRecorderActionResult {
    if (this._state.status === 'recording') {
      return { ok: false, code: 'E_ALREADY_RECORDING', message: '正在录入中' };
    }
    this._pendingModifiers = [];
    this._pendingKey = null;
    this._setState({ status: 'recording', current: this._config.current });
    return { ok: true };
  }

  /**
   * 停止录入。
   */
  stop(): ShortcutRecorderActionResult {
    if (this._state.status !== 'recording') {
      return { ok: false, code: 'E_NOT_RECORDING', message: '未在录入中' };
    }
    this._pendingModifiers = [];
    this._pendingKey = null;
    this._setState({ status: 'idle', current: this._config.current });
    return { ok: true };
  }

  /**
   * 记录修饰键按下。
   */
  recordModifier(modifier: ModifierKey): ShortcutRecorderActionResult {
    if (this._state.status !== 'recording') {
      return { ok: false, code: 'E_NOT_RECORDING', message: '未在录入中' };
    }
    if (this._pendingModifiers.includes(modifier)) {
      return { ok: false, code: 'E_DUPLICATE_MODIFIER', message: '修饰键重复' };
    }
    if (this._pendingModifiers.length >= this._config.maxKeys - 1) {
      return { ok: false, code: 'E_MAX_KEYS', message: '已达最大按键数' };
    }
    this._pendingModifiers.push(modifier);
    return { ok: true };
  }

  /**
   * 记录修饰键释放。
   */
  releaseModifier(modifier: ModifierKey): ShortcutRecorderActionResult {
    if (this._state.status !== 'recording') {
      return { ok: false, code: 'E_NOT_RECORDING', message: '未在录入中' };
    }
    const index = this._pendingModifiers.indexOf(modifier);
    if (index === -1) {
      return { ok: false, code: 'E_NOT_PRESSED', message: '修饰键未按下' };
    }
    this._pendingModifiers.splice(index, 1);
    return { ok: true };
  }

  /**
   * 记录主键按下。
   */
  recordKey(key: string): ShortcutRecorderActionResult {
    if (this._state.status !== 'recording') {
      return { ok: false, code: 'E_NOT_RECORDING', message: '未在录入中' };
    }
    if (this._pendingKey) {
      return { ok: false, code: 'E_KEY_ALREADY_PRESSED', message: '主键已按下' };
    }
    if (this._isModifierKey(key)) {
      return { ok: false, code: 'E_MODIFIER_ONLY', message: '需要按下一个主键' };
    }

    this._pendingKey = key;

    // 检查最少按键数
    const totalKeys = this._pendingModifiers.length + 1;
    if (totalKeys < this._config.minKeys && !this._config.allowSingleKey) {
      this._pendingKey = null;
      this._setState({
        status: 'error',
        current: this._config.current,
        error: `至少需要 ${this._config.minKeys} 个键的组合`,
      });
      return { ok: false, code: 'E_TOO_FEW_KEYS', message: `至少需要 ${this._config.minKeys} 个键` };
    }

    // 检查最大按键数
    if (totalKeys > this._config.maxKeys) {
      this._pendingKey = null;
      this._setState({
        status: 'error',
        current: this._config.current,
        error: `最多 ${this._config.maxKeys} 个键`,
      });
      return { ok: false, code: 'E_TOO_MANY_KEYS', message: `最多 ${this._config.maxKeys} 个键` };
    }

    // 构建快捷键字符串
    const shortcut = this._buildShortcut();

    // 检查冲突
    if (this._hasConflict(shortcut)) {
      this._pendingKey = null;
      this._setState({
        status: 'error',
        current: this._config.current,
        error: `快捷键 ${shortcut} 已被分配`,
      });
      return { ok: false, code: 'E_CONFLICT', message: `快捷键 ${shortcut} 已被分配` };
    }

    // 更新配置
    this._config.current = shortcut;
    this._pendingModifiers = [];
    this._pendingKey = null;
    this._setState({ status: 'idle', current: shortcut });
    return { ok: true, shortcut };
  }

  /**
   * 清除当前快捷键。
   */
  clear(): ShortcutRecorderActionResult {
    this._config.current = null;
    this._setState({ status: 'idle', current: null });
    return { ok: true };
  }

  /**
   * 验证快捷键格式。
   */
  validate(shortcut: string): ShortcutRecorderActionResult {
    if (!shortcut || shortcut.trim() === '') {
      return { ok: false, code: 'E_EMPTY', message: '快捷键不能为空' };
    }

    const parts = shortcut.split('+').filter((p) => p.trim());
    if (parts.length < this._config.minKeys && !this._config.allowSingleKey) {
      return {
        ok: false,
        code: 'E_TOO_FEW_KEYS',
        message: `至少需要 ${this._config.minKeys} 个键`,
      };
    }
    if (parts.length > this._config.maxKeys) {
      return {
        ok: false,
        code: 'E_TOO_MANY_KEYS',
        message: `最多 ${this._config.maxKeys} 个键`,
      };
    }

    // 检查是否有非法字符
    for (const part of parts) {
      const trimmed = part.trim();
      if (!/^[A-Za-z0-9]+$/.test(trimmed)) {
        return { ok: false, code: 'E_INVALID_CHAR', message: `非法字符: ${trimmed}` };
      }
    }

    return { ok: true };
  }

  // ──────────────────────────────────────────────────────────────────────
  // Internal
  // ──────────────────────────────────────────────────────────────────────

  /** 判断是否为修饰键。 */
  private _isModifierKey(key: string): boolean {
    const modifiers = ['Control', 'Alt', 'Shift', 'Meta', 'Ctrl', 'Command', 'Cmd'];
    return modifiers.some((m) => key.toLowerCase() === m.toLowerCase());
  }

  /** 构建快捷键字符串。 */
  private _buildShortcut(): string {
    const parts: string[] = [];

    // 按配置顺序添加修饰键
    for (const modifier of this._config.modifierOrder) {
      if (this._pendingModifiers.includes(modifier)) {
        parts.push(this._formatModifier(modifier));
      }
    }

    // 添加主键
    if (this._pendingKey) {
      parts.push(this._formatKey(this._pendingKey));
    }

    return parts.join('+');
  }

  /** 格式化修饰键。 */
  private _formatModifier(modifier: ModifierKey): string {
    switch (modifier) {
      case 'Ctrl':
        return 'Ctrl';
      case 'Shift':
        return 'Shift';
      case 'Alt':
        return 'Alt';
      case 'Meta':
        return 'Meta';
    }
  }

  /** 格式化主键。 */
  private _formatKey(key: string): string {
    // 特殊键名映射
    const specialKeys: Record<string, string> = {
      ' ': 'Space',
      'Enter': 'Enter',
      'Escape': 'Esc',
      'Backspace': 'Backspace',
      'Tab': 'Tab',
      'Delete': 'Delete',
      'ArrowUp': 'Up',
      'ArrowDown': 'Down',
      'ArrowLeft': 'Left',
      'ArrowRight': 'Right',
    };
    if (specialKeys[key]) {
      return specialKeys[key];
    }
    // 单字符键大写
    if (key.length === 1) {
      return key.toUpperCase();
    }
    return key;
  }

  /** 检查冲突。 */
  private _hasConflict(shortcut: string): boolean {
    return this._config.assigned.some((s) => s.toLowerCase() === shortcut.toLowerCase());
  }
}
