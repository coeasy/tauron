// wc-motion.ts 测试（P1-6 Splash 启动画面）
// @vitest-environment happy-dom

import { describe, it, expect, beforeEach } from 'vitest';
import {
  SplashStore,
  DEFAULT_SPLASH_CONFIG,
  type SplashPhase,
  type SplashConfig,
} from './wc-motion.js';
import './wc-motion.js';
import type { OcSplash } from './wc-motion.js';

describe('SplashStore', () => {
  let store: SplashStore;

  beforeEach(() => {
    store = new SplashStore();
  });

  // ── 初始状态 ──

  describe('初始状态', () => {
    it('默认阶段为 entering', () => {
      expect(store.phase).toBe('entering');
    });

    it('默认进度为 0', () => {
      expect(store.progress).toBe(0);
    });

    it('默认配置正确', () => {
      const config = store.config;
      expect(config.enabled).toBe(true);
      expect(config.minDuration).toBe(1500);
      expect(config.title).toBe('Tauron');
      expect(config.progress).toBe('bar');
      expect(config.exitAnimation).toBe('fade');
    });
  });

  // ── 配置 ──

  describe('setConfig', () => {
    it('可覆盖 title', () => {
      store.setConfig({ title: 'My App' });
      expect(store.config.title).toBe('My App');
    });

    it('可覆盖 logo', () => {
      store.setConfig({ logo: './logo.png' });
      expect(store.config.logo).toBe('./logo.png');
    });

    it('可覆盖 progress 模式', () => {
      store.setConfig({ progress: 'spinner' });
      expect(store.config.progress).toBe('spinner');
    });

    it('可覆盖 background', () => {
      store.setConfig({ background: '#000' });
      expect(store.config.background).toBe('#000');
    });
  });

  // ── 进度 ──

  describe('setProgress', () => {
    it('可设置进度', () => {
      store.setProgress(0.5);
      expect(store.progress).toBe(0.5);
    });

    it('进度不超过 1', () => {
      store.setProgress(1.5);
      expect(store.progress).toBe(1);
    });

    it('进度不低于 0', () => {
      store.setProgress(-0.5);
      expect(store.progress).toBe(0);
    });
  });

  // ── 阶段转换 ──

  describe('阶段转换', () => {
    it('start() 设置 entering', () => {
      store.start();
      expect(store.phase).toBe('entering');
    });

    it('markReady() 设置 exiting', () => {
      store.start();
      store.markReady();
      expect(store.phase).toBe('exiting');
    });

    it('complete() 设置 done', () => {
      store.complete();
      expect(store.phase).toBe('done');
    });

    it('cancel() 直接完成', () => {
      store.start();
      store.cancel();
      expect(store.phase).toBe('done');
    });

    it('complete() 幂等', () => {
      store.complete();
      store.complete();
      expect(store.phase).toBe('done');
    });

    it('markReady() 在 done 后无效', () => {
      store.complete();
      store.markReady();
      expect(store.phase).toBe('done');
    });
  });

  // ── 快照 ──

  describe('snapshot', () => {
    it('包含 phase/progress/config', () => {
      const snap = store.snapshot;
      expect(snap.phase).toBe('entering');
      expect(snap.progress).toBe(0);
      expect(snap.config).toBeDefined();
      expect(snap.config.title).toBe('Tauron');
    });

    it('config 是副本', () => {
      const snap = store.snapshot;
      snap.config.title = 'Modified';
      expect(store.config.title).toBe('Tauron');
    });
  });

  // ── 订阅 ──

  describe('subscribe', () => {
    it('初始订阅触发一次', () => {
      const snapshots: string[] = [];
      store.subscribe((snap) => snapshots.push(snap.phase));
      expect(snapshots.length).toBe(1);
      expect(snapshots[0]).toBe('entering');
    });

    it('状态变化触发通知', () => {
      const phases: string[] = [];
      store.subscribe((snap) => phases.push(snap.phase));
      store.start();
      expect(phases.length).toBeGreaterThanOrEqual(1);
    });

    it('退订后不再通知', () => {
      let count = 0;
      const unsub = store.subscribe(() => count++);
      unsub();
      store.start();
      expect(count).toBe(1); // 只有初始通知
    });

    it('订阅者异常不影响其他订阅者', () => {
      let okCount = 0;
      store.subscribe(() => { throw new Error('test'); });
      const unsub = store.subscribe(() => okCount++);
      unsub();
      expect(okCount).toBe(1);
    });
  });

  // ── 自定义配置 ──

  describe('自定义配置构造', () => {
    it('支持自定义 title', () => {
      const store = new SplashStore({ title: 'Custom App' });
      expect(store.config.title).toBe('Custom App');
    });

    it('支持自定义 minDuration', () => {
      const store = new SplashStore({ minDuration: 100 });
      expect(store.config.minDuration).toBe(100);
    });

    it('支持自定义 exitAnimation', () => {
      const store = new SplashStore({ exitAnimation: 'slide-down' });
      expect(store.config.exitAnimation).toBe('slide-down');
    });

    it('支持 enabled=false', () => {
      const store = new SplashStore({ enabled: false });
      expect(store.config.enabled).toBe(false);
    });
  });

  // ── destroy ──

  describe('destroy', () => {
    it('清理订阅者', () => {
      let count = 0;
      store.subscribe(() => count++);
      store.destroy();
      store.start();
      expect(count).toBe(1); // 只有初始通知
    });
  });

  // ── DEFAULT_SPLASH_CONFIG ──

  describe('DEFAULT_SPLASH_CONFIG', () => {
    it('默认值正确', () => {
      expect(DEFAULT_SPLASH_CONFIG.enabled).toBe(true);
      expect(DEFAULT_SPLASH_CONFIG.minDuration).toBe(1500);
      expect(DEFAULT_SPLASH_CONFIG.title).toBe('Tauron');
      expect(DEFAULT_SPLASH_CONFIG.progress).toBe('bar');
      expect(DEFAULT_SPLASH_CONFIG.exitAnimation).toBe('fade');
    });
  });
});

// ── <oc-splash> 组件渲染 ─────────────────────────────────────────────────

describe('<oc-splash>', () => {
  it('注册为自定义元素', () => {
    expect(customElements.get('oc-splash')).toBeDefined();
  });

  it('customHtml 注入为真实 DOM（unsafeHTML），而非字面属性', async () => {
    const el = document.createElement('oc-splash') as OcSplash;
    el.store = new SplashStore({
      customHtml: '<b data-x="brand">Logo</b>',
      exitAnimation: 'none',
    });
    document.body.appendChild(el);
    await el.updateComplete;

    const container = el.shadowRoot?.querySelector('.splash-container');
    expect(container).toBeTruthy();
    // 旧实现用 React 的 dangerouslySetInnerHTML（Lit 不认识）→ 渲染成字面属性，
    // 自定义 HTML 静默失效；回归门禁：必须产出真实子元素。
    expect(container?.querySelector('b[data-x="brand"]')).toBeTruthy();
    expect(container?.hasAttribute('dangerouslySetInnerHTML')).toBe(false);

    el.remove();
  });
});