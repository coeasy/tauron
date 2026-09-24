// ──────────────────────────────────────────────────────────────────────────
// tauron Web Components 壳（§4.10 剩余组件）。
//
// 包含：TitleBar、TrayMenu、UpdaterDialog、CommandPalette、
//       ShortcutRecorder、PluginManager。
// 非装饰器 API（兼容 vitest/esbuild），Shadow DOM 隔离。
// ──────────────────────────────────────────────────────────────────────────

import { LitElement, html, css, type CSSResultGroup } from 'lit';
import { SHELL_EVENTS } from '@tauron/shell-events';
import type {
  TrayItemEventDetail,
  CommandSelectEventDetail,
  ShortcutChangeEventDetail,
  PluginToggleEventDetail,
} from '@tauron/shell-events';
import { ShortcutRecorderStore, type ShortcutRecorderState, type ModifierKey } from './shortcut-recorder.js';

// detail 类型的事实源在 `@tauron/shell-events`（R3）：这里转出以保持既有
// `from '@tauron/ui-primitives'` 的导入路径可用，避免两处定义漂移。
export type {
  TrayItemEventDetail,
  CommandSelectEventDetail,
  ShortcutChangeEventDetail,
  PluginToggleEventDetail,
};

// ──────────────────────────────────────────────────────────────────────────
// <oc-title-bar> — 标题栏
// ──────────────────────────────────────────────────────────────────────────

export class OcTitleBar extends LitElement {
  static override styles: CSSResultGroup = css`
    :host { padding: 8px 16px; background: var(--oc-title-bar-bg, #f8f9fa); border-bottom: 1px solid var(--oc-border-color, #dee2e6); height: 48px; user-select: none; }
    .title-root { display: flex; align-items: center; justify-content: space-between; width: 100%; height: 100%; }
    .title-left { display: flex; align-items: center; gap: 12px; }
    .app-icon { width: 24px; height: 24px; border-radius: 4px; background: var(--oc-primary-color, #2563eb); }
    .app-title { font-weight: 600; font-size: 14px; margin: 0; color: var(--oc-text-color, #212529); }
    .title-actions { display: flex; align-items: center; gap: 8px; }
    .title-btn { background: none; border: none; cursor: pointer; padding: 4px 8px; border-radius: 4px; font-size: 14px; color: var(--oc-text-secondary, #495057); }
    .title-btn:hover { background: rgba(0,0,0,0.05); }
  `;

  private _title: string = 'Open Client';
  private _subtitle: string = '';

  // `title`/`subtitle` 是 HTMLElement 上已有的成员，这里覆盖其 getter/setter。
  override get title(): string { return this._title; }
  override set title(v: string) { this._title = v; this.requestUpdate(); }

  get subtitle(): string { return this._subtitle; }
  set subtitle(v: string) { this._subtitle = v; this.requestUpdate(); }

  protected override render() {
    return html`
      <div data-tauri-drag-region class="title-root">
        <div class="title-left">
          <div class="app-icon"></div>
          <h1 class="app-title">${this._title}</h1>
          ${this._subtitle ? html`<span class="subtitle">${this._subtitle}</span>` : ''}
        </div>
        <div class="title-actions" data-tauri-drag-region=false>
          <button class="title-btn" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.minimize, { bubbles: true, composed: true }))}>—</button>
          <button class="title-btn" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.maximize, { bubbles: true, composed: true }))}>□</button>
          <button class="title-btn" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.close, { bubbles: true, composed: true }))}>✕</button>
        </div>
      </div>
    `;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// <oc-tray-menu> — 托盘菜单
// ──────────────────────────────────────────────────────────────────────────

export interface TrayMenuItem {
  id: string;
  label: string;
  icon?: string;
  shortcut?: string;
  disabled?: boolean;
  separator?: boolean;
}

export class OcTrayMenu extends LitElement {
  static override styles: CSSResultGroup = css`
    :host { display: flex; flex-direction: column; background: var(--oc-bg-color, #ffffff); border: 1px solid var(--oc-border-color, #dee2e6); border-radius: 8px; box-shadow: 0 4px 12px rgba(0,0,0,0.15); min-width: 200px; padding: 4px; }
    .menu-item { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-radius: 4px; cursor: pointer; font-size: 13px; color: var(--oc-text-color, #212529); }
    .menu-item:hover:not(.disabled) { background: var(--oc-bg-secondary, #f8f9fa); }
    .menu-item.disabled { opacity: 0.5; cursor: not-allowed; }
    .menu-icon { width: 16px; text-align: center; }
    .menu-label { flex: 1; }
    .menu-shortcut { color: var(--oc-text-secondary, #868e96); font-size: 11px; }
    .menu-separator { height: 1px; background: var(--oc-border-color, #dee2e6); margin: 4px 0; }
  `;

  private _items: TrayMenuItem[] = [];

  get items(): TrayMenuItem[] { return this._items; }
  set items(v: TrayMenuItem[]) { this._items = v; this.requestUpdate(); }

  protected override render() {
    return html`
      ${this._items.map((item) => {
        if (item.separator) return html`<div class="menu-separator"></div>`;
        return html`
          <div class="menu-item ${item.disabled ? 'disabled' : ''}" @click=${() => !item.disabled && this.dispatchEvent(new CustomEvent(SHELL_EVENTS.trayItem, { bubbles: true, composed: true, detail: { id: item.id } satisfies TrayItemEventDetail }))}>
            ${item.icon ? html`<span class="menu-icon">${item.icon}</span>` : ''}
            <span class="menu-label">${item.label}</span>
            ${item.shortcut ? html`<span class="menu-shortcut">${item.shortcut}</span>` : ''}
          </div>
        `;
      })}
    `;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// <oc-updater-dialog> — 更新对话框
// ──────────────────────────────────────────────────────────────────────────

export class OcUpdaterDialog extends LitElement {
  static override styles: CSSResultGroup = css`
    :host { display: none; position: fixed; top: 0; left: 0; right: 0; bottom: 0; z-index: 10000; background: rgba(0,0,0,0.5); align-items: center; justify-content: center; }
    :host([open]) { display: flex; }
    .dialog { background: var(--oc-bg-color, #ffffff); border-radius: 12px; padding: 24px; max-width: 480px; width: 90%; box-shadow: 0 8px 24px rgba(0,0,0,0.2); }
    .dialog-title { font-size: 18px; font-weight: 600; margin: 0 0 16px; color: var(--oc-text-color, #212529); }
    .dialog-content { font-size: 14px; color: var(--oc-text-secondary, #495057); margin-bottom: 24px; }
    .progress-bar { height: 8px; background: var(--oc-bg-secondary, #f8f9fa); border-radius: 4px; overflow: hidden; margin-bottom: 16px; }
    .progress-fill { height: 100%; background: var(--oc-primary-color, #2563eb); transition: width 0.3s; border-radius: 4px; }
    .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; }
    .btn { padding: 8px 16px; border-radius: 6px; border: 1px solid var(--oc-border-color, #dee2e6); background: var(--oc-bg-color, #ffffff); cursor: pointer; font-size: 13px; }
    .btn-primary { background: var(--oc-primary-color, #2563eb); color: white; border-color: var(--oc-primary-color, #2563eb); }
  `;

  private _open: boolean = false;
  private _version: string = '';
  private _message: string = '';
  private _progress: number = 0;
  private _status: string = 'idle';

  get open(): boolean { return this._open; }
  set open(v: boolean) { this._open = v; this.requestUpdate(); }

  get version(): string { return this._version; }
  set version(v: string) { this._version = v; this.requestUpdate(); }

  get message(): string { return this._message; }
  set message(v: string) { this._message = v; this.requestUpdate(); }

  get progress(): number { return this._progress; }
  set progress(v: number) { this._progress = v; this.requestUpdate(); }

  get status(): string { return this._status; }
  set status(v: string) { this._status = v; this.requestUpdate(); }

  protected override render() {
    return html`
      <div class="dialog">
        <h2 class="dialog-title">软件更新</h2>
        ${this._version ? html`<div class="dialog-content">新版本 ${this._version} 可用</div>` : ''}
        ${this._message ? html`<div class="dialog-content">${this._message}</div>` : ''}
        ${this._status === 'downloading' || this._status === 'updating' ? html`<div class="progress-bar"><div class="progress-fill" style="width: ${this._progress}%"></div></div>` : ''}
        <div class="dialog-actions">
          <button class="btn" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.close, { bubbles: true, composed: true }))}>稍后</button>
          ${this._status === 'done'
            ? html`<button class="btn btn-primary" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.restart, { bubbles: true, composed: true }))}>立即重启</button>`
            : this._status === 'idle'
              ? html`<button class="btn btn-primary" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.updaterCheck, { bubbles: true, composed: true }))}>检查更新</button>`
              : html`<button class="btn btn-primary" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.updateStart, { bubbles: true, composed: true }))}>开始更新</button>`}
        </div>
      </div>
    `;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// <oc-command-palette> — 命令面板
//
// 契约（哑组件，数据与执行都在接入方）：
// - 供数：接入方设置 `commands` 属性——典型来源是
//   `ShellClient.contributesList('command')`（插件的贡献命令）
// - 消费：选中项派发 `oc-command-select`（detail `{ id }`）——执行是应用域
//   （跨窗口命令调用走 `host_plugin_call` 的 C/D 后端路径），
//   ShellController 不接本事件
// ──────────────────────────────────────────────────────────────────────────

export interface CommandItem {
  id: string;
  label: string;
  icon?: string;
  shortcut?: string;
  category?: string;
}

export class OcCommandPalette extends LitElement {
  static override styles: CSSResultGroup = css`
    :host { display: none; position: fixed; top: 0; left: 0; right: 0; bottom: 0; z-index: 10001; background: rgba(0,0,0,0.5); padding-top: 20vh; }
    :host([open]) { display: block; }
    .palette { background: var(--oc-bg-color, #ffffff); border-radius: 12px; max-width: 640px; margin: 0 auto; box-shadow: 0 8px 24px rgba(0,0,0,0.2); overflow: hidden; }
    .palette-input { width: 100%; padding: 16px 20px; border: none; border-bottom: 1px solid var(--oc-border-color, #dee2e6); font-size: 16px; outline: none; }
    .palette-list { max-height: 400px; overflow-y: auto; padding: 8px; }
    .palette-item { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border-radius: 6px; cursor: pointer; font-size: 14px; }
    .palette-item:hover { background: var(--oc-bg-secondary, #f8f9fa); }
    .palette-icon { width: 20px; text-align: center; }
    .palette-label { flex: 1; }
    .palette-shortcut { color: var(--oc-text-secondary, #868e96); font-size: 12px; }
  `;

  private _open: boolean = false;
  private _commands: CommandItem[] = [];
  private _query: string = '';

  get open(): boolean { return this._open; }
  set open(v: boolean) { this._open = v; this.requestUpdate(); }

  get commands(): CommandItem[] { return this._commands; }
  set commands(v: CommandItem[]) { this._commands = v; this.requestUpdate(); }

  get query(): string { return this._query; }
  set query(v: string) { this._query = v; this.requestUpdate(); }

  protected override render() {
    const filtered = this._filterCommands(this._commands, this._query);
    return html`
      <div class="palette">
        <input class="palette-input" type="text" placeholder="搜索命令..." .value="${this._query}" @input="${(e: Event) => { this._query = (e.target as HTMLInputElement).value; this.requestUpdate(); }}" />
        <div class="palette-list">
          ${filtered.length === 0 ? html`<div class="palette-item">无匹配命令</div>` : filtered.map((cmd) => html`
            <div class="palette-item" @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.commandSelect, { bubbles: true, composed: true, detail: { id: cmd.id } satisfies CommandSelectEventDetail }))}>
              ${cmd.icon ? html`<span class="palette-icon">${cmd.icon}</span>` : ''}
              <span class="palette-label">${cmd.label}</span>
              ${cmd.shortcut ? html`<span class="palette-shortcut">${cmd.shortcut}</span>` : ''}
            </div>
          `)}
        </div>
      </div>
    `;
  }

  private _filterCommands(commands: CommandItem[], query: string): CommandItem[] {
    if (!query) return commands;
    const q = query.toLowerCase();
    return commands.filter((c) => c.label.toLowerCase().includes(q) || c.id.toLowerCase().includes(q));
  }
}

// ──────────────────────────────────────────────────────────────────────────
// <oc-shortcut-recorder> — 快捷键录制器
// ──────────────────────────────────────────────────────────────────────────

export class OcShortcutRecorder extends LitElement {
  static override styles: CSSResultGroup = css`
    :host { display: inline-flex; align-items: center; gap: 8px; }
    .recorder { padding: 6px 12px; border: 1px solid var(--oc-border-color, #dee2e6); border-radius: 6px; background: var(--oc-bg-color, #ffffff); cursor: pointer; font-size: 13px; min-width: 80px; text-align: center; }
    .recorder.recording { border-color: var(--oc-primary-color, #2563eb); background: var(--oc-bg-secondary, #f8f9fa); }
    .recorder.error { border-color: var(--oc-danger-color, #e03131); background: var(--oc-bg-secondary, #f8f9fa); }
    .clear-btn { background: none; border: none; cursor: pointer; padding: 4px; color: var(--oc-text-secondary, #868e96); font-size: 14px; }
    .error-msg { font-size: 11px; color: var(--oc-danger-color, #e03131); max-width: 200px; }
  `;

  /** 外部注入的 Store（默认自建）。 */
  store: ShortcutRecorderStore;

  private _shortcut: string = '';
  private _recording: boolean = false;
  private _error: string | null = null;
  private _unsubscribe: (() => void) | null = null;
  private _keydownHandler: ((e: KeyboardEvent) => void) | null = null;
  private _keyupHandler: ((e: KeyboardEvent) => void) | null = null;

  constructor() {
    super();
    this.store = new ShortcutRecorderStore();
    this._shortcut = this.store.config.current ?? '';
  }

  get shortcut(): string { return this._shortcut; }
  set shortcut(v: string) { this._shortcut = v; this.requestUpdate(); }

  get recording(): boolean { return this._recording; }
  set recording(v: boolean) { this._recording = v; this.requestUpdate(); }

  override firstUpdated(): void {
    // 订阅 Store 状态变化
    this._unsubscribe = this.store.subscribe((snapshot) => {
      const { state } = snapshot;
      this._shortcut = state.current ?? '';
      if (state.status === 'error') {
        this._error = state.error;
      }
      this.requestUpdate();
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this._stopListening();
    this._unsubscribe?.();
  }

  protected override render() {
    const status = this._recording ? 'recording' : (this._error ? 'error' : '');
    return html`
      <div class="recorder ${status}" @click="${this._handleClick}">
        ${this._recording ? '按下快捷键...' : (this._shortcut || '未设置')}
      </div>
      ${this._shortcut ? html`<button class="clear-btn" @click="${this._handleClear}">✕</button>` : ''}
      ${this._error ? html`<span class="error-msg">${this._error}</span>` : ''}
    `;
  }

  private _handleClick(): void {
    if (this._recording) {
      this._handleClear();
      return;
    }
    this._error = null;
    const result = this.store.start();
    if (result.ok) {
      this._recording = true;
      this._startListening();
      this.requestUpdate();
    }
  }

  private _handleClear(): void {
    this._stopListening();
    this._recording = false;
    this._error = null;
    this.store.clear();
    this._shortcut = '';
    this.dispatchEvent(new CustomEvent(SHELL_EVENTS.shortcutChange, {
      detail: { shortcut: '' } satisfies ShortcutChangeEventDetail,
      bubbles: true,
      composed: true,
    }));
  }

  private _startListening(): void {
    this._keydownHandler = (e: KeyboardEvent) => this._onKeyDown(e);
    this._keyupHandler = (e: KeyboardEvent) => this._onKeyUp(e);
    window.addEventListener('keydown', this._keydownHandler, true);
    window.addEventListener('keyup', this._keyupHandler, true);
  }

  private _stopListening(): void {
    if (this._keydownHandler) {
      window.removeEventListener('keydown', this._keydownHandler, true);
      this._keydownHandler = null;
    }
    if (this._keyupHandler) {
      window.removeEventListener('keyup', this._keyupHandler, true);
      this._keyupHandler = null;
    }
  }

  private _onKeyDown(e: KeyboardEvent): void {
    if (!this._recording) return;

    // Escape = 取消录入
    if (e.key === 'Escape') {
      e.preventDefault();
      this._stopListening();
      this._recording = false;
      this._error = null;
      this.requestUpdate();
      return;
    }

    // 修饰键
    const modifier = this._keyToModifier(e);
    if (modifier) {
      e.preventDefault();
      e.stopPropagation();
      const result = this.store.recordModifier(modifier);
      if (!result.ok) {
        this._error = 'message' in result ? result.message : null;
        this.requestUpdate();
      }
      return;
    }

    // 主键
    const key = e.key;
    if (key.length === 1 || ['Enter', 'Escape', 'Backspace', 'Delete', 'Tab', 'Space', 'F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12'].includes(key)) {
      e.preventDefault();
      e.stopPropagation();
      const result = this.store.recordKey(key === ' ' ? 'Space' : key);
      if (result.ok) {
        // 录入成功
        this._shortcut = result.shortcut ?? '';
        this._recording = false;
        this._error = null;
        this._stopListening();
        this.dispatchEvent(new CustomEvent(SHELL_EVENTS.shortcutChange, {
          detail: { shortcut: this._shortcut } satisfies ShortcutChangeEventDetail,
          bubbles: true,
          composed: true,
        }));
      } else {
        // 录入失败（冲突/按键数不足）
        this._error = 'message' in result ? result.message : null;
        this._recording = false;
        this._stopListening();
      }
      this.requestUpdate();
    }
  }

  private _onKeyUp(e: KeyboardEvent): void {
    if (!this._recording) return;
    const modifier = this._keyToModifier(e);
    if (modifier) {
      e.preventDefault();
      e.stopPropagation();
      this.store.releaseModifier(modifier);
    }
  }

  private _keyToModifier(e: KeyboardEvent): ModifierKey | null {
    if (e.ctrlKey) return 'Ctrl';
    if (e.shiftKey) return 'Shift';
    if (e.altKey) return 'Alt';
    if (e.metaKey) return 'Meta';
    return null;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// <oc-plugin-manager> — 插件管理器
// ──────────────────────────────────────────────────────────────────────────

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  enabled: boolean;
  type: 'js' | 'process' | 'wasm';
  description?: string;
}

export class OcPluginManager extends LitElement {
  static override styles: CSSResultGroup = css`
    :host { display: block; padding: 16px; font-size: 14px; }
    .manager-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 16px; }
    .manager-title { font-size: 18px; font-weight: 600; margin: 0; color: var(--oc-text-color, #212529); }
    .plugin-list { display: flex; flex-direction: column; gap: 8px; }
    .plugin-item { display: flex; align-items: center; gap: 12px; padding: 12px 16px; border: 1px solid var(--oc-border-color, #dee2e6); border-radius: 8px; background: var(--oc-bg-color, #ffffff); }
    .plugin-info { flex: 1; }
    .plugin-name { font-weight: 600; margin: 0 0 4px; color: var(--oc-text-color, #212529); }
    .plugin-meta { font-size: 12px; color: var(--oc-text-secondary, #868e96); }
    .toggle-switch { width: 40px; height: 24px; background: var(--oc-bg-secondary, #f8f9fa); border-radius: 12px; position: relative; cursor: pointer; border: 1px solid var(--oc-border-color, #dee2e6); }
    .toggle-switch.enabled { background: var(--oc-primary-color, #2563eb); border-color: var(--oc-primary-color, #2563eb); }
    .toggle-knob { width: 18px; height: 18px; background: white; border-radius: 50%; position: absolute; top: 2px; left: 2px; transition: left 0.2s; }
    .toggle-switch.enabled .toggle-knob { left: 18px; }
    .empty-state { text-align: center; padding: 32px; color: var(--oc-text-secondary, #868e96); }
  `;

  private _plugins: PluginInfo[] = [];

  get plugins(): PluginInfo[] { return this._plugins; }
  set plugins(v: PluginInfo[]) { this._plugins = v; this.requestUpdate(); }

  protected override render() {
    if (this._plugins.length === 0) {
      return html`<div class="empty-state">暂无插件</div>`;
    }
    return html`
      <div class="manager-header">
        <h2 class="manager-title">插件管理</h2>
        <span>${this._plugins.filter(p => p.enabled).length}/${this._plugins.length} 已启用</span>
      </div>
      <div class="plugin-list">
        ${this._plugins.map((plugin) => html`
          <div class="plugin-item">
            <div class="plugin-info">
              <p class="plugin-name">${plugin.name}</p>
              <p class="plugin-meta">v${plugin.version} · ${plugin.type} · ${plugin.id}</p>
              ${plugin.description ? html`<p class="plugin-meta">${plugin.description}</p>` : ''}
            </div>
            <div class="toggle-switch ${plugin.enabled ? 'enabled' : ''}" @click="${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.pluginToggle, { bubbles: true, composed: true, detail: { id: plugin.id, enabled: !plugin.enabled } satisfies PluginToggleEventDetail }))}">
              <div class="toggle-knob"></div>
            </div>
          </div>
        `)}
      </div>
    `;
  }
}

// ── 注册所有组件 ──

customElements.define('oc-title-bar', OcTitleBar);
customElements.define('oc-tray-menu', OcTrayMenu);
customElements.define('oc-updater-dialog', OcUpdaterDialog);
customElements.define('oc-command-palette', OcCommandPalette);
customElements.define('oc-shortcut-recorder', OcShortcutRecorder);
customElements.define('oc-plugin-manager', OcPluginManager);

// ──────────────────────────────────────────────────────────────────────────
// 全局类型登记（与 theme-picker.ts / wc.ts 同一模式）
//
// 让使用方直接获得类型化的 `document.createElement('oc-…')` 与
// `addEventListener('oc-…', …)`（监听器参数自动推断为对应事件类型）。
// 本文件经 index.ts 导出，因此该增强会随 `@tauron/ui` 入口一并生效。
// ──────────────────────────────────────────────────────────────────────────

declare global {
  interface HTMLElementTagNameMap {
    'oc-title-bar': OcTitleBar;
    'oc-tray-menu': OcTrayMenu;
    'oc-updater-dialog': OcUpdaterDialog;
    'oc-command-palette': OcCommandPalette;
    'oc-shortcut-recorder': OcShortcutRecorder;
    'oc-plugin-manager': OcPluginManager;
  }

  interface HTMLElementEventMap {
    /**
     * 通知类事件（无 detail）。
     *
     * `new CustomEvent(name)` 未传 `detail`，按 DOM 规范 `detail` 为 `null`。
     */
    'oc-minimize': CustomEvent<null>;
    /** @see 'oc-minimize' */
    'oc-maximize': CustomEvent<null>;
    /** `<oc-title-bar>` / `<oc-updater-dialog>` 的关闭请求。 @see 'oc-minimize' */
    'oc-close': CustomEvent<null>;
    /** `<oc-updater-dialog>` 请求开始更新。 @see 'oc-minimize' */
    'oc-update-start': CustomEvent<null>;
    /** `<oc-updater-dialog>` 请求重启应用。 @see 'oc-minimize' */
    'oc-restart': CustomEvent<null>;
    /** `<oc-tray-menu>` 菜单项被点击。 */
    'oc-tray-item': CustomEvent<TrayItemEventDetail>;
    /** `<oc-command-palette>` 命令被选中。 */
    'oc-command-select': CustomEvent<CommandSelectEventDetail>;
    /** `<oc-shortcut-recorder>` 快捷键变化。 */
    'oc-shortcut-change': CustomEvent<ShortcutChangeEventDetail>;
    /** `<oc-plugin-manager>` 插件启用状态切换。 */
    'oc-plugin-toggle': CustomEvent<PluginToggleEventDetail>;
  }
}
