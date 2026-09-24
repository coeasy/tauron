// ──────────────────────────────────────────────────────────────────────────
// tauron Web Components（§4.10）。
//
// 设计参考 Shoelace (shoelace.style) 的模式：
// - 非装饰器 API（兼容 vitest/esbuild，无需 experimentalDecorators）
// - 状态管理委托给已有的 Store 类（ToastStore）
// - 组件 subscribe store 变化，触发 requestUpdate()
// - 样式用 CSSResult（Shadow DOM 隔离）
// - 通过 `import '@tauron/ui/wc'` 一次性注册所有自定义元素
// ──────────────────────────────────────────────────────────────────────────

import { LitElement, html, css, type CSSResultGroup } from 'lit';
import { SHELL_EVENTS } from '@tauron/shell-events';
import type { ToastActionEventDetail } from '@tauron/shell-events';

import { ToastStore, type ToastItem, type ToastSnapshot } from './toast.js';

// ──────────────────────────────────────────────────────────────────────────
// <oc-toast> — 统一通知出口
// ──────────────────────────────────────────────────────────────────────────

/**
 * `<oc-toast>` Web Component。
 *
 * 用法：
 * ```html
 * <oc-toast></oc-toast>
 * <script>
 *   const el = document.querySelector('oc-toast');
 *   el.push({ title: 'Hello', message: 'World', level: 'info' });
 * </script>
 * ```
 */
export class OcToast extends LitElement {
  static override styles: CSSResultGroup = css`
    :host {
      display: block;
      position: fixed;
      top: 16px;
      right: 16px;
      z-index: 9999;
      max-width: 400px;
      pointer-events: none;
    }
    .toast-container {
      display: flex;
      flex-direction: column;
      gap: 8px;
      position: relative;
    }
    .toast {
      pointer-events: auto;
      padding: 12px 16px;
      border-radius: 8px;
      box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
      display: flex;
      align-items: flex-start;
      gap: 8px;
      animation: slide-in 0.3s ease-out;
    }
    @keyframes slide-in {
      from { transform: translateX(100%); opacity: 0; }
      to { transform: translateX(0); opacity: 1; }
    }
    .toast.info     { background: var(--oc-toast-info-bg, #e7f5ff); border: 1px solid var(--oc-toast-info-border, #74c0fc); }
    .toast.success  { background: var(--oc-toast-success-bg, #ebfbee); border: 1px solid var(--oc-toast-success-border, #69db7c); }
    .toast.warning  { background: var(--oc-toast-warning-bg, #fff9db); border: 1px solid var(--oc-toast-warning-border, #ffe066); }
    .toast.error    { background: var(--oc-toast-error-bg, #fff5f5); border: 1px solid var(--oc-toast-error-border, #ffa8a8); }
    .toast-content { flex: 1; min-width: 0; }
    .toast-title { font-weight: 600; font-size: 14px; margin: 0 0 4px; }
    .toast-message { font-size: 13px; color: var(--oc-text-secondary, #495057); margin: 0; word-wrap: break-word; }
    .toast-actions { display: flex; gap: 8px; margin-top: 8px; }
    .toast-action {
      background: none; border: 1px solid currentColor; border-radius: 4px;
      padding: 4px 12px; font-size: 12px; cursor: pointer;
    }
    .toast-action:hover { background: rgba(0,0,0,0.05); }
    .toast-close {
      background: none; border: none; cursor: pointer;
      font-size: 18px; line-height: 1; padding: 0 4px;
      color: var(--oc-text-secondary, #868e96);
    }
    .toast-close:hover { color: var(--oc-text-primary, #212529); }
    .badge {
      position: absolute; top: -4px; right: -4px;
      background: var(--oc-danger, #e03131); color: white;
      border-radius: 50%; min-width: 18px; height: 18px;
      display: flex; align-items: center; justify-content: center;
      font-size: 11px; font-weight: 600;
    }
  `;

  private _store: ToastStore | null = null;
  private _unsubscribe: (() => void) | null = null;
  private _snapshot: ToastSnapshot | null = null;

  /** 外部注入的 ToastStore（可选；不传则内部创建）。 */
  set store(value: ToastStore | null) {
    this._disconnect();
    this._store = value;
    if (value) {
      this._unsubscribe = value.subscribe((snap) => {
        this._snapshot = snap;
        this.requestUpdate();
      });
    }
  }
  get store(): ToastStore {
    if (!this._store) {
      this.store = new ToastStore();
    }
    return this._store!;
  }

  /** 接受 ToastStore 作为 property（非装饰器 API）。 */
  // Note: Lit's static properties above handles reactivity for _snapshot.

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this._disconnect();
  }

  private _disconnect(): void {
    if (this._unsubscribe) {
      this._unsubscribe();
      this._unsubscribe = null;
    }
  }

  /** 推送一条通知，返回通知 ID。 */
  push(item: Partial<ToastItem> & { title: string; message: string }): string {
    // `ToastStore.push` 要求显式 level；元素侧保持 level 可选（公开 API 不变），
    // 未提供时默认按 `info` 处理。
    const result = this.store.push({ ...item, level: item.level ?? 'info' });
    if (!result.ok) return '';
    const items = this.store.items;
    return items[items.length - 1]?.id ?? '';
  }

  /** 关闭一条通知。 */
  dismiss(id: string): void {
    this.store.dismiss(id);
  }

  /** 清空所有通知。 */
  clear(): void {
    this.store.clear();
  }

  private _renderToast(item: ToastItem) {
    return html`
      <div class="toast ${item.level}" role="alert">
        <div class="toast-content">
          <p class="toast-title">${item.title}</p>
          <p class="toast-message">${item.message}</p>
          ${item.actions.length > 0
            ? html`<div class="toast-actions">
                ${item.actions.map(
                  (a) => html`<button
                    class="toast-action"
                    @click=${() => this.dispatchEvent(new CustomEvent(SHELL_EVENTS.toastAction, {
                      detail: { toastId: item.id, actionId: a.id, label: a.label } satisfies ToastActionEventDetail,
                      bubbles: true,
                      composed: true,
                    }))}
                  >${a.label}</button>`,
                )}
              </div>`
            : null}
        </div>
        <button
          class="toast-close"
          aria-label="关闭"
          @click=${() => this.dismiss(item.id)}
        >×</button>
      </div>
    `;
  }

  protected override render() {
    const items = this._snapshot?.items ?? [];
    const visible = items.filter((i) => !i.dismissed).slice(-5);
    const unread = this._snapshot?.unreadCount ?? 0;

    return html`
      <div class="toast-container">
        ${unread > 0 ? html`<span class="badge">${unread > 99 ? '99+' : unread}</span>` : null}
        ${visible.map((item) => this._renderToast(item))}
      </div>
    `;
  }
}

// 注册自定义元素
customElements.define('oc-toast', OcToast);

declare global {
  interface HTMLElementTagNameMap {
    'oc-toast': OcToast;
  }
}
