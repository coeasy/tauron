// ──────────────────────────────────────────────────────────────────────────
// <oc-skeleton> — 骨架屏组件（P3-1）。
//
// 职责：
// 1. 显示加载占位（shimmer 动画）
// 2. 支持多种形状（circle/rect/text/line）
// 3. 支持自定义尺寸和布局
// 4. 尊重 prefers-reduced-motion
//
// 用法：
// ```html
// <oc-skeleton shape="rect" width="200" height="100"></oc-skeleton>
// <oc-skeleton shape="text" lines="3"></oc-skeleton>
// <oc-skeleton circle></oc-skeleton>
// ```
// ──────────────────────────────────────────────────────────────────────────

import { LitElement, html, css } from 'lit';
import { prefersReducedMotion } from './motion.js';

/** 骨架形状类型 */
export type SkeletonShape = 'circle' | 'rect' | 'text' | 'line' | 'image';

/**
 * <oc-skeleton> — 骨架屏 Web Component。
 */
export class OcSkeleton extends LitElement {
  static override styles = css`
    :host {
      display: inline-block;
      position: relative;
      overflow: hidden;
      background-color: var(--oc-skeleton-bg, #f0f0f0);
      border-radius: var(--oc-skeleton-radius, 4px);
    }

    :host([circle]) {
      border-radius: 50%;
    }

    :host([shape='text']) {
      display: block;
      width: 100%;
    }

    .skeleton-content {
      position: relative;
      width: 100%;
      height: 100%;
    }

    .skeleton-shimmer {
      position: absolute;
      inset: 0;
      background: linear-gradient(
        90deg,
        transparent 0%,
        var(--oc-skeleton-shimmer, rgba(255, 255, 255, 0.4)) 50%,
        transparent 100%
      );
      transform: translateX(-100%);
      animation: shimmer 1.5s infinite;
    }

    @keyframes shimmer {
      100% {
        transform: translateX(100%);
      }
    }

    @media (prefers-reduced-motion: reduce) {
      .skeleton-shimmer {
        animation: none;
        opacity: 0;
      }
    }
  `;

  /** 骨架形状 */
  private _shape: SkeletonShape = 'rect';
  get shape(): SkeletonShape { return this._shape; }
  set shape(v: SkeletonShape) { this._shape = v; this.requestUpdate(); }

  /** 宽度（px） */
  private _width: number = 200;
  get width(): number { return this._width; }
  set width(v: number) { this._width = v; this.requestUpdate(); }

  /** 高度（px） */
  private _height: number = 20;
  get height(): number { return this._height; }
  set height(v: number) { this._height = v; this.requestUpdate(); }

  /** 文本行数（shape='text' 时） */
  private _lines: number = 1;
  get lines(): number { return this._lines; }
  set lines(v: number) { this._lines = v; this.requestUpdate(); }

  /** 是否启用 shimmer 动画 */
  private _animated: boolean = true;
  get animated(): boolean { return this._animated; }
  set animated(v: boolean) { this._animated = v; this.requestUpdate(); }

  /**
   * 渲染骨架屏。
   */
  override render() {
    if (this._shape === 'text') {
      return html`
        ${Array.from({ length: this._lines }, (_, i) =>
          i === this._lines - 1
            ? html`<div class="skeleton-line" style="width: 80%; height: 16px; margin: 8px 0; border-radius: 4px; background: var(--oc-skeleton-bg, #f0f0f0); position: relative; overflow: hidden;">${this._renderShimmer()}</div>`
            : html`<div class="skeleton-line" style="width: 100%; height: 16px; margin: 8px 0; border-radius: 4px; background: var(--oc-skeleton-bg, #f0f0f0); position: relative; overflow: hidden;">${this._renderShimmer()}</div>`,
        )}
      `;
    }

    const styleParts: string[] = [];
    styleParts.push(`width: ${this._width}px`);
    styleParts.push(`height: ${this._height}px`);

    if (this._shape === 'circle') {
      styleParts.push(`border-radius: 50%`);
    }

    return html`<div class="skeleton-content" style="${styleParts.join(';')}">${this._renderShimmer()}</div>`;
  }

  /**
   * 渲染 shimmer 动画。
   */
  private _renderShimmer() {
    if (!this._animated || prefersReducedMotion()) {
      return '';
    }
    return html`<div class="skeleton-shimmer"></div>`;
  }

  /**
   * 设置 shape。
   */
  setShape(shape: SkeletonShape): void {
    this._shape = shape;
    this.requestUpdate();
  }

  /**
   * 设置尺寸。
   */
  setSize(width: number, height: number): void {
    this._width = width;
    this._height = height;
    this.requestUpdate();
  }
}

customElements.define('oc-skeleton', OcSkeleton);