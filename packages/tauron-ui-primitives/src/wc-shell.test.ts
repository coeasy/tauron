// @vitest-environment happy-dom
// ──────────────────────────────────────────────────────────────────────────
// WC 壳组件测试（TitleBar、TrayMenu、UpdaterDialog、CommandPalette、
// ShortcutRecorder、PluginManager）。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it, beforeEach } from 'vitest';
import './wc-shell.js';

describe('<oc-title-bar>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('注册为自定义元素', () => {
    expect(customElements.get('oc-title-bar')).toBeDefined();
  });

  it('渲染标题', async () => {
    const el = document.createElement('oc-title-bar');
    el.title = 'My App';
    document.body.appendChild(el);
    await el.updateComplete;
    const title = el.shadowRoot?.querySelector('.app-title');
    expect(title?.textContent).toContain('My App');
  });
});

describe('<oc-tray-menu>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('渲染菜单项', async () => {
    const el = document.createElement('oc-tray-menu');
    el.items = [
      { id: 'refresh', label: '刷新' },
      { id: 'settings', label: '设置' },
      // separator 不渲染 label，但 TrayMenuItem.label 为必填字段
      { id: 'sep', label: '', separator: true },
      { id: 'quit', label: '退出' },
    ];
    document.body.appendChild(el);
    await el.updateComplete;
    const items = el.shadowRoot?.querySelectorAll('.menu-item');
    expect(items?.length).toBe(3); // 不含 separator
  });
});

describe('<oc-updater-dialog>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('open 时可见', async () => {
    const el = document.createElement('oc-updater-dialog');
    el.open = true;
    el.version = '2.0.0';
    document.body.appendChild(el);
    await el.updateComplete;
    const dialog = el.shadowRoot?.querySelector('.dialog');
    expect(dialog).toBeTruthy();
  });
});

describe('<oc-command-palette>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('渲染命令列表', async () => {
    const el = document.createElement('oc-command-palette');
    el.open = true;
    el.commands = [
      { id: 'cmd1', label: '命令 1' },
      { id: 'cmd2', label: '命令 2' },
    ];
    document.body.appendChild(el);
    await el.updateComplete;
    const items = el.shadowRoot?.querySelectorAll('.palette-item');
    expect(items?.length).toBe(2);
  });
});

describe('<oc-shortcut-recorder>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('显示快捷键', async () => {
    const el = document.createElement('oc-shortcut-recorder');
    el.shortcut = 'Ctrl+K';
    document.body.appendChild(el);
    await el.updateComplete;
    const recorder = el.shadowRoot?.querySelector('.recorder');
    expect(recorder?.textContent).toContain('Ctrl+K');
  });
});

describe('<oc-plugin-manager>', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('渲染插件列表', async () => {
    const el = document.createElement('oc-plugin-manager');
    el.plugins = [
      { id: 'p1', name: 'Plugin 1', version: '1.0.0', enabled: true, type: 'js' },
      { id: 'p2', name: 'Plugin 2', version: '2.0.0', enabled: false, type: 'wasm' },
    ];
    document.body.appendChild(el);
    await el.updateComplete;
    const items = el.shadowRoot?.querySelectorAll('.plugin-item');
    expect(items?.length).toBe(2);
  });

  it('空列表显示空状态', async () => {
    const el = document.createElement('oc-plugin-manager');
    el.plugins = [];
    document.body.appendChild(el);
    await el.updateComplete;
    const empty = el.shadowRoot?.querySelector('.empty-state');
    expect(empty?.textContent).toContain('暂无插件');
  });
});
