// @vitest-environment happy-dom

import { describe, expect, it, beforeEach } from 'vitest';
import type { ThemeChangeEventDetail } from './theme-picker.js';
import './theme-picker.js';

describe('<oc-theme-picker>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('注册为自定义元素', () => {
    expect(customElements.get('oc-theme-picker')).toBeDefined();
  });

  it('创建实例后渲染容器', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    await el.updateComplete;
    const container = el.shadowRoot?.querySelector('.picker-header');
    expect(container).toBeTruthy();
  });

  it('空主题列表渲染空状态', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    await el.updateComplete;
    const emptyState = el.shadowRoot?.querySelector('.empty-state');
    expect(emptyState).toBeTruthy();
    expect(emptyState?.textContent?.trim()).toContain('暂无可用主题');
  });

  it('设置主题列表后渲染主题卡片', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff', '#2563eb'] },
      { id: 'dark', name: '暗色', isDark: true, colors: ['#111827', '#3b82f6'] },
    ]);
    await el.updateComplete;
    const cards = el.shadowRoot?.querySelectorAll('.theme-card');
    expect(cards?.length).toBe(2);
  });

  it('激活主题高亮显示', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff'] },
      { id: 'dark', name: '暗色', isDark: true, colors: ['#111827'] },
    ]);
    el.setActive('dark');
    await el.updateComplete;
    const activeCard = el.shadowRoot?.querySelector('.theme-card.active');
    expect(activeCard).toBeTruthy();
  });

  it('setActive 更新激活状态并 dispatch 事件', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'dark', name: '暗色', isDark: true, colors: ['#111827'] },
    ]);
    await el.updateComplete;

    const details: ThemeChangeEventDetail[] = [];
    // 类型来自 theme-picker.ts 的 HTMLElementEventMap 增强：`e` 自动推断为
    // CustomEvent<ThemeChangeEventDetail>，无需任何断言。
    el.addEventListener('oc-theme-change', (e) => {
      details.push(e.detail);
    });

    const card = el.shadowRoot?.querySelector('.theme-card');
    expect(card).toBeTruthy();
    // 无法走 card.click()：happy-dom 的 EventTarget 在派发 click 时以错误的
    // 接收者调用 Lit 监听器的 handleEvent（TypeError），这也是原测试绕过 click
    // 的原因；而 _handleThemeClick 是私有方法 —— 因此这里保留唯一一处局部
    // any 来驱动该私有入口，事件本身的类型仍由真实声明提供。
    (el as any)._handleThemeClick({ id: 'dark', name: '暗色', isDark: true, colors: ['#111827'] });

    const eventDetail = details[0];
    expect(eventDetail).toBeTruthy();
    expect(eventDetail?.themeId).toBe('dark');
    expect(eventDetail?.isDark).toBe(true);
  });

  it('过滤器分组显示', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff'] },
      { id: 'dark', name: '暗色', isDark: true, colors: ['#111827'] },
    ]);
    el.setFilter('dark');
    await el.updateComplete;
    const cards = el.shadowRoot?.querySelectorAll('.theme-card');
    expect(cards?.length).toBe(1);
    const nameEl = el.shadowRoot?.querySelector('.theme-name');
    expect(nameEl?.textContent?.trim()).toContain('暗色');
  });

  it('过滤器按钮高亮', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setFilter('dark');
    await el.updateComplete;
    const activeBtn = el.shadowRoot?.querySelector('.filter-btn.active');
    expect(activeBtn).toBeTruthy();
    expect(activeBtn?.textContent?.trim()).toBe('暗色');
  });

  it('activeTheme 属性返回当前主题', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff'] },
      { id: 'dark', name: '暗色', isDark: true, colors: ['#111827'] },
    ]);
    el.setActive('dark');
    await el.updateComplete;
    expect(el.activeTheme?.id).toBe('dark');
    expect(el.activeTheme?.name).toBe('暗色');
  });

  it('颜色预览色板渲染', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff', '#2563eb', '#1f2937'] },
    ]);
    await el.updateComplete;
    const swatches = el.shadowRoot?.querySelectorAll('.color-swatch');
    expect(swatches?.length).toBe(3);
  });

  it('亮色/暗色徽章显示', async () => {
    const el = document.createElement('oc-theme-picker');
    document.body.appendChild(el);
    el.setThemes([
      { id: 'light', name: '亮色', isDark: false, colors: ['#ffffff'] },
      { id: 'dark', name: '暗色', isDark: true, colors: ['#111827'] },
    ]);
    await el.updateComplete;
    const badges = el.shadowRoot?.querySelectorAll('.theme-badge');
    expect(badges?.length).toBe(2);
    const badgeTexts = Array.from(badges ?? []).map((b) => (b as HTMLElement).textContent?.trim());
    expect(badgeTexts).toContain('亮色');
    expect(badgeTexts).toContain('暗色');
  });
});
