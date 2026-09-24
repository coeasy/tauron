// @vitest-environment happy-dom
// skeleton.ts 测试（P3-1：骨架屏组件）

import { describe, it, expect, beforeEach } from 'vitest';
import { OcSkeleton } from './skeleton.js';

describe('<oc-skeleton>', () => {
  let el: OcSkeleton;

  beforeEach(async () => {
    el = document.createElement('oc-skeleton') as OcSkeleton;
    document.body.appendChild(el);
    await el.updateComplete;
  });

  it('创建实例', () => {
    expect(el).toBeDefined();
    expect(el.tagName.toLowerCase()).toBe('oc-skeleton');
  });

  it('默认 shape 为 rect', () => {
    expect(el.shape).toBe('rect');
  });

  it('默认 width 为 200', () => {
    expect(el.width).toBe(200);
  });

  it('默认 height 为 20', () => {
    expect(el.height).toBe(20);
  });

  it('默认 lines 为 1', () => {
    expect(el.lines).toBe(1);
  });

  it('设置 shape 属性', async () => {
    el.shape = 'circle';
    await el.updateComplete;
    expect(el.shape).toBe('circle');
  });

  it('设置 width/height', async () => {
    el.setSize(300, 150);
    await el.updateComplete;
    expect(el.width).toBe(300);
    expect(el.height).toBe(150);
  });

  it('设置 shape 为 text', async () => {
    el.shape = 'text';
    el.lines = 3;
    await el.updateComplete;
    expect(el.shape).toBe('text');
    expect(el.lines).toBe(3);
  });

  it('animated 为 false 时不渲染 shimmer', async () => {
    el.animated = false;
    await el.updateComplete;
    // shimmer 元素不应存在
    const shimmer = el.shadowRoot?.querySelector('.skeleton-shimmer');
    expect(shimmer).toBeNull();
  });

  it('animated 为 true 时渲染 shimmer', async () => {
    el.animated = true;
    await el.updateComplete;
    const shimmer = el.shadowRoot?.querySelector('.skeleton-shimmer');
    expect(shimmer).not.toBeNull();
  });

  it('circle shape 渲染圆形', async () => {
    el.shape = 'circle';
    el.width = 100;
    el.height = 100;
    await el.updateComplete;
    const content = el.shadowRoot?.querySelector('.skeleton-content');
    expect(content?.getAttribute('style')).toContain('border-radius: 50%');
  });

  it('text shape 渲染多行', async () => {
    el.shape = 'text';
    el.lines = 3;
    await el.updateComplete;
    const divs = el.shadowRoot?.querySelectorAll('.skeleton-line');
    expect(divs?.length).toBe(3);
  });
});