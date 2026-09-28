// @vitest-environment happy-dom
// shell-controller.ts 测试（UI 事件 → ShellClient 桥接）

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ShellController } from './shell-controller.js';
import { MockBackend } from './backend.js';
import { AdminClient } from './host.js';
import { ShellClient } from './shell-client.js';
import type { ContributeEntry } from './shell-client.js';
import type { PendingCallInfo } from './events.js';

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
  'host_registry_install',
  'host_registry_install_preview',
  'host_window_create',
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
    await new Promise((r) => setTimeout(r, 10));
    expect(backend.invocations.some((i) => i.cmd === 'host_window_minimize')).toBe(true);
  });

  it('oc-maximize 触发 windowMaximize', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-maximize'));
    await new Promise((r) => setTimeout(r, 10));
    expect(backend.invocations.some((i) => i.cmd === 'host_window_maximize')).toBe(true);
  });

  it('oc-close 触发 windowClose', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-close'));
    await new Promise((r) => setTimeout(r, 10));
    expect(backend.invocations.some((i) => i.cmd === 'host_window_close')).toBe(true);
  });

  it('多个事件目标都可以监听', async () => {
    const el1 = document.createElement('div');
    const el2 = document.createElement('div');
    document.body.appendChild(el1);
    document.body.appendChild(el2);

    controller.start([el1, el2]);
    el1.dispatchEvent(new CustomEvent('oc-minimize'));
    await new Promise((r) => setTimeout(r, 10));
    expect(backend.invocations.some((i) => i.cmd === 'host_window_minimize')).toBe(true);

    controller.stop();
    el1.remove();
    el2.remove();
  });

  // ── 断链回归：三个此前零监听的死按钮 ──

  it('oc-update-start 触发 下载→安装（此前「开始更新」是死按钮）', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-update-start'));
    await new Promise((r) => setTimeout(r, 10));
    const downloadIdx = backend.invocations.findIndex((i) => i.cmd === 'host_market_download');
    const installIdx = backend.invocations.findIndex((i) => i.cmd === 'host_market_install');
    expect(downloadIdx).toBeGreaterThan(-1);
    expect(installIdx, '必须先下载再安装').toBeGreaterThan(downloadIdx);
  });

  it('oc-restart 触发 host_window_relaunch（不是 quit：重启前必须先对账恢复阶段）', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-restart'));
    await new Promise((r) => setTimeout(r, 10));
    expect(
      backend.invocations.some((i) => i.cmd === 'host_window_relaunch'),
      'oc-restart 必须走 host_window_relaunch（先对账、后重启）',
    ).toBe(true);
    // 回归锁：此前这里连的是 host_window_quit —— "重启"只退不重启，且跳过对账。
    expect(
      backend.invocations.some((i) => i.cmd === 'host_window_quit'),
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
    const seen: Array<{ err: unknown; context: string }> = [];
    try {
      const c = new ShellController({
        backend: degradeBackend,
        onError: (err, context) => seen.push({ err, context }),
      });
      c.start([container]);
      container.dispatchEvent(new CustomEvent('oc-restart'));
      await new Promise((r) => setTimeout(r, 10));
      // 降级必须经**用户可见的**错误出口（onError），而不是只打 console。
      const hit = seen.find((s) => s.context === 'window.relaunch');
      expect(hit, 'onError 必须收到 window.relaunch 降级').toBeDefined();
      expect(String((hit!.err as Error).message)).toContain('宿主没有重启原语');
      // 关键：降级时**不动** —— 回退到 quit 会变成"点了重启却直接退出且不再起来"。
      expect(degradeBackend.invocations.some((i) => i.cmd === 'host_window_quit')).toBe(false);
    } finally {
      warn.mockRestore();
    }
  });

  /** 让 host_window_minimize 真的失败（MockBackend 无 case 时视作成功）。 */
  function failingBackend(): MockBackend {
    return new MockBackend({
      capabilities: CONTROLLER_CAPS,
      pluginId: 'p.shell',
      cases: [{ cmd: 'host_window_minimize', error: new Error('boom') }],
    });
  }

  it('命令失败经 onError 上报（不再只 console.warn 静默）', async () => {
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend: failingBackend(),
      onError: (err, context) => seen.push({ err, context }),
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-minimize'));
    await new Promise((r) => setTimeout(r, 10));
    expect(seen.some((s) => s.context === 'window.minimize')).toBe(true);
  });

  it('未传 onError 时回落到 console.warn（保持既有行为，不静默）', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      const c = new ShellController({ backend: failingBackend() });
      c.start([container]);
      container.dispatchEvent(new CustomEvent('oc-minimize'));
      await new Promise((r) => setTimeout(r, 10));
      expect(warn.mock.calls.some((args) => String(args[0]).includes('window.minimize'))).toBe(
        true,
      );
    } finally {
      warn.mockRestore();
    }
  });

  it('oc-plugin-toggle 触发 host_registry_admin enable/disable（此前开关是死按钮）', async () => {
    controller.start([container]);

    container.dispatchEvent(
      new CustomEvent('oc-plugin-toggle', { detail: { id: 'com.a', enabled: true } }),
    );
    await new Promise((r) => setTimeout(r, 10));
    let inv = backend.invocations.find((i) => i.cmd === 'host_registry_admin');
    expect(inv?.args).toEqual({ op: { op: 'enable', id: 'com.a' } });

    container.dispatchEvent(
      new CustomEvent('oc-plugin-toggle', { detail: { id: 'com.a', enabled: false } }),
    );
    await new Promise((r) => setTimeout(r, 10));
    inv = backend.invocations.filter((i) => i.cmd === 'host_registry_admin').pop();
    expect(inv?.args).toEqual({ op: { op: 'disable', id: 'com.a' } });
  });

  it('oc-plugin-uninstall 触发 host_registry_admin uninstall（此前卸载无入口）', async () => {
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-plugin-uninstall', { detail: { id: 'com.a' } }));
    await new Promise((r) => setTimeout(r, 10));
    const inv = backend.invocations.find((i) => i.cmd === 'host_registry_admin');
    expect(inv?.args).toEqual({ op: { op: 'uninstall', id: 'com.a' } });
  });

  it('安装先展示签名包权限，再以完整批准集调用安装', async () => {
    const preview = vi.spyOn(AdminClient.prototype, 'registryInstallPreview').mockResolvedValue({
      pluginId: 'com.install',
      pluginName: 'Install Me',
      version: '1.0.0',
      reviewToken: 'review-token-1',
      packageDigest: 'a'.repeat(64),
      permissions: [
        { permission: 'host:notify', risk: 'low', description: '发送通知', defaultChecked: true },
      ],
    });
    const install = vi.spyOn(AdminClient.prototype, 'registryInstall').mockResolvedValue({
      pluginId: 'com.install',
      version: '1.0.0',
      installPath: '/plugins/com.install',
      approvedPermissions: ['host:notify'],
    });
    const admin = vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue();
    const launch = vi.spyOn(ShellClient.prototype, 'windowCreate').mockResolvedValue({
      created: true,
      label: 'plugin-com.install',
      pluginId: 'com.install',
      reason: null,
    });
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    controller.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-plugin-install', { detail: { packagePath: '/tmp/install.tpkg' } }),
    );
    await new Promise((r) => setTimeout(r, 10));
    expect(preview).toHaveBeenCalledWith('/tmp/install.tpkg');
    expect(confirm).toHaveBeenCalledOnce();
    expect(install).toHaveBeenCalledWith(
      '/tmp/install.tpkg',
      ['host:notify'],
      'review-token-1',
    );
    expect(admin).toHaveBeenCalledWith({ op: 'enable', id: 'com.install' });
    expect(launch).toHaveBeenCalledWith('com.install');
    expect(alert).toHaveBeenCalledOnce();
  });

  it('拒绝任一声明权限时不调用安装', async () => {
    vi.spyOn(AdminClient.prototype, 'registryInstallPreview').mockResolvedValue({
      pluginId: 'com.install',
      pluginName: 'Install Me',
      version: '1.0.0',
      permissions: [
        { permission: 'host:notify', risk: 'low', description: '发送通知', defaultChecked: true },
      ],
    });
    const install = vi.spyOn(AdminClient.prototype, 'registryInstall');
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    controller.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-plugin-install', { detail: { packagePath: '/tmp/install.tpkg' } }),
    );
    await new Promise((r) => setTimeout(r, 10));
    expect(install).not.toHaveBeenCalled();
    expect(
      backend.invocations.some((invocation) => invocation.cmd === 'host_registry_install'),
    ).toBe(false);
  });

  it('宿主未启用 plugin-install 特性时明确拒绝安装（不发注定 command not found 的 invoke）', async () => {
    // 默认构建下 `host_registry_install*` 是 feature-gated 的——能力表说没有，
    // 就必须在客户端拒绝，而不是把"命令不存在"甩给用户。
    const noInstall = new MockBackend({
      capabilities: CONTROLLER_CAPS.filter((c) => !c.startsWith('host_registry_install')),
      pluginId: 'p.shell',
    });
    const preview = vi.spyOn(AdminClient.prototype, 'registryInstallPreview');
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend: noInstall,
      onError: (err, context) => seen.push({ err, context }),
    });
    c.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-plugin-install', { detail: { packagePath: '/tmp/install.tpkg' } }),
    );
    await new Promise((r) => setTimeout(r, 10));

    expect(preview, '能力缺失时不得进入预览流程').not.toHaveBeenCalled();
    expect(alert, '不得弹出"已安装"的假成功提示').not.toHaveBeenCalled();
    expect(
      noInstall.invocations.some((i) => i.cmd.startsWith('host_registry_install')),
      '不得发出注定失败的 invoke',
    ).toBe(false);
    const hit = seen.find((s) => s.context === 'plugin.install');
    expect(hit, '拒绝必须经 onError 上报').toBeDefined();
    expect(String((hit!.err as Error).message)).toContain('plugin-install');
  });

  // ── onRegistryChange：改完注册表必须让接入方重取列表 ────────────────
  //
  // `<oc-plugin-manager>` 的列表由接入方喂（`plugins` 属性）。控制器改完注册表
  // 若不通知，开关拨了、列表还是旧的——用户看到「点了没反应」。

  it('oc-plugin-toggle 成功后触发 onRegistryChange（列表才会刷新）', async () => {
    vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue();
    let calls = 0;
    const c = new ShellController({
      backend,
      onRegistryChange: () => {
        calls += 1;
      },
    });
    const local = document.createElement('div');
    document.body.appendChild(local);
    c.start([local]);
    local.dispatchEvent(
      new CustomEvent('oc-plugin-toggle', { detail: { id: 'com.a', enabled: true } }),
    );
    await new Promise((r) => setTimeout(r, 10));
    expect(calls).toBe(1);
    c.stop();
    local.remove();
  });

  it('oc-plugin-uninstall 成功后触发 onRegistryChange', async () => {
    vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue();
    let calls = 0;
    const c = new ShellController({
      backend,
      onRegistryChange: () => {
        calls += 1;
      },
    });
    const local = document.createElement('div');
    document.body.appendChild(local);
    c.start([local]);
    local.dispatchEvent(new CustomEvent('oc-plugin-uninstall', { detail: { id: 'com.a' } }));
    await new Promise((r) => setTimeout(r, 10));
    expect(calls).toBe(1);
    c.stop();
    local.remove();
  });

  it('onRegistryChange 回调抛错经 onError 上报（不静默吞掉）', async () => {
    vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue();
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend,
      onError: (err, context) => seen.push({ err, context }),
      onRegistryChange: () => {
        throw new Error('list reload failed');
      },
    });
    const local = document.createElement('div');
    document.body.appendChild(local);
    c.start([local]);
    local.dispatchEvent(
      new CustomEvent('oc-plugin-toggle', { detail: { id: 'com.a', enabled: false } }),
    );
    await new Promise((r) => setTimeout(r, 10));
    const hit = seen.find((s) => s.context === 'registry.refresh');
    expect(hit).toBeDefined();
    expect(String((hit!.err as Error).message)).toContain('list reload failed');
    c.stop();
    local.remove();
  });

  // ── 命令面板：跨主体调用取件必须**轮询** ──────────────────────────
  //
  // `callTakeResult` 是一次性取件语义：`settled` 取走即删，`pending` 返回副本、
  // 条目保留（「还没好，可稍后再取」）。只取一次就把 `pending` 判成失败，会把
  // **正常的异步投递**误报为错误——跨 webview 的执行总要几毫秒才回填。

  /** 造一条 `PendingCallInfo`（本组只关心 state / errorCode）。 */
  function callInfo(state: 'pending' | 'settled', errorCode?: string): PendingCallInfo {
    return {
      callId: 'c-1',
      pluginId: 'com.fmt',
      cmd: 'fmt.run',
      args: null,
      seq: 1,
      createdAt: 0,
      expiresAt: 0,
      state,
      ...(errorCode !== undefined ? { errorCode } : {}),
    };
  }

  const FMT_ENTRY: ContributeEntry = {
    pluginId: 'com.fmt',
    kind: 'command',
    id: 'fmt.run',
    label: '格式化',
  };

  it('oc-command-select：投递后轮询取件直到结算（pending 不是失败）', async () => {
    const list = vi.spyOn(ShellClient.prototype, 'contributesList').mockResolvedValue([FMT_ENTRY]);
    const call = vi
      .spyOn(ShellClient.prototype, 'callPlugin')
      .mockResolvedValue(callInfo('pending'));
    let n = 0;
    const take = vi.spyOn(ShellClient.prototype, 'callTakeResult').mockImplementation(async () => {
      n += 1;
      return n < 3 ? callInfo('pending') : callInfo('settled');
    });
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend,
      onError: (err, context) => seen.push({ err, context }),
      commandResultBudget: { attempts: 5, intervalMs: 1 },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-command-select', { detail: { id: 'fmt.run' } }));
    await vi.waitFor(() => expect(take.mock.calls.length).toBeGreaterThanOrEqual(3));

    expect(list).toHaveBeenCalledWith('command');
    expect(call).toHaveBeenCalledWith('com.fmt', 'fmt.run');
    expect(take.mock.calls.length, '必须轮询取件，不能只取一次').toBeGreaterThanOrEqual(3);
    expect(seen, 'pending 不是失败，不得上报错误').toEqual([]);
  });

  it('oc-command-select：预算耗尽仍 pending → 如实报取件超时（不静默）', async () => {
    vi.spyOn(ShellClient.prototype, 'contributesList').mockResolvedValue([FMT_ENTRY]);
    vi.spyOn(ShellClient.prototype, 'callPlugin').mockResolvedValue(callInfo('pending'));
    const take = vi
      .spyOn(ShellClient.prototype, 'callTakeResult')
      .mockImplementation(async () => callInfo('pending'));
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend,
      onError: (err, context) => seen.push({ err, context }),
      commandResultBudget: { attempts: 3, intervalMs: 1 },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-command-select', { detail: { id: 'fmt.run' } }));
    await vi.waitFor(() => expect(seen.some((s) => s.context === 'command.select')).toBe(true));

    expect(take.mock.calls.length, '必须把预算用满').toBe(3);
    const hit = seen.find((s) => s.context === 'command.select');
    expect(hit, '超时必须经 onError 上报').toBeDefined();
    expect(String((hit!.err as Error).message)).toContain('取件超时');
  });

  it('oc-command-select：结算但带 errorCode → 如实报执行失败', async () => {
    vi.spyOn(ShellClient.prototype, 'contributesList').mockResolvedValue([FMT_ENTRY]);
    vi.spyOn(ShellClient.prototype, 'callPlugin').mockResolvedValue(callInfo('pending'));
    vi.spyOn(ShellClient.prototype, 'callTakeResult').mockResolvedValue(
      callInfo('settled', 'E_PLUGIN_TYPE_NO_RUNTIME'),
    );
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend,
      onError: (err, context) => seen.push({ err, context }),
      commandResultBudget: { attempts: 3, intervalMs: 1 },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-command-select', { detail: { id: 'fmt.run' } }));
    await vi.waitFor(() => expect(seen.some((s) => s.context === 'command.select')).toBe(true));

    const hit = seen.find((s) => s.context === 'command.select');
    expect(hit).toBeDefined();
    expect(String((hit!.err as Error).message)).toContain('E_PLUGIN_TYPE_NO_RUNTIME');
  });

  it('oc-command-select：找不到归属 → 如实报错且不投递（不静默丢弃）', async () => {
    vi.spyOn(ShellClient.prototype, 'contributesList').mockResolvedValue([]);
    const call = vi.spyOn(ShellClient.prototype, 'callPlugin');
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend,
      onError: (err, context) => seen.push({ err, context }),
      commandResultBudget: { attempts: 3, intervalMs: 1 },
    });
    c.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-command-select', { detail: { id: 'builtin.quit' } }),
    );
    await vi.waitFor(() => expect(seen.some((s) => s.context === 'command.select')).toBe(true));

    expect(call, '找不到归属时不得投递').not.toHaveBeenCalled();
    const hit = seen.find((s) => s.context === 'command.select');
    expect(hit).toBeDefined();
    expect(String((hit!.err as Error).message)).toContain('不是任何插件的贡献命令');
  });
});
