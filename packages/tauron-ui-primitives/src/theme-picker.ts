// ──────────────────────────────────────────────────────────────────────────
// <oc-theme-picker> — 主题选择器 Web Component。
//
// 设计参考 Shoelace `data-theme` + CSS 变量模式：
// - 展示所有可用主题（亮色/暗色分组）
// - 点击切换主题，dispatch `oc-theme-change` 事件
// - 颜色预览色板
// - Shadow DOM 样式隔离
// ──────────────────────────────────────────────────────────────────────────

import { LitElement, html, css, type CSSResultGroup } from 'lit';
import { SHELL_EVENTS } from '@tauron/shell-events';
import type { ThemeChangeEventDetail } from '@tauron/shell-events';

// detail 类型的事实源在 `@tauron/shell-events`（R3）；这里转出以保持既有导入路径。
export type { ThemeChangeEventDetail };

// ── 类型 ──

/** 主题预览信息。 */
export interface ThemePreview {
  id: string;
  name: string;
  isDark: boolean;
  colors: string[];
}

/** 主题选择器状态快照。 */
export interface ThemePickerSnapshot {
  themes: ThemePreview[];
  activeId: string | null;
  filter: 'all' | 'light' | 'dark';
}

// ── ThemeStore ──

/**
 * 主题选择器状态存储。
 */
export class ThemePickerStore {
  private _themes: ThemePreview[] = [];
  private _activeId: string | null = null;
  private _filter: 'all' | 'light' | 'dark' = 'all';
  private readonly _subscribers = new Set<(snapshot: ThemePickerSnapshot) => void>();

  /** 设置主题列表。 */
  setThemes(themes: ThemePreview[]): void {
    this._themes = [...themes];
    this._notify();
  }

  /** 获取主题列表。 */
  get themes(): ThemePreview[] {
    return [...this._themes];
  }

  /** 设置激活主题 ID。 */
  setActive(id: string): void {
    this._activeId = id;
    this._notify();
  }

  /** 获取激活主题 ID。 */
  get activeId(): string | null {
    return this._activeId;
  }

  /** 设置过滤器。 */
  setFilter(filter: 'all' | 'light' | 'dark'): void {
    this._filter = filter;
    this._notify();
  }

  /** 获取过滤器。 */
  get filter(): 'all' | 'light' | 'dark' {
    return this._filter;
  }

  /** 获取快照。 */
  get snapshot(): ThemePickerSnapshot {
    return {
      themes: this._filterThemes(),
      activeId: this._activeId,
      filter: this._filter,
    };
  }

  /** 获取当前激活主题。 */
  get activeTheme(): ThemePreview | undefined {
    return this._themes.find((t) => t.id === this._activeId);
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: ThemePickerSnapshot) => void): () => void {
    this._subscribers.add(fn);
    fn(this.snapshot);
    return () => {
      this._subscribers.delete(fn);
    };
  }

  private _filterThemes(): ThemePreview[] {
    if (this._filter === 'all') return this._themes;
    if (this._filter === 'dark') return this._themes.filter((t) => t.isDark);
    return this._themes.filter((t) => !t.isDark);
  }

  private _notify(): void {
    const snap = this.snapshot;
    for (const fn of this._subscribers) {
      try {
        fn(snap);
      } catch {
        // 订阅者异常不影响其他订阅者
      }
    }
  }
}

// ── <oc-theme-picker> ──

/**
 * `<oc-theme-picker>` Web Component。
 *
 * 用法：
 * ```html
 * <oc-theme-picker></oc-theme-picker>
 * <script>
 *   const picker = document.querySelector('oc-theme-picker');
 *   picker.setThemes([
 *     { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff', '#2563eb'] },
 *     { id: 'dark', name: '暗色', isDark: true, colors: ['#111827', '#3b82f6'] },
 *   ]);
 *   picker.addEventListener('oc-theme-change', (e) => {
 *     console.log('Theme changed:', e.detail.themeId);
 *   });
 * </script>
 * ```
 */
export class OcThemePicker extends LitElement {
  static override styles: CSSResultGroup = css`
    :host {
      display: block;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      font-size: 14px;
      color: var(--oc-color-text, #1f2937);
    }

    .picker-header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      margin-bottom: 12px;
    }

    .picker-title {
      font-weight: 600;
      font-size: 16px;
      margin: 0;
    }

    .filter-group {
      display: flex;
      gap: 4px;
    }

    .filter-btn {
      padding: 4px 12px;
      border: 1px solid var(--oc-color-border, #e5e7eb);
      border-radius: 6px;
      background: var(--oc-color-background, #ffffff);
      color: var(--oc-color-text-secondary, #6b7280);
      font-size: 12px;
      cursor: pointer;
      transition: all 150ms ease-in-out;
    }

    .filter-btn:hover {
      border-color: var(--oc-color-border-hover, #d1d5db);
      color: var(--oc-color-text, #1f2937);
    }

    .filter-btn.active {
      background: var(--oc-color-primary, #2563eb);
      border-color: var(--oc-color-primary, #2563eb);
      color: #ffffff;
    }

    .theme-list {
      display: flex;
      flex-direction: column;
      gap: 8px;
    }

    .theme-card {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 12px;
      border: 2px solid var(--oc-color-border, #e5e7eb);
      border-radius: 8px;
      background: var(--oc-color-background, #ffffff);
      cursor: pointer;
      transition: all 150ms ease-in-out;
    }

    .theme-card:hover {
      border-color: var(--oc-color-border-hover, #d1d5db);
      box-shadow: 0 2px 8px rgba(0, 0, 0, 0.08);
    }

    .theme-card.active {
      border-color: var(--oc-color-primary, #2563eb);
      box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.15);
    }

    .theme-preview {
      display: flex;
      gap: 4px;
      flex-shrink: 0;
    }

    .color-swatch {
      width: 24px;
      height: 24px;
      border-radius: 4px;
      border: 1px solid rgba(0, 0, 0, 0.1);
    }

    .theme-info {
      flex: 1;
      min-width: 0;
    }

    .theme-name {
      font-weight: 500;
      font-size: 14px;
      margin: 0;
    }

    .theme-badge {
      display: inline-block;
      padding: 2px 8px;
      border-radius: 4px;
      font-size: 11px;
      font-weight: 500;
      margin-left: 8px;
    }

    .badge-light {
      background: #f3f4f6;
      color: #4b5563;
    }

    .badge-dark {
      background: #1f2937;
      color: #f9fafb;
    }

    .theme-check {
      width: 20px;
      height: 20px;
      border-radius: 50%;
      border: 2px solid var(--oc-color-border, #e5e7eb);
      flex-shrink: 0;
      display: flex;
      align-items: center;
      justify-content: center;
    }

    .theme-card.active .theme-check {
      border-color: var(--oc-color-primary, #2563eb);
      background: var(--oc-color-primary, #2563eb);
    }

    .theme-check svg {
      width: 12px;
      height: 12px;
      fill: #ffffff;
    }

    .empty-state {
      text-align: center;
      padding: 32px 16px;
      color: var(--oc-color-text-muted, #9ca3af);
    }
  `;

  private _store = new ThemePickerStore();
  private _unsubscribe: (() => void) | null = null;
  private _snapshot: ThemePickerSnapshot | null = null;
  private _activeId: string | null = null;
  private _filter: 'all' | 'light' | 'dark' = 'all';

  override connectedCallback(): void {
    super.connectedCallback();
    this._unsubscribe = this._store.subscribe((snapshot) => {
      this._snapshot = snapshot;
      this.requestUpdate();
    });
  }

  override disconnectedCallback(): void {
    if (this._unsubscribe) {
      this._unsubscribe();
      this._unsubscribe = null;
    }
    super.disconnectedCallback();
  }

  /** 设置主题列表。 */
  setThemes(themes: ThemePreview[]): void {
    this._store.setThemes(themes);
  }

  /** 设置激活主题。 */
  setActive(id: string): void {
    this._activeId = id;
    this._store.setActive(id);
  }

  /** 设置过滤器。 */
  setFilter(filter: 'all' | 'light' | 'dark'): void {
    this._filter = filter;
    this._store.setFilter(filter);
  }

  /** 获取当前激活主题 ID。 */
  get activeThemeId(): string | null {
    return this._activeId;
  }

  /** 获取当前激活主题。 */
  get activeTheme(): ThemePreview | undefined {
    return this._store.themes.find((t) => t.id === this._activeId);
  }

  private _handleThemeClick(theme: ThemePreview): void {
    this._store.setActive(theme.id);
    this.dispatchEvent(
      new CustomEvent(SHELL_EVENTS.themeChange, {
        detail: {
          themeId: theme.id,
          themeName: theme.name,
          isDark: theme.isDark,
        } satisfies ThemeChangeEventDetail,
        bubbles: true,
        composed: true,
      }),
    );
  }

  private _renderFilterButton(label: string, value: 'all' | 'light' | 'dark'): unknown {
    const isActive = this._store.filter === value;
    return html`
      <button
        class="filter-btn ${isActive ? 'active' : ''}"
        @click=${() => this.setFilter(value)}
      >
        ${label}
      </button>
    `;
  }

  private _renderThemeCard(theme: ThemePreview): unknown {
    const isActive = this._store.activeId === theme.id;
    return html`
      <div
        class="theme-card ${isActive ? 'active' : ''}"
        @click=${() => this._handleThemeClick(theme)}
        role="radio"
        aria-checked="${isActive}"
        tabindex="0"
      >
        <div class="theme-preview">
          ${theme.colors.slice(0, 4).map((color) => html`<div
              class="color-swatch"
              style="background-color: ${color};"
            ></div>`)}
        </div>
        <div class="theme-info">
          <p class="theme-name">
            ${theme.name}
            <span class="theme-badge ${theme.isDark ? 'badge-dark' : 'badge-light'}">
              ${theme.isDark ? '暗色' : '亮色'}
            </span>
          </p>
        </div>
        <div class="theme-check">
          ${isActive
            ? html`<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                <path
                  d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"
                />
              </svg>`
            : ''}
        </div>
      </div>
    `;
  }

  protected override render(): unknown {
    const themes = this._store.themes;
    const filteredThemes = this._snapshot?.themes ?? themes;

    return html`
      <div class="picker-header">
        <h3 class="picker-title">主题</h3>
        <div class="filter-group">
          ${this._renderFilterButton('全部', 'all')}
          ${this._renderFilterButton('亮色', 'light')}
          ${this._renderFilterButton('暗色', 'dark')}
        </div>
      </div>
      ${
        filteredThemes.length === 0
          ? html`<div class="empty-state">暂无可用主题</div>`
          : html`<div class="theme-list">
              ${filteredThemes.map((theme) => this._renderThemeCard(theme))}
            </div>`
      }
    `;
  }
}

customElements.define('oc-theme-picker', OcThemePicker);

declare global {
  interface HTMLElementTagNameMap {
    'oc-theme-picker': OcThemePicker;
  }

  interface HTMLElementEventMap {
    [SHELL_EVENTS.themeChange]: CustomEvent<ThemeChangeEventDetail>;
  }
}
