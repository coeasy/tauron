// ──────────────────────────────────────────────────────────────────────────
// <oc-splash> — 启动画面 Web Component（§5.4 动效与启动退出体验）。
//
// 职责：
// - 品牌 logo + 标题 + 进度条
// - 淡入 → 保持 → 淡出动画序列
// - 可配置（通过 store 或属性注入）
//
// 设计原则：
// - 零配置默认：不传任何配置也有可用的默认启动画面
// - 配置覆盖：通过 SplashStore 或属性覆盖
// - 动画序列：fade-in → minDuration → fade-out
// ──────────────────────────────────────────────────────────────────────────

import { LitElement, html, css, type CSSResultGroup } from 'lit';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { shouldAnimate, prefersReducedMotion } from './motion.js';

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** Splash 进度模式。 */
export type SplashProgressMode = 'bar' | 'spinner' | 'dots' | 'none';

/** Splash 退出动画。 */
export type SplashExitAnimation = 'fade' | 'slide-down' | 'scale-out' | 'none';

/** Splash 配置。 */
export interface SplashConfig {
  /** 是否启用（false=不显示）。 */
  enabled: boolean;
  /** 最短显示时间（ms）。 */
  minDuration: number;
  /** 品牌 logo（URL 或 data URI）。 */
  logo?: string;
  /** 标题。 */
  title: string;
  /** 副标题。 */
  subtitle?: string;
  /** 背景色/渐变。 */
  background: string;
  /** 进度条模式。 */
  progress: SplashProgressMode;
  /** 退出动画。 */
  exitAnimation: SplashExitAnimation;
  /** 自定义 HTML（template='custom' 时使用）。 */
  customHtml?: string;
}

/** 默认 Splash 配置。 */
export const DEFAULT_SPLASH_CONFIG: SplashConfig = {
  enabled: true,
  minDuration: 1500,
  title: 'Tauron',
  background: 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)',
  progress: 'bar',
  exitAnimation: 'fade',
};

/** Splash 阶段。 */
export type SplashPhase = 'entering' | 'holding' | 'exiting' | 'done';

/** Splash 快照。 */
export interface SplashSnapshot {
  phase: SplashPhase;
  progress: number;
  config: SplashConfig;
}

// ──────────────────────────────────────────────────────────────────────────
// SplashStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * Splash 状态存储。
 *
 * 管理启动画面的阶段和进度。
 */
export class SplashStore {
  private _phase: SplashPhase = 'entering';
  private _progress: number = 0;
  private _config: SplashConfig;
  private readonly _subscribers = new Set<(snapshot: SplashSnapshot) => void>();
  private _minDurationTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(config: Partial<SplashConfig> = {}) {
    this._config = { ...DEFAULT_SPLASH_CONFIG, ...config };
  }

  /** 当前快照。 */
  get snapshot(): SplashSnapshot {
    return {
      phase: this._phase,
      progress: this._progress,
      config: { ...this._config },
    };
  }

  /** 当前阶段。 */
  get phase(): SplashPhase {
    return this._phase;
  }

  /** 当前进度。 */
  get progress(): number {
    return this._progress;
  }

  /** 当前配置。 */
  get config(): SplashConfig {
    return { ...this._config };
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: SplashSnapshot) => void): () => void {
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
    const snap = this.snapshot;
    for (const fn of this._subscribers) {
      try {
        fn(snap);
      } catch {
        // 忽略订阅者异常
      }
    }
  }

  /** 设置配置。 */
  setConfig(config: Partial<SplashConfig>): void {
    this._config = { ...this._config, ...config };
    this._notify();
  }

  /** 更新进度（0-1）。 */
  setProgress(progress: number): void {
    this._progress = Math.max(0, Math.min(1, progress));
    this._notify();
  }

  /**
   * 开始显示（标记为 entering，启动 minDuration 计时器）。
   */
  start(): void {
    this._phase = 'entering';
    this._notify();

    // 启动最短显示时间计时器
    this._minDurationTimer = setTimeout(() => {
      if (this._phase === 'entering' || this._phase === 'holding') {
        this._phase = 'holding';
        this._notify();
      }
    }, this._config.minDuration);
  }

  /**
   * 标记就绪（app ready 后调用）。
   * 开始退出动画。
   */
  markReady(): void {
    if (this._phase === 'done') return;
    this._phase = 'exiting';
    this._notify();
  }

  /**
   * 完成退出动画（退出动画播放完毕后调用）。
   */
  complete(): void {
    this._phase = 'done';
    if (this._minDurationTimer) {
      clearTimeout(this._minDurationTimer);
      this._minDurationTimer = null;
    }
    this._notify();
  }

  /**
   * 取消（强制立即完成，不播放退出动画）。
   */
  cancel(): void {
    this.complete();
  }

  /** 销毁（清理资源）。 */
  destroy(): void {
    if (this._minDurationTimer) {
      clearTimeout(this._minDurationTimer);
      this._minDurationTimer = null;
    }
    this._subscribers.clear();
  }
}

// ──────────────────────────────────────────────────────────────────────────
// <oc-splash> Web Component
// ──────────────────────────────────────────────────────────────────────────

/**
 * `<oc-splash>` Web Component。
 *
 * 用法：
 * ```html
 * <oc-splash></oc-splash>
 * ```
 *
 * 或注入配置：
 * ```html
 * <oc-splash data-logo="./logo.png" data-title="My App"></oc-splash>
 * ```
 */
export class OcSplash extends LitElement {
  static override styles: CSSResultGroup = css`
    :host {
      position: fixed;
      inset: 0;
      z-index: 9999;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      transition: opacity var(--oc-motion-duration-normal, 300ms) var(--oc-motion-easing-standard, ease-in-out);
    }
    :host([data-phase="entering"]) {
      animation: oc-splash-fade-in var(--oc-motion-duration-normal, 300ms) var(--oc-motion-easing-decelerated, ease-out) both;
    }
    :host([data-phase="exiting"]) {
      opacity: 0;
      transition: opacity var(--oc-motion-duration-normal, 300ms) var(--oc-motion-easing-accelerated, ease-in);
    }
    :host([data-phase="done"]) {
      display: none;
    }
    .splash-container {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 16px;
      padding: 32px;
      max-width: 400px;
      text-align: center;
    }
    .splash-logo {
      width: 64px;
      height: 64px;
      border-radius: 12px;
      background: rgba(255, 255, 255, 0.2);
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 28px;
      color: #fff;
      animation: oc-splash-pulse 2s ease-in-out infinite;
    }
    .splash-logo img {
      width: 40px;
      height: 40px;
      object-fit: contain;
    }
    .splash-title {
      font-size: 20px;
      font-weight: 600;
      color: #fff;
      margin: 0;
    }
    .splash-subtitle {
      font-size: 13px;
      color: rgba(255, 255, 255, 0.7);
      margin: 0;
    }
    .splash-progress {
      width: 200px;
      height: 3px;
      background: rgba(255, 255, 255, 0.2);
      border-radius: 2px;
      overflow: hidden;
      margin-top: 8px;
    }
    .splash-progress-bar {
      height: 100%;
      background: #fff;
      border-radius: 2px;
      transition: width var(--oc-motion-duration-normal, 300ms) var(--oc-motion-easing-standard, ease-in-out);
    }
    .splash-spinner {
      width: 24px;
      height: 24px;
      border: 3px solid rgba(255, 255, 255, 0.2);
      border-top-color: #fff;
      border-radius: 50%;
      animation: oc-splash-spin 0.8s linear infinite;
      margin-top: 8px;
    }
    .splash-dots {
      display: flex;
      gap: 6px;
      margin-top: 8px;
    }
    .splash-dot {
      width: 8px;
      height: 8px;
      border-radius: 50%;
      background: rgba(255, 255, 255, 0.4);
      animation: oc-splash-dot 1.4s ease-in-out infinite;
    }
    .splash-dot:nth-child(2) { animation-delay: 0.2s; }
    .splash-dot:nth-child(3) { animation-delay: 0.4s; }

    @keyframes oc-splash-fade-in {
      from { opacity: 0; }
      to { opacity: 1; }
    }
    @keyframes oc-splash-pulse {
      0%, 100% { transform: scale(1); opacity: 1; }
      50% { transform: scale(1.05); opacity: 0.85; }
    }
    @keyframes oc-splash-spin {
      to { transform: rotate(360deg); }
    }
    @keyframes oc-splash-dot {
      0%, 80%, 100% { transform: scale(0.6); opacity: 0.4; }
      40% { transform: scale(1); opacity: 1; }
    }
  `;

  /** 外部注入的 Store。 */
  store: SplashStore;

  private _unsubscribe: (() => void) | null = null;
  private _exitTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    super();
    this.store = new SplashStore();
  }

  override connectedCallback(): void {
    super.connectedCallback();

    // 读取 data-* 属性覆盖配置
    const dataLogo = this.getAttribute('data-logo');
    const dataTitle = this.getAttribute('data-title');
    const dataSubtitle = this.getAttribute('data-subtitle');
    const dataBackground = this.getAttribute('data-background');
    const dataProgress = this.getAttribute('data-progress') as SplashProgressMode | null;
    const dataMinDuration = this.getAttribute('data-min-duration');

    const overrides: Partial<SplashConfig> = {};
    if (dataLogo) overrides.logo = dataLogo;
    if (dataTitle) overrides.title = dataTitle;
    if (dataSubtitle) overrides.subtitle = dataSubtitle;
    if (dataBackground) overrides.background = dataBackground;
    if (dataProgress) overrides.progress = dataProgress;
    if (dataMinDuration) overrides.minDuration = parseInt(dataMinDuration, 10);

    if (Object.keys(overrides).length > 0) {
      this.store.setConfig(overrides);
    }

    // 订阅 store
    this._unsubscribe = this.store.subscribe((snap) => {
      this.setAttribute('data-phase', snap.phase);
      this.requestUpdate();
    });

    // 自动开始
    if (this.store.config.enabled) {
      this.store.start();
    }

    // 监听 data-ready 属性变化
    const observer = new MutationObserver((mutations) => {
      for (const m of mutations) {
        if (m.attributeName === 'data-ready' && this.hasAttribute('data-ready')) {
          this._handleReady();
        }
      }
    });
    observer.observe(this, { attributes: true, attributeFilter: ['data-ready'] });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this._unsubscribe?.();
    if (this._exitTimer) clearTimeout(this._exitTimer);
    this.store.destroy();
  }

  private _handleReady(): void {
    if (!shouldAnimate()) {
      // reduced-motion: 立即完成
      this.store.complete();
      return;
    }
    this.store.markReady();
    // 等待退出动画播放完毕
    const duration = this.store.config.exitAnimation === 'none' ? 0 : 350;
    this._exitTimer = setTimeout(() => this.store.complete(), duration);
  }

  protected override render() {
    const snap = this.store.snapshot;
    if (!snap.config.enabled) return html``;

    // 自定义 HTML 模式（unsafeHTML 指令：Lit 不认识 React 的 dangerouslySetInnerHTML，
    // 直接写会渲染成字面属性导致 customHtml 静默失效）
    if (snap.config.customHtml) {
      return html`<div class="splash-container">${unsafeHTML(snap.config.customHtml)}</div>`;
    }

    return html`
      <div class="splash-container">
        ${snap.config.logo
          ? html`<div class="splash-logo"><img src="${snap.config.logo}" alt="logo" /></div>`
          : html`<div class="splash-logo">${snap.config.title.charAt(0)}</div>`}
        <h1 class="splash-title">${snap.config.title}</h1>
        ${snap.config.subtitle ? html`<p class="splash-subtitle">${snap.config.subtitle}</p>` : ''}
        ${this._renderProgress(snap)}
      </div>
    `;
  }

  private _renderProgress(snap: SplashSnapshot) {
    switch (snap.config.progress) {
      case 'bar':
        return html`
          <div class="splash-progress">
            <div class="splash-progress-bar" style="width: ${snap.progress * 100}%"></div>
          </div>`;
      case 'spinner':
        return html`<div class="splash-spinner"></div>`;
      case 'dots':
        return html`<div class="splash-dots"><span class="splash-dot"></span><span class="splash-dot"></span><span class="splash-dot"></span></div>`;
      case 'none':
        return html``;
    }
  }

  /** 外部调用：标记 app 就绪，触发退出动画。 */
  markReady(): void {
    this._handleReady();
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'oc-splash': OcSplash;
  }
}

customElements.define('oc-splash', OcSplash);