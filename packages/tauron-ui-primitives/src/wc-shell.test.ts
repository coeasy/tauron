// @vitest-environment happy-dom
// ──────────────────────────────────────────────────────────────────────────
// WC 壳组件测试（TitleBar、TrayMenu、UpdaterDialog、CommandPalette、
// ShortcutRecorder、PluginManager）。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it, beforeEach } from 'vitest';
import { UPDATER_STATUSES } from '@tauron/shell-events';
import './wc-shell.js';
import type { OcShortcutRecorder, UpdaterStatus } from './wc-shell.js';
import { ShortcutRecorderStore } from './shortcut-recorder.js';

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

  it('open 反映到宿主属性（`:host([open])` 才是可见性开关）', async () => {
    const el = document.createElement('oc-updater-dialog');
    document.body.appendChild(el);
    expect(el.hasAttribute('open')).toBe(false);
    el.open = true;
    expect(el.hasAttribute('open'), 'open=true 必须反映到属性，否则组件永远不可见').toBe(true);
    el.open = false;
    expect(el.hasAttribute('open')).toBe(false);
  });

  it('「稍后」派发 oc-updater-dismiss 并收起对话框——**不得**派发 oc-close', async () => {
    // 回归锁：该按钮曾派发 `SHELL_EVENTS.close`，而 ShellController 把
    // `oc-close` 无条件路由到 `windowClose()` → 点「稍后」把主窗口关掉。
    //
    // 无法走 `later.click()`：happy-dom 的 EventTarget 派发 click 时以错误的
    // 接收者调用 Lit 监听器的 `handleEvent`（TypeError，见 theme-picker.test.ts
    // 同一处注释）。`_dismiss` 是私有方法，因此这里保留一处局部 any 驱动该入口。
    const el = document.createElement('oc-updater-dialog');
    el.open = true;
    document.body.appendChild(el);
    await el.updateComplete;

    const seen: string[] = [];
    for (const type of ['oc-updater-dismiss', 'oc-close', 'oc-minimize']) {
      document.body.addEventListener(type, () => seen.push(type));
    }

    (el as unknown as { _dismiss(): void })._dismiss();

    expect(seen).toEqual(['oc-updater-dismiss']);
    expect(el.open, '「稍后」后对话框应收起').toBe(false);
    expect(el.hasAttribute('open')).toBe(false);
  });

  // ── 状态 → 主按钮（轮 32：词表与 `@tauron/host` 写侧同源）──────────────
  //
  // 这条腿此前是断的：组件的重启分支只认 `'done'`，而客户端的已装好终态叫 `'ready'`，
  // 全仓没人产出 `'done'`——「立即重启」在真装配里点不出来，`oc-restart` →
  // `host_window_relaunch` 整条腿因此形同虚设。

  const dialogAt = async (status: UpdaterStatus) => {
    const el = document.createElement('oc-updater-dialog');
    el.open = true;
    el.status = status;
    document.body.appendChild(el);
    await el.updateComplete;
    return el;
  };
  const primary = (el: Awaited<ReturnType<typeof dialogAt>>) =>
    (el.shadowRoot?.querySelector('.btn-primary')?.textContent ?? '').trim();

  it('ready 是「立即重启」的唯一触发态', async () => {
    document.body.innerHTML = '';
    expect(primary(await dialogAt('ready'))).toBe('立即重启');
  });

  it('答不了与还没查都只给「检查更新」（重试允许，但不等于已装好）', async () => {
    document.body.innerHTML = '';
    for (const status of ['idle', 'checking', 'error'] as const) {
      expect(primary(await dialogAt(status)), `${status} 的主按钮应是检查更新`).toBe('检查更新');
    }
  });

  it('available / downloaded / downloading / installing 都给「开始更新」，进度条只在本轮在飞时出现', async () => {
    document.body.innerHTML = '';
    for (const status of ['available', 'downloaded', 'downloading', 'installing'] as const) {
      const el = await dialogAt(status);
      expect(primary(el), `${status} 的主按钮应是开始更新`).toBe('开始更新');
      const hasProgress = el.shadowRoot?.querySelector('.progress-bar') !== null;
      expect(hasProgress, `${status} 的进度条应与"在飞"一致`).toBe(
        status === 'downloading' || status === 'installing',
      );
    }
  });

  it('词表里每个状态都渲染得出主按钮（不许有落空的分支）', async () => {
    document.body.innerHTML = '';
    for (const status of UPDATER_STATUSES) {
      expect(primary(await dialogAt(status)), `${status} 没有主按钮`).not.toBe('');
    }
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

  // 订阅此前挂在 `firstUpdated`（每个元素实例只跑一次），而 `disconnectedCallback`
  // 会退订——「摘掉再插回」之后组件就与 store 永久脱钩，且没有任何报错。
  it('重连后仍然跟随 store（订阅不得只建在 firstUpdated）', async () => {
    const store = new ShortcutRecorderStore({ current: 'Ctrl+K' });
    const el = document.createElement('oc-shortcut-recorder') as OcShortcutRecorder;
    el.store = store;
    document.body.appendChild(el);
    await el.updateComplete;

    store.setConfig({ current: 'Ctrl+J' });
    store.start();
    expect(el.shortcut, '挂载期间应跟随 store').toBe('Ctrl+J');

    // 摘掉再插回：firstUpdated 不会重跑，只有 connectedCallback 会
    el.remove();
    document.body.appendChild(el);
    await el.updateComplete;

    store.setConfig({ current: 'Ctrl+M' });
    store.stop();
    expect(el.shortcut, '重连后必须仍然跟随 store（否则组件已静默脱钩）').toBe('Ctrl+M');
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

  // 注：happy-dom 派发 click 时会以错误接收者调用 Lit 监听器的 handleEvent
  // （TypeError，见 theme-picker.test.ts 的既有说明），故这里直接驱动组件的
  // `_requestToggle` / `_requestUninstall`（模板 @click 调的就是这两个方法）。
  const P1 = { id: 'p1', name: 'P1', version: '1.0.0', enabled: false, type: 'js' } as const;

  it('点开关派发 oc-plugin-toggle（detail 为取反后的状态）', async () => {
    const el = document.createElement('oc-plugin-manager');
    el.plugins = [{ ...P1 }];
    document.body.appendChild(el);
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.toggle-switch')).toBeTruthy();
    const events: CustomEvent[] = [];
    el.addEventListener('oc-plugin-toggle', (e) => events.push(e as CustomEvent));
    el._requestToggle({ ...P1 });
    expect(events.length).toBe(1);
    expect(events[0]?.detail).toEqual({ id: 'p1', enabled: true });
  });

  it('点卸载派发 oc-plugin-uninstall（此前卸载无入口）', async () => {
    const el = document.createElement('oc-plugin-manager');
    el.plugins = [{ ...P1, enabled: true }];
    document.body.appendChild(el);
    await el.updateComplete;
    expect(el.shadowRoot?.querySelector('.uninstall-btn')).toBeTruthy();
    const events: CustomEvent[] = [];
    el.addEventListener('oc-plugin-uninstall', (e) => events.push(e as CustomEvent));
    el._requestUninstall({ ...P1 });
    expect(events.length).toBe(1);
    expect(events[0]?.detail).toEqual({ id: 'p1' });
  });
});
