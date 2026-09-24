// @vitest-environment happy-dom
// component-animation.ts 测试（P2-8：组件动画工具）

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { animateEnter, animateLeave, staggerIn, staggerOut, animateListUpdate } from './component-animation.js';

describe('animateEnter()', () => {
  let el: HTMLElement;

  beforeEach(() => {
    el = document.createElement('div');
    document.body.appendChild(el);
  });

  it('fade 动画', async () => {
    await animateEnter(el, { type: 'fade', duration: 50 });
    expect(el.style.opacity).toBe('1');
  });

  it('slide-up 动画', async () => {
    await animateEnter(el, { type: 'slide-up', duration: 50 });
    expect(el.style.opacity).toBe('1');
  });

  it('scale 动画', async () => {
    await animateEnter(el, { type: 'scale', duration: 50 });
    expect(el.style.opacity).toBe('1');
  });

  it('type: none 不执行', async () => {
    await animateEnter(el, { type: 'none' });
    expect(el.style.opacity).toBe('');
  });

  it('延迟执行', async () => {
    const start = Date.now();
    await animateEnter(el, { type: 'fade', duration: 50, delay: 100 });
    const elapsed = Date.now() - start;
    expect(elapsed).toBeGreaterThanOrEqual(140);
  });
});

describe('animateLeave()', () => {
  let el: HTMLElement;

  beforeEach(() => {
    el = document.createElement('div');
    document.body.appendChild(el);
  });

  it('fade 动画', async () => {
    await animateLeave(el, { type: 'fade', duration: 50 });
    expect(el.style.opacity).toBe('0');
  });

  it('slide-down 动画', async () => {
    await animateLeave(el, { type: 'slide-down', duration: 50 });
    expect(el.style.opacity).toBe('0');
  });

  it('type: none 不执行', async () => {
    await animateLeave(el, { type: 'none' });
    expect(el.style.opacity).toBe('');
  });
});

describe('staggerIn()', () => {
  it('交错进入动画', async () => {
    const els = [1, 2, 3].map(() => document.createElement('div'));
    els.forEach((el) => document.body.appendChild(el));
    await staggerIn(els, { type: 'fade', duration: 50, stagger: 20 });
    els.forEach((el) => {
      expect(el.style.opacity).toBe('1');
      el.remove();
    });
  });

  it('空数组不报错', async () => {
    await expect(staggerIn([], { type: 'fade' })).resolves.toBeUndefined();
  });
});

describe('staggerOut()', () => {
  it('交错离开动画', async () => {
    const els = [1, 2, 3].map(() => document.createElement('div'));
    els.forEach((el) => document.body.appendChild(el));
    await staggerOut(els, { type: 'fade', duration: 50, stagger: 20 });
    els.forEach((el) => {
      expect(el.style.opacity).toBe('0');
      el.remove();
    });
  });
});

describe('animateListUpdate()', () => {
  it('新增项播放进入动画', async () => {
    const oldItems = [{ id: '1', element: document.createElement('div') }];
    const newItems = [
      { id: '1', element: document.createElement('div') },
      { id: '2', element: document.createElement('div') },
    ];
    newItems.forEach((i) => document.body.appendChild(i.element));
    await animateListUpdate(newItems, oldItems, { type: 'fade', duration: 50 });
    expect(newItems[1]!.element.style.opacity).toBe('1');
  });

  it('删除项播放离开动画', async () => {
    const oldItems = [
      { id: '1', element: document.createElement('div') },
      { id: '2', element: document.createElement('div') },
    ];
    const newItems = [{ id: '1', element: document.createElement('div') }];
    oldItems.forEach((i) => document.body.appendChild(i.element));
    await animateListUpdate(newItems, oldItems, { type: 'fade', duration: 50 });
    expect(oldItems[1]!.element.style.opacity).toBe('0');
  });

  it('空列表不报错', async () => {
    await expect(animateListUpdate([], [], { type: 'fade' })).resolves.toBeUndefined();
  });
});