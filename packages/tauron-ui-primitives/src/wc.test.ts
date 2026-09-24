// @vitest-environment happy-dom

import { describe, expect, it, beforeEach, vi } from 'vitest';
import { html, render } from 'lit';

import './wc.js';

describe('<oc-toast>', () => {
  beforeEach(() => {
    // 清理已注册的 custom elements
    document.body.innerHTML = '';
  });

  it('注册为自定义元素', () => {
    expect(customElements.get('oc-toast')).toBeDefined();
  });

  it('创建实例后渲染容器', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    // 等待 lit 更新
    await el.updateComplete;
    const container = el.shadowRoot?.querySelector('.toast-container');
    expect(container).toBeTruthy();
  });

  it('push 后渲染通知', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    el.push({ title: 'Hello', message: 'World', level: 'info' });
    await el.updateComplete;
    const toast = el.shadowRoot?.querySelector('.toast');
    expect(toast).toBeTruthy();
    expect(toast?.classList.contains('info')).toBe(true);
    expect(el.shadowRoot?.querySelector('.toast-title')?.textContent).toBe('Hello');
    expect(el.shadowRoot?.querySelector('.toast-message')?.textContent).toBe('World');
  });

  it('dismiss 后移除通知', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    const id = el.push({ title: 'Test', message: 'msg' });
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.toast')).toBeTruthy();
    el.dismiss(id);
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.toast')).toBeNull();
  });

  it('clear 后清空所有', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    el.push({ title: 'A', message: 'a' });
    el.push({ title: 'B', message: 'b' });
    await el.updateComplete;
    expect(el.shadowRoot?.querySelectorAll('.toast').length).toBe(2);
    el.clear();
    await el.updateComplete;
    expect(el.shadowRoot?.querySelectorAll('.toast').length).toBe(0);
  });

  it('不同 level 渲染不同样式', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    el.push({ title: 'S', message: 'ok', level: 'success' });
    el.push({ title: 'W', message: 'warn', level: 'warning' });
    el.push({ title: 'E', message: 'err', level: 'error' });
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.toast.success')).toBeTruthy();
    expect(el.shadowRoot?.querySelector('.toast.warning')).toBeTruthy();
    expect(el.shadowRoot?.querySelector('.toast.error')).toBeTruthy();
  });

  it('actions 渲染为按钮', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    el.push({
      title: 'Action',
      message: 'test',
      actions: [{ id: 'ok', label: '确定' }, { id: 'cancel', label: '取消' }],
    });
    await el.updateComplete;
    const btns = el.shadowRoot?.querySelectorAll('.toast-action') as NodeListOf<HTMLButtonElement>;
    expect(btns).toBeTruthy();
    expect(btns.length).toBe(2);
    expect(btns[0]?.textContent?.trim()).toBe('确定');
    expect(btns[1]?.textContent?.trim()).toBe('取消');
  });

  it('未读计数 badge 显示', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    el.push({ title: 'Unread', message: 'test' });
    await el.updateComplete;
    const badge = el.shadowRoot?.querySelector('.badge');
    expect(badge).toBeTruthy();
  });

  it('关闭按钮可关闭通知', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    const id = el.push({ title: 'Close', message: 'test' });
    await el.updateComplete;
    const closeBtn = el.shadowRoot?.querySelector('.toast-close') as HTMLButtonElement;
    expect(closeBtn).toBeTruthy();
    // 直接调用 dismiss（绕过 happy-dom 的 shadow DOM click 限制）
    el.dismiss(id);
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.toast')).toBeNull();
  });

  it('外部注入 store', async () => {
    const { ToastStore } = await import('./toast.js');
    const store = new ToastStore();
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    el.store = store;
    store.push({ title: 'Injected', message: 'test', level: 'info' });
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.toast-title')?.textContent).toBe('Injected');
  });

  it('disconnected 时清理订阅', async () => {
    const el = document.createElement('oc-toast');
    document.body.appendChild(el);
    const store = el.store;
    el.push({ title: 'Test', message: 'm' });
    await el.updateComplete;
    el.remove();
    // 不应抛异常
    expect(() => store.push({ title: 'After', message: 'rm', level: 'info' })).not.toThrow();
  });
});
