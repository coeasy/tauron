// ──────────────────────────────────────────────────────────────────────────
// CommandPalette 状态管理（开发计划 §4.10）。
//
// 职责：命令面板状态（搜索、过滤、选中、执行）。
//
// 关键约束：
// - 命令按权限过滤（只显示用户可执行的命令）
// - 键盘导航：上下键、Enter、Esc
// - 搜索结果高亮
// - 命令分组
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 命令条目。 */
export interface CommandItem {
  /** 命令 ID。 */
  id: string;
  /** 命令名称。 */
  name: string;
  /** 命令描述。 */
  description: string;
  /** 命令分组。 */
  group: string;
  /** 命令图标（可选）。 */
  icon?: string;
  /** 命令快捷键（可选）。 */
  shortcut?: string;
  /** 命令是否可用。 */
  enabled: boolean;
  /** 命令参数（可选）。 */
  args?: unknown;
  /** 命令执行函数（可选，由外部注入）。 */
  execute?: () => void | Promise<void>;
}

/** 命令面板状态。 */
export type CommandPaletteState =
  | { status: 'closed' }
  | { status: 'open'; query: string; results: CommandItem[]; selectedId: string | null }
  | { status: 'executing'; executingId: string };

/** 命令面板配置。 */
export interface CommandPaletteConfig {
  /** 命令列表。 */
  commands: CommandItem[];
  /** 最大搜索结果数。 */
  maxResults: number;
  /** 是否启用模糊搜索。 */
  fuzzySearch: boolean;
  /** 是否显示分组标题。 */
  showGroupHeaders: boolean;
  /** 快捷键打开命令面板。 */
  shortcut: string;
}

/** 命令面板快照。 */
export interface CommandPaletteSnapshot {
  state: CommandPaletteState;
  config: CommandPaletteConfig;
  filteredResults: CommandItem[];
}

/** 命令面板操作结果。 */
export type CommandPaletteActionResult =
  | { ok: true }
  | { ok: false; code: string; message: string };

// ──────────────────────────────────────────────────────────────────────────
// CommandPaletteStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * CommandPalette 状态存储。
 *
 * 管理命令面板的状态机和操作。
 */
export class CommandPaletteStore {
  private _state: CommandPaletteState = { status: 'closed' };
  private _config: CommandPaletteConfig;
  private readonly _subscribers = new Set<(snapshot: CommandPaletteSnapshot) => void>();

  constructor(config: Partial<CommandPaletteConfig> = {}) {
    this._config = {
      commands: config.commands ?? [],
      maxResults: config.maxResults ?? 20,
      fuzzySearch: config.fuzzySearch ?? true,
      showGroupHeaders: config.showGroupHeaders ?? true,
      shortcut: config.shortcut ?? 'Ctrl+K',
    };
  }

  /** 当前快照。 */
  get snapshot(): CommandPaletteSnapshot {
    return {
      state: this._state,
      config: { ...this._config },
      filteredResults: this._getFilteredResults(),
    };
  }

  /** 当前状态。 */
  get state(): CommandPaletteState {
    return this._state;
  }

  /** 当前配置。 */
  get config(): CommandPaletteConfig {
    return { ...this._config };
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: CommandPaletteSnapshot) => void): () => void {
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
  private _setState(state: CommandPaletteState): void {
    this._state = state;
    this._notify();
  }

  /** 设置配置。 */
  setConfig(config: Partial<CommandPaletteConfig>): void {
    this._config = { ...this._config, ...config };
    this._notify();
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 打开命令面板。
   */
  open(): CommandPaletteActionResult {
    if (this._state.status === 'open' || this._state.status === 'executing') {
      return { ok: false, code: 'E_ALREADY_OPEN', message: '命令面板已打开' };
    }
    this._setState({ status: 'open', query: '', results: [], selectedId: null });
    return { ok: true };
  }

  /**
   * 关闭命令面板。
   */
  close(): CommandPaletteActionResult {
    if (this._state.status === 'closed') {
      return { ok: false, code: 'E_ALREADY_CLOSED', message: '命令面板已关闭' };
    }
    if (this._state.status === 'executing') {
      return { ok: false, code: 'E_EXECUTING', message: '命令正在执行中' };
    }
    this._setState({ status: 'closed' });
    return { ok: true };
  }

  /**
   * 切换命令面板。
   */
  toggle(): CommandPaletteActionResult {
    if (this._state.status === 'closed') {
      return this.open();
    }
    return this.close();
  }

  /**
   * 设置搜索查询。
   */
  setQuery(query: string): CommandPaletteActionResult {
    if (this._state.status !== 'open') {
      return { ok: false, code: 'E_NOT_OPEN', message: '命令面板未打开' };
    }
    if (this._state.status === 'open') {
      this._setState({
        ...this._state,
        query,
        results: this._filterCommands(query),
        selectedId: this._getFirstResultId(query),
      });
    }
    return { ok: true };
  }

  /**
   * 选择命令（键盘导航）。
   */
  select(index: number): CommandPaletteActionResult {
    if (this._state.status !== 'open') {
      return { ok: false, code: 'E_NOT_OPEN', message: '命令面板未打开' };
    }
    if (this._state.status === 'open') {
      const results = this._filterCommands(this._state.query);
      if (index < 0 || index >= results.length) {
        return { ok: false, code: 'E_INVALID_INDEX', message: '索引超出范围' };
      }
      const selectedId = results[index]?.id ?? null;
      this._setState({
        ...this._state,
        selectedId,
      });
    }
    return { ok: true };
  }

  /**
   * 向下移动选择。
   */
  selectNext(): CommandPaletteActionResult {
    if (this._state.status !== 'open') {
      return { ok: false, code: 'E_NOT_OPEN', message: '命令面板未打开' };
    }
    const state = this._state;
    const results = this._filterCommands(state.query);
    const currentIndex = results.findIndex((r) => r.id === state.selectedId);
    const nextIndex = Math.min(currentIndex + 1, results.length - 1);
    const selectedId = results[nextIndex]?.id ?? null;
    this._setState({ ...state, selectedId });
    return { ok: true };
  }

  /**
   * 向上移动选择。
   */
  selectPrev(): CommandPaletteActionResult {
    if (this._state.status !== 'open') {
      return { ok: false, code: 'E_NOT_OPEN', message: '命令面板未打开' };
    }
    const state = this._state;
    const results = this._filterCommands(state.query);
    const currentIndex = results.findIndex((r) => r.id === state.selectedId);
    const prevIndex = Math.max(currentIndex - 1, 0);
    const selectedId = results[prevIndex]?.id ?? null;
    this._setState({ ...state, selectedId });
    return { ok: true };
  }

  /**
   * 执行选中命令。
   */
  execute(): CommandPaletteActionResult {
    if (this._state.status !== 'open') {
      return { ok: false, code: 'E_NOT_OPEN', message: '命令面板未打开' };
    }
    const state = this._state;
    if (!state.selectedId) {
      return { ok: false, code: 'E_NO_SELECTION', message: '未选择命令' };
    }
    const command = this._config.commands.find((c) => c.id === state.selectedId);
    if (!command) {
      return { ok: false, code: 'E_COMMAND_NOT_FOUND', message: '命令不存在' };
    }
    if (!command.enabled) {
      return { ok: false, code: 'E_COMMAND_DISABLED', message: '命令不可用' };
    }

    this._setState({ status: 'executing', executingId: command.id });

    // 执行命令
    if (command.execute) {
      command.execute();
    }

    // 执行后关闭面板
    this._setState({ status: 'closed' });
    return { ok: true };
  }

  // ──────────────────────────────────────────────────────────────────────
  // Internal
  // ──────────────────────────────────────────────────────────────────────

  /** 过滤命令。 */
  private _filterCommands(query: string): CommandItem[] {
    if (!query.trim()) {
      return this._config.commands.slice(0, this._config.maxResults);
    }

    const q = query.toLowerCase();
    const results = this._config.commands.filter((cmd) => {
      if (!cmd.enabled) return false;
      return this._matchesQuery(cmd, q);
    });

    // 按匹配度排序
    results.sort((a, b) => {
      const scoreA = this._matchScore(a, q);
      const scoreB = this._matchScore(b, q);
      return scoreB - scoreA;
    });

    return results.slice(0, this._config.maxResults);
  }

  /** 检查命令是否匹配查询。 */
  private _matchesQuery(cmd: CommandItem, query: string): boolean {
    if (this._config.fuzzySearch) {
      return this._fuzzyMatch(cmd.name, query) || this._fuzzyMatch(cmd.description, query);
    }
    return cmd.name.toLowerCase().includes(query) || cmd.description.toLowerCase().includes(query);
  }

  /** 计算匹配分数。 */
  private _matchScore(cmd: CommandItem, query: string): number {
    let score = 0;
    if (cmd.name.toLowerCase().startsWith(query)) {
      score += 10;
    } else if (cmd.name.toLowerCase().includes(query)) {
      score += 5;
    }
    if (cmd.description.toLowerCase().includes(query)) {
      score += 2;
    }
    if (cmd.group.toLowerCase().includes(query)) {
      score += 1;
    }
    return score;
  }

  /** 模糊匹配。 */
  private _fuzzyMatch(text: string, query: string): boolean {
    let qi = 0;
    for (let ti = 0; ti < text.length && qi < query.length; ti++) {
      // `charAt` 而非下标访问：`noUncheckedIndexedAccess` 下下标访问返回 `string | undefined`。
      if (text.charAt(ti).toLowerCase() === query.charAt(qi)) {
        qi++;
      }
    }
    return qi === query.length;
  }

  /** 获取第一个结果 ID。 */
  private _getFirstResultId(query: string): string | null {
    const results = this._filterCommands(query);
    return results[0]?.id ?? null;
  }

  /** 获取过滤后的结果。 */
  private _getFilteredResults(): CommandItem[] {
    if (this._state.status === 'open') {
      return this._filterCommands(this._state.query);
    }
    return [];
  }
}
