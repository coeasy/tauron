// @vitest-environment happy-dom
// shell-controller.ts 测试（UI 事件 → ShellClient 桥接）

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ShellController } from './shell-controller.js';
import { MockBackend } from './backend.js';

/** 控制器用到的命令面（与 `ShellClient` 的调用点一一对应）。 */
const CONTROLLER_CAPS = [
  'host_window_minimize',
  'host_window_maximize',
  'host_window_close',
  'host_window_quit',
  'host_window_relaunch',
  'host_market_check',
  'host_market_download',
  'host_market_install',
  'host_registry_admin',
];

describe('ShellController', () => {
  let backend: MockBackend;
  let controller: ShellController;
  let container: HTMLElement;

  beforeEach(() => {
    backend = new MockBackend({
      capabilities: CONTROLLER_CAPS,
      pluginId: 'p.shell',
      // 真 Tauri 宿主的形状：对账完成 + 已请求重启（`relaunch()` 是发散函数，
      // 调用方通常读不到返回值，但线形上它是 `true`）。
      cases: [
        {
          cmd: 'host_window_relaunch',
          result: {
            reconcile: { scanned: 2, entered: 1, exited: 0, ignored: 0 },
            relaunchRequested: true,
            reason: null,
          },
        },
      ],
    });
    controller = new ShellController({ backend });
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    controller.stop();
    container.remove();
  });

  it('start() 注册事件监听', () => {
    controller.start([container]);
    expect(controller).toBeDefined();
  });

  it('stop() 移除所有事件监听', () => {
    controller.start([container]);
    controller.stop();
    // 再次派发不应触发调用
    container.dispatchEvent(new CustomEvent('oc-minimize'));
    expect(backend.invocations.length).toBe(0);
  });

  it('oc-minimize 触发 windowMinimize', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-minimize'));
    // 等待微任务完成
    await new Promise(r => setTimeout(r, 10));
    expect(backend.invocations.some(i => i.cmd === 'host_window_minimize')).toBe(true);
  });

  it('oc-maximize 触发 windowMaximize', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-maximize'));
    await new Promise(r => setTimeout(r, 10));
    expect(backend.invocations.some(i => i.cmd === 'host_window_maximize')).toBe(true);
  });

  it('oc-close 触发 windowClose', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-close'));
    await new Promise(r => setTimeout(r, 10));
    expect(backend.invocations.some(i => i.cmd === 'host_window_close')).toBe(true);
  });

  it('多个事件目标都可以监听', async () => {
    const el1 = document.createElement('div');
    const el2 = document.createElement('div');
    document.body.appendChild(el1);
    document.body.appendChild(el2);

    controller.start([el1, el2]);
    el1.dispatchEvent(new CustomEvent('oc-minimize'));
    await new Promise(r => setTimeout(r, 10));
    expect(backend.invocations.some(i => i.cmd === 'host_window_minimize')).toBe(true);

    controller.stop();
    el1.remove();
    el2.remove();
  });

  // ── 断链回归：三个此前零监听的死按钮 ──

  it('oc-update-start 触发 下载→安装（此前「开始更新」是死按钮）', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-update-start'));
    await new Promise(r => setTimeout(r, 10));
    const downloadIdx = backend.invocations.findIndex(i => i.cmd === 'host_market_download');
    const installIdx = backend.invocations.findIndex(i => i.cmd === 'host_market_install');
    expect(downloadIdx).toBeGreaterThan(-1);
    expect(installIdx, '必须先下载再安装').toBeGreaterThan(downloadIdx);
  });

  it('oc-restart 触发 host_window_relaunch（不是 quit：重启前必须先对账恢复阶段）', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-restart'));
    await new Promise(r => setTimeout(r, 10));
    expect(
      backend.invocations.some(i => i.cmd === 'host_window_relaunch'),
      'oc-restart 必须走 host_window_relaunch（先对账、后重启）',
    ).toBe(true);
    // 回归锁：此前这里连的是 host_window_quit —— "重启"只退不重启，且跳过对账。
    expect(
      backend.invocations.some(i => i.cmd === 'host_window_quit'),
      'oc-restart 不得再退化为 quit',
    ).toBe(false);
  });

  it('oc-restart 在宿主无重启原语时如实告警，且**不**回退到 quit', async () => {
    const degradeBackend = new MockBackend({
      capabilities: CONTROLLER_CAPS,
      pluginId: 'p.shell',
      cases: [
        {
          cmd: 'host_window_relaunch',
          result: {
            reconcile: { scanned: 0, entered: 0, exited: 0, ignored: 0 },
            relaunchRequested: false,
            reason: '宿主没有重启原语',
          },
        },
      ],
    });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      const c = new ShellController({ backend: degradeBackend });
      c.start([container]);
      container.dispatchEvent(new CustomEvent('oc-restart'));
      await new Promise(r => setTimeout(r, 10));
      expect(warn.mock.calls.some(args => String(args[0]).includes('宿主没有重启原语'))).toBe(true);
      // 关键：降级时**不动** —— 回退到 quit 会变成"点了重启却直接退出且不再起来"。
      expect(degradeBackend.invocations.some(i => i.cmd === 'host_window_quit')).toBe(false);
    } finally {
      warn.mockRestore();
    }
  });

  it('oc-plugin-toggle 触发 host_registry_admin enable/disable（此前开关是死按钮）', async () => {
    controller.start([container]);

    container.dispatchEvent(
      new CustomEvent('oc-plugin-toggle', { detail: { id: 'com.a', enabled: true } }),
    );
    await new Promise(r => setTimeout(r, 10));
    let inv = backend.invocations.find(i => i.cmd === 'host_registry_admin');
    expect(inv?.args).toEqual({ op: { op: 'enable', id: 'com.a' } });

    container.dispatchEvent(
      new CustomEvent('oc-plugin-toggle', { detail: { id: 'com.a', enabled: false } }),
    );
    await new Promise(r => setTimeout(r, 10));
    inv = backend.invocations.filter(i => i.cmd === 'host_registry_admin').pop();
    expect(inv?.args).toEqual({ op: { op: 'disable', id: 'com.a' } });
  });
});