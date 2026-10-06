// @vitest-environment happy-dom
// shell-controller.ts 测试（UI 事件 → ShellClient 桥接）

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ShellController } from './shell-controller.js';
import { MockBackend, type Backend } from './backend.js';
import { NOTIFICATION_TOPIC } from './host-topics.js';
import { AdminClient } from './host.js';
import { ShellClient } from './shell-client.js';
import type { ContributeEntry, NotificationsListResult } from './shell-client.js';
import type { AdminReviewToken, PendingCallInfo, RegistryAdminOutcome } from './events.js';
import type { ApprovalRow } from './grants.js';

/**
 * 控制器用到的命令面（与 `ShellClient` 的调用点一一对应）。
 *
 * 轮 31 起 **不含** `host_market_check`：检查腿改打 `host_updater_check`，这条桩命令
 * 对本控制器就是「不该再被调用」。留着声明它等于给回归留后门（mock 会照样答
 * `available: false` 让测试变绿），删掉之后一旦有人把那一腿改回桩，mock 直接
 * `command not found` 判红。
 */
const CONTROLLER_CAPS = [
  'host_window_minimize',
  'host_window_maximize',
  'host_window_close',
  'host_window_quit',
  'host_window_relaunch',
  'host_updater_check',
  'host_market_download',
  'host_market_install',
  'host_registry_admin',
  'host_registry_install',
  'host_registry_install_preview',
  'host_window_create',
];

const REVIEW_TOKEN = {
  packageDigest: 'a'.repeat(64),
  manifestDigest: 'b'.repeat(64),
  permissionDigest: 'c'.repeat(64),
  keyId: 'publisher-key-1',
  publisherId: 'publisher.example',
  pluginId: 'com.install',
  version: '1.0.0',
  issuedAt: 1,
  expiresAt: 9999999999,
  nonce: 'review-1',
};

/** 轮 43（A83）：破坏性操作的审批令牌（Rust `AdminReviewToken` 的线形）。 */
const UNINSTALL_TOKEN: AdminReviewToken = {
  pluginId: 'com.a',
  op: 'uninstall',
  version: '2.3.4',
  issuedAt: 1,
  expiresAt: 9999999999,
  nonce: 'admin-review-1',
};

/** 轮 43：预览返回（`RegistryAdminResponse::Review` 的线形）。 */
function reviewOutcome(): RegistryAdminOutcome {
  return {
    kind: 'review',
    op: 'uninstall',
    pluginId: 'com.a',
    version: '2.3.4',
    state: 'RUNNING',
    reviewToken: UNINSTALL_TOKEN,
  };
}

/** 轮 43：提交返回（`RegistryAdminResponse::Executed` 的线形）。 */
function executedOutcome(): RegistryAdminOutcome {
  return {
    kind: 'executed',
    event: 'UNINSTALL',
    from: 'RUNNING',
    to: 'UNINSTALLED',
    depth: 1,
    illegal: false,
    actions: [],
  };
}

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

  // ── 轮 31：「检查更新」按钮必须打**真更新通道**，且答案要有人读 ──
  //
  // 这一腿此前调 `host_market_check`（宿主硬编码桩，恒 `available: false` +
  // `simulated: true`）**并把返回值丢掉**：宿主注入真端点后，SDK 侧（轮 29 已改读真
  // 通道）报得出 `UpdateAvailable`，对话框却永远显示"没有更新"。同一次用户动作两个
  // 相反答案，就是轮 29 那条断链的上移一层。

  /** 真通道 `host_updater_check` 的线上形状（Rust `UpdaterCheckOutcome`）。 */
  const updaterOutcome = (over: Record<string, unknown> = {}) => ({
    available: false,
    version: null,
    url: null,
    releasedAt: null,
    degraded: false,
    reason: null,
    ...over,
  });

  const updaterBackend = (result: unknown): MockBackend =>
    new MockBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_updater_check'],
      cases: [{ cmd: 'host_updater_check', result }],
    });

  it('oc-updater-check 打 host_updater_check 并把 currentVersion 作入参，不打宿主桩', async () => {
    const b = updaterBackend(updaterOutcome());
    const c = new ShellController({ backend: b, currentVersion: '1.4.2' });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    const hit = b.invocations.find((i) => i.cmd === 'host_updater_check');
    expect(hit, '检查腿必须打真通道 host_updater_check').toBeDefined();
    expect(hit?.args).toEqual({ currentVersion: '1.4.2' });
    expect(
      b.invocations.some((i) => i.cmd === 'host_market_check'),
      '不得回落 host_market_check（那是恒 available:false 的桩）',
    ).toBe(false);
    c.stop();
  });

  it('缺 currentVersion 时什么都不调并经 onError 报缺参，而不是拿桩冒充"已是最新"', async () => {
    const b = updaterBackend(updaterOutcome());
    const errors: Array<[unknown, string]> = [];
    const c = new ShellController({
      backend: b,
      onError: (err, context) => errors.push([err, context]),
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    expect(b.invocations, '缺必填入参就不该发出任何命令').toEqual([]);
    expect(errors.map(([, ctx]) => ctx)).toEqual(['updater.check']);
    expect(String(errors[0]?.[0])).toContain('currentVersion');
    c.stop();
  });

  it('检查结果交给 onUpdaterCheck：available 与 degraded 是两种读数，不得合并', async () => {
    const seen: unknown[] = [];
    const b = updaterBackend(updaterOutcome({ available: true, version: '2.0.0' }));
    const c = new ShellController({
      backend: b,
      currentVersion: '1.0.0',
      onUpdaterCheck: (info) => {
        seen.push(info);
      },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ available: true, version: '2.0.0', degraded: false });

    // 端点未注入（`Unsupported`）：available 仍是 false，但 degraded 必须为真——
    // 它与"确实没有更新"共用一个 available:false，UI 只有靠 degraded 才分得开。
    const b2 = new MockBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_updater_check'],
      cases: [
        {
          cmd: 'host_updater_check',
          result: { supported: false, reason: '宿主未注入更新端点', fallback: null },
        },
      ],
    });
    const seen2: unknown[] = [];
    const c2 = new ShellController({
      backend: b2,
      currentVersion: '1.0.0',
      onUpdaterCheck: (info) => {
        seen2.push(info);
      },
    });
    c2.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    expect(seen2).toHaveLength(1);
    expect(seen2[0]).toMatchObject({ available: false, degraded: true });
    c.stop();
    c2.stop();
  });

  it('真通道返回空结果不当作"没有更新"：经 onError 如实报答不了', async () => {
    // MockBackend 对「已注册但无预置行为」返回 undefined，正是"宿主答了个空"。
    const b = updaterBackend(undefined);
    const errors: string[] = [];
    const c = new ShellController({
      backend: b,
      currentVersion: '1.0.0',
      onError: (err, context) => errors.push(`${context}:${(err as Error).message}`),
      onUpdaterCheck: () => {
        errors.push('onUpdaterCheck 不该被调用');
      },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    expect(errors).toEqual([
      'updater.check:host_updater_check 返回空结果：无法判定可用性，不当作"没有更新"',
    ]);
    c.stop();
  });

  // ── 轮 33：「没有更新」要带**为什么**，账本的模拟推进要说出口 ──
  //
  // `host_updater_status` 是灰度百分比 / 崩溃门禁 / 进程内账本的唯一出口，此前在 TS 侧
  // 零消费者：于是「你不在灰度批次」「更新被崩溃门禁停发」「宿主没装端点」在 UI 上是
  // 同一句含糊的「没有更新」。而账本的两个写入方今天都是宿主桩，`state` 字符串自己
  // 分不出真假——只看 `state` 会把「点了一下模拟安装」显示成「已安装 2.0.0」。

  /** 真通道 `host_updater_status` 的线上形状（Rust `UpdaterStatus`）。 */
  const channelStatus = (over: Record<string, unknown> = {}) => ({
    available: true,
    state: null,
    stateSimulated: false,
    grayscalePercent: 100,
    crashGateStopped: false,
    reason: null,
    ...over,
  });

  /** 检查腿 + 诊断腿都有答复的后端。 */
  const diagnosisBackend = (check: unknown, channel: unknown): MockBackend =>
    new MockBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_updater_check', 'host_updater_status'],
      cases: [
        { cmd: 'host_updater_check', result: check },
        { cmd: 'host_updater_status', result: channel },
      ],
    });

  /** 派发一次「检查更新」，取回 onUpdaterCheck 收到的 info。 */
  async function runCheck(b: MockBackend): Promise<Record<string, unknown> | undefined> {
    const seen: Array<Record<string, unknown>> = [];
    const c = new ShellController({
      backend: b,
      currentVersion: '1.0.0',
      onUpdaterCheck: (info) => {
        seen.push(info as unknown as Record<string, unknown>);
      },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    c.stop();
    return seen[0];
  }

  it('答「没有更新」时把通道诊断拼进 reason：灰度/崩溃门禁不再是隐形事实', async () => {
    const b = diagnosisBackend(
      updaterOutcome({ reason: '不在灰度批次' }),
      channelStatus({ grayscalePercent: 30 }),
    );
    const info = await runCheck(b);
    expect(String(info?.reason)).toContain('灰度 30%');
    expect(String(info?.reason)).toContain('崩溃门禁未停发');
    expect(String(info?.reason)).toContain('不在灰度批次');
    // 原有 reason 不能被诊断句顶掉：那句才是"为什么没更新"的主答案。
    expect(info?.reason).toBe('更新通道已装配｜灰度 30%｜崩溃门禁未停发｜不在灰度批次');
    // 账本为空时不凭空造一句「宿主账本 null」。
    expect(String(info?.reason)).not.toContain('宿主账本');
  });

  it('degraded 走同一出口：端点未装配要在 reason 里看得见，而不是只剩「答不了」', async () => {
    const b = diagnosisBackend(
      { supported: false, reason: '宿主未注入更新端点', fallback: null },
      channelStatus({ available: false, reason: '等待宿主注入 EndpointClient' }),
    );
    const info = await runCheck(b);
    expect(info).toMatchObject({ available: false, degraded: true });
    expect(String(info?.reason)).toContain('更新通道未装配');
    expect(String(info?.reason)).toContain('宿主未注入更新端点');
    expect(String(info?.reason)).toContain('等待宿主注入 EndpointClient');
  });

  it('宿主账本的模拟推进必须带限定语，stateSimulated:false 时不加', async () => {
    const simulated = await runCheck(
      diagnosisBackend(
        updaterOutcome(),
        channelStatus({ state: 'installed:2.0.0', stateSimulated: true }),
      ),
    );
    expect(String(simulated?.reason)).toContain('宿主账本 installed:2.0.0（模拟推进，未真的装上）');

    const real = await runCheck(
      diagnosisBackend(updaterOutcome(), channelStatus({ state: 'installed:2.0.0' })),
    );
    expect(String(real?.reason)).toContain('宿主账本 installed:2.0.0');
    expect(String(real?.reason)).not.toContain('模拟推进');
  });

  it('有更新时不问诊断腿：诊断只在「答不了 / 没更新」这两条分支上跑', async () => {
    const b = diagnosisBackend(
      updaterOutcome({ available: true, version: '2.0.0' }),
      channelStatus(),
    );
    const info = await runCheck(b);
    expect(info).toMatchObject({ available: true, version: '2.0.0' });
    expect(
      b.invocations.some((i) => i.cmd === 'host_updater_status'),
      '有更新却问通道状态，等于把一次点击变成两条命令',
    ).toBe(false);
  });

  it('诊断命令失败/缺席都不掩盖检查结论，也不冒充成一句诊断', async () => {
    // ① 命令未注册（旧宿主）：mock 抛 command not found。
    const missing = new MockBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_updater_check'],
      cases: [{ cmd: 'host_updater_check', result: updaterOutcome({ reason: '已是最新版本' }) }],
    });
    const info1 = await runCheck(missing);
    expect(info1).toMatchObject({ available: false, degraded: false, reason: '已是最新版本' });

    // ② 命令在但答错。
    const failing = new MockBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_updater_check', 'host_updater_status'],
      cases: [
        { cmd: 'host_updater_check', result: updaterOutcome({ reason: '已是最新版本' }) },
        { cmd: 'host_updater_status', error: new Error('boom') },
      ],
    });
    const info2 = await runCheck(failing);
    expect(info2).toMatchObject({ available: false, reason: '已是最新版本' });
    // 空结果（宿主答了个空）同样不能读成"通道已装配、灰度 0%"。
    const empty = diagnosisBackend(updaterOutcome({ reason: '已是最新版本' }), undefined);
    const info3 = await runCheck(empty);
    expect(info3).toMatchObject({ available: false, reason: '已是最新版本' });
  });

  it('不传 onUpdaterCheck 时不发诊断：没有读者就不多打一条命令', async () => {
    const b = diagnosisBackend(updaterOutcome(), channelStatus());
    const c = new ShellController({ backend: b, currentVersion: '1.0.0' });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-updater-check'));
    await new Promise((r) => setTimeout(r, 10));
    expect(b.invocations.map((i) => i.cmd)).toEqual(['host_updater_check']);
    c.stop();
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

  it('oc-plugin-uninstall 两步链：预览（无副作用）→ 确认 → 以令牌提交（轮 43 / A83）', async () => {
    const admin = vi
      .spyOn(AdminClient.prototype, 'registryAdmin')
      .mockResolvedValueOnce(reviewOutcome())
      .mockResolvedValueOnce(executedOutcome());
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-plugin-uninstall', { detail: { id: 'com.a' } }));
    await new Promise((r) => setTimeout(r, 10));
    // 第一步是预览：`preview: true`，宿主此时不改任何状态。
    expect(admin).toHaveBeenNthCalledWith(1, { op: 'uninstall', id: 'com.a', preview: true });
    // 预览事实（id + 版本）必须呈现给用户后才可能提交。
    expect(confirm.mock.calls[0]?.[0]).toContain('com.a');
    expect(confirm.mock.calls[0]?.[0]).toContain('2.3.4');
    // 第二步才是提交：原样带回一次性令牌（宿主在动作前重核 id/操作/版本）。
    expect(admin).toHaveBeenNthCalledWith(2, {
      op: 'uninstall',
      id: 'com.a',
      reviewToken: UNINSTALL_TOKEN,
    });
  });

  it('oc-plugin-uninstall 用户拒绝时不提交（令牌弃用，不通知列表刷新）', async () => {
    const admin = vi
      .spyOn(AdminClient.prototype, 'registryAdmin')
      .mockResolvedValueOnce(reviewOutcome());
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    let changes = 0;
    const c = new ShellController({
      backend,
      onRegistryChange: () => {
        changes += 1;
      },
    });
    c.start([container]);
    container.dispatchEvent(new CustomEvent('oc-plugin-uninstall', { detail: { id: 'com.a' } }));
    await new Promise((r) => setTimeout(r, 10));
    expect(confirm).toHaveBeenCalledOnce();
    expect(admin, '拒绝后不得有提交调用').toHaveBeenCalledOnce();
    expect(changes, '未执行 → 不触发注册表变更通知').toBe(0);
    c.stop();
  });

  it('老宿主（预览即执行、无 review 判别）不重复提交', async () => {
    // 旧版宿主的返回是裸 TransitionOutcome（没有 `kind` 判别）：第一步已直接执行。
    const admin = vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValueOnce({
      event: 'UNINSTALL',
      from: 'RUNNING',
      to: 'UNINSTALLED',
      depth: 1,
      illegal: false,
      actions: [],
    } as unknown as RegistryAdminOutcome);
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    controller.start([container]);
    container.dispatchEvent(new CustomEvent('oc-plugin-uninstall', { detail: { id: 'com.a' } }));
    await new Promise((r) => setTimeout(r, 10));
    expect(admin, '老宿主在预览步已执行，不再提交第二次').toHaveBeenCalledOnce();
    expect(confirm, '老宿主无预览事实可确认，不应弹框').not.toHaveBeenCalled();
  });

  it('安装先展示签名包权限，再以完整批准集调用安装', async () => {
    const preview = vi.spyOn(AdminClient.prototype, 'registryInstallPreview').mockResolvedValue({
      pluginId: 'com.install',
      pluginName: 'Install Me',
      version: '1.0.0',
      permissions: [
        { permission: 'host:notify', risk: 'low', description: '发送通知', defaultChecked: true },
      ],
      reviewToken: REVIEW_TOKEN,
    });
    const install = vi.spyOn(AdminClient.prototype, 'registryInstall').mockResolvedValue({
      pluginId: 'com.install',
      version: '1.0.0',
      installPath: '/plugins/com.install',
      approvedPermissions: ['host:notify'],
    });
    const admin = vi
      .spyOn(AdminClient.prototype, 'registryAdmin')
      .mockResolvedValue(executedOutcome());
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
    expect(install).toHaveBeenCalledWith('/tmp/install.tpkg', ['host:notify'], REVIEW_TOKEN);
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
      reviewToken: REVIEW_TOKEN,
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

  // 轮 55 把审批行的判定收到宿主单源（`tauron_acl::build_approval_rows`），并把
  // `defaultChecked` / `scope` / `confirmationHint` 送上线；本轮要求壳层**真的读它们**——
  // 否则宿主的高危判定在最后一米被丢掉，前端链路仍是断的。
  const MIXED_ROWS: ApprovalRow[] = [
    { permission: 'host:notify', risk: 'low', description: '发送通知', defaultChecked: true },
    {
      permission: 'fs:allow-remove',
      risk: 'high',
      description: '删除应用数据目录内的文件',
      defaultChecked: false,
      scope: '$APPDATA/sub/**',
      confirmationHint: '我理解该权限的能力边界并显式批准',
    },
  ];

  it('轮 56：宿主判为高危的行必须逐字输入确认词，输入不符即中止且不提交缺项批准集', async () => {
    vi.spyOn(AdminClient.prototype, 'registryInstallPreview').mockResolvedValue({
      pluginId: 'com.install',
      pluginName: 'Install Me',
      version: '1.0.0',
      permissions: MIXED_ROWS,
      reviewToken: REVIEW_TOKEN,
    });
    const install = vi.spyOn(AdminClient.prototype, 'registryInstall');
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const prompt = vi.spyOn(window, 'prompt').mockReturnValue('我理解该权限的能力边界');
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    controller.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-plugin-install', { detail: { packagePath: '/tmp/install.tpkg' } }),
    );
    await new Promise((r) => setTimeout(r, 10));

    expect(confirm, '默认勾的行仍是一问一答').toHaveBeenCalledOnce();
    expect(prompt, '默认不勾的行必须问宿主给的确认词').toHaveBeenCalledOnce();
    const asked = String(prompt.mock.calls[0]?.[0]);
    expect(asked, '宿主给的 scope 必须显示出来').toContain('$APPDATA/sub/**');
    expect(asked, '确认词逐字来自宿主').toContain('我理解该权限的能力边界并显式批准');
    expect(install, '不得把缺一项的批准集交给宿主冒充成功').not.toHaveBeenCalled();
    expect(alert, '不一致要看得见，不能静默返回').toHaveBeenCalledOnce();
  });

  it('轮 56：逐字输入确认后批准集完整交给宿主（两条都不丢）', async () => {
    vi.spyOn(AdminClient.prototype, 'registryInstallPreview').mockResolvedValue({
      pluginId: 'com.install',
      pluginName: 'Install Me',
      version: '1.0.0',
      permissions: MIXED_ROWS,
      reviewToken: REVIEW_TOKEN,
    });
    const install = vi.spyOn(AdminClient.prototype, 'registryInstall').mockResolvedValue({
      pluginId: 'com.install',
      version: '1.0.0',
      installPath: '/plugins/com.install',
      approvedPermissions: ['host:notify', 'fs:allow-remove'],
    });
    vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue(executedOutcome());
    vi.spyOn(ShellClient.prototype, 'windowCreate').mockResolvedValue({
      created: true,
      label: 'plugin-com.install',
      pluginId: 'com.install',
      reason: null,
    });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const prompt = vi.spyOn(window, 'prompt').mockReturnValue('我理解该权限的能力边界并显式批准');
    vi.spyOn(window, 'alert').mockImplementation(() => {});
    controller.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-plugin-install', { detail: { packagePath: '/tmp/install.tpkg' } }),
    );
    await new Promise((r) => setTimeout(r, 10));

    expect(prompt).toHaveBeenCalledOnce();
    expect(install).toHaveBeenCalledWith(
      '/tmp/install.tpkg',
      ['host:notify', 'fs:allow-remove'],
      REVIEW_TOKEN,
    );
  });

  it('轮 56：宿主没给确认词时具名拒绝，壳层不代为拟定文案', async () => {
    vi.spyOn(AdminClient.prototype, 'registryInstallPreview').mockResolvedValue({
      pluginId: 'com.install',
      pluginName: 'Install Me',
      version: '1.0.0',
      permissions: [
        {
          permission: 'fs:allow-remove',
          risk: 'high',
          description: '删除应用数据目录内的文件',
          defaultChecked: false,
        },
      ],
      reviewToken: REVIEW_TOKEN,
    });
    const install = vi.spyOn(AdminClient.prototype, 'registryInstall');
    const prompt = vi.spyOn(window, 'prompt').mockReturnValue('我随便写的');
    const seen: Array<{ err: unknown; context: string }> = [];
    const c = new ShellController({
      backend,
      onError: (err, context) => seen.push({ err, context }),
    });
    c.start([container]);
    container.dispatchEvent(
      new CustomEvent('oc-plugin-install', { detail: { packagePath: '/tmp/install.tpkg' } }),
    );
    await new Promise((r) => setTimeout(r, 10));

    expect(prompt, '没有宿主文案就不该问一个本地编出来的确认词').not.toHaveBeenCalled();
    expect(install).not.toHaveBeenCalled();
    const hit = seen.find((s) => s.context === 'plugin.install');
    expect(hit, '契约缺口必须经 onError 具名上报').toBeDefined();
    expect(String((hit!.err as Error).message)).toContain('confirmationHint');
    c.stop();
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
    vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue(executedOutcome());
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
    vi.spyOn(AdminClient.prototype, 'registryAdmin')
      .mockResolvedValueOnce(reviewOutcome())
      .mockResolvedValueOnce(executedOutcome());
    vi.spyOn(window, 'confirm').mockReturnValue(true);
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
    vi.spyOn(AdminClient.prototype, 'registryAdmin').mockResolvedValue(executedOutcome());
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

// ──────────────────────────────────────────────────────────────────────────
// 轮 36：宿主投递信号 → 拉取快照 → 上屏。这条腿此前**整条不存在**。
//
// Rust `TauriDispatchSink::send` 每次通知都 `emit(NOTIFICATION_TOPIC, …)` 并返回 `Ok`，
// 注释写着「前端据此刷新通知中心」；而全仓没有任何 TS 代码监听那个 topic，
// `ShellClient.notificationsList()` 也只有自身的单测在调。发出端一路绿灯、
// 读端零消费者——通知永远到不了用户，且**不会有任何报错**。这与轮 31 那条
// 「检查腿打桩且把返回值整个丢弃」同型，只是这次断的是事件方向。
//
// 载荷刻意只有 `{ id, pluginId, kind, ts }`（广播带正文 = 跨插件泄露，轮 11 的修正），
// 所以监听端必须是「信号 + 拉取」，下面的测试钉的就是这件事。
// 竞态一律用手动 resolve 的 deferred，不用固定 sleep 自证。
// ──────────────────────────────────────────────────────────────────────────

describe('ShellController 通知腿（轮 36：信号 → 拉取 → 上屏）', () => {
  /** 一份 `host_notifications_list` 的完整线上快照。 */
  const snapshot = (): NotificationsListResult =>
    ({
      unread: 2,
      total: 2,
      items: [
        {
          id: 'n2',
          pluginId: 'p.shell',
          kind: 'warning',
          title: '磁盘将满',
          message: '剩余 3%',
          ts: 2000,
          read: false,
          data: null,
        },
        {
          id: 'n1',
          pluginId: 'p.shell',
          kind: 'info',
          title: '同步完成',
          message: '3 项已更新',
          ts: 1000,
          read: false,
          data: null,
        },
      ],
      dispatchLog: [{ entryId: 'n2', outcome: 'degraded', ts: 2001 }],
      capacity: 200,
      pluginCapacity: 50,
      pluginUsage: { 'p.shell': 2 },
    }) as NotificationsListResult;

  /** 宿主侧的投递信号载荷（如实：没有正文）。 */
  const signal = (id: string): Record<string, unknown> => ({
    id,
    pluginId: 'p.shell',
    kind: 'info',
    ts: 1,
  });

  interface Harness {
    backend: Backend;
    invocations: Array<{ cmd: string; args?: Record<string, unknown> }>;
    listenerCount: (event: string) => number;
    fire: (event: string, payload: unknown) => void;
    resolveListen: (event: string) => void;
    rejectListen: (err: unknown) => void;
    resolveList: (value: unknown) => void;
    rejectList: (err: unknown) => void;
    listCallCount: () => number;
    unsubscribed: () => number;
  }

  const harness = (): Harness => {
    const invocations: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
    const handlers = new Map<string, Set<(p: unknown) => void>>();
    const listenQueue: Array<{
      event: string;
      resolve: (u: () => void) => void;
      reject: (e: unknown) => void;
    }> = [];
    const listQueue: Array<{ resolve: (v: unknown) => void; reject: (e: unknown) => void }> = [];
    let unsubscribed = 0;

    const backend: Backend = {
      invoke: <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
        invocations.push(args === undefined ? { cmd } : { cmd, args });
        if (cmd === 'host_notifications_list') {
          return new Promise<T>((resolve, reject) => {
            listQueue.push({ resolve: resolve as (v: unknown) => void, reject });
          });
        }
        return Promise.resolve(undefined as T);
      },
      listen: (event, handler) => {
        const set = handlers.get(event) ?? new Set();
        handlers.set(event, set);
        set.add(handler);
        return new Promise<() => void>((resolve, reject) => {
          listenQueue.push({ event, resolve, reject });
        });
      },
      channel: () => ({ id: 'ch-h', onmessage: null }),
      principal: () => ({ kind: 'main-window', origin: null }),
      pluginId: () => null,
      capabilities: () => new Set(['host_notifications_list', 'host_notifications_read']),
    };

    return {
      backend,
      invocations,
      listenerCount: (event) => handlers.get(event)?.size ?? 0,
      fire: (event, payload) => handlers.get(event)?.forEach((h) => h(payload)),
      resolveListen: (event) => {
        const index = listenQueue.findIndex((l) => l.event === event);
        if (index < 0) throw new Error(`没有对 ${event} 的订阅请求`);
        const [hit] = listenQueue.splice(index, 1);
        hit!.resolve(() => {
          unsubscribed++;
          handlers.get(event)?.clear();
        });
      },
      rejectListen: (err) => listenQueue.shift()?.reject(err),
      resolveList: (value) => listQueue.shift()?.resolve(value),
      rejectList: (err) => listQueue.shift()?.reject(err),
      listCallCount: () => invocations.filter((i) => i.cmd === 'host_notifications_list').length,
      unsubscribed: () => unsubscribed,
    };
  };

  let host: HTMLElement;
  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });
  afterEach(() => {
    host.remove();
  });

  it('start() 订阅 NOTIFICATION_TOPIC，信号到达后拉取快照并交给 onNotification', async () => {
    const h = harness();
    const seen: NotificationsListResult[] = [];
    const c = new ShellController({
      backend: h.backend,
      onNotification: (snap) => {
        seen.push(snap);
      },
    });
    c.start([host]);
    await vi.waitFor(() => expect(h.listenerCount(NOTIFICATION_TOPIC)).toBe(1));
    h.resolveListen(NOTIFICATION_TOPIC);

    h.fire(NOTIFICATION_TOPIC, signal('n2'));
    await vi.waitFor(() => expect(h.listCallCount()).toBe(1));
    expect(h.invocations[0]?.args, 'limit 由控制器给出，不让宿主猜').toEqual({ limit: 20 });
    h.resolveList(snapshot());
    await vi.waitFor(() => expect(seen).toHaveLength(1));
    expect(seen[0]?.unread).toBe(2);
    c.stop();
  });

  it('正文只来自拉取：事件载荷里没有 title/message，上屏内容取自快照 items', async () => {
    const h = harness();
    const titles: string[][] = [];
    const c = new ShellController({
      backend: h.backend,
      onNotification: (snap) => {
        titles.push(snap.items.map((i) => i.title));
      },
    });
    c.start([host]);
    h.resolveListen(NOTIFICATION_TOPIC);

    const payload = signal('n1');
    expect(payload).not.toHaveProperty('title');
    expect(payload).not.toHaveProperty('body');
    h.fire(NOTIFICATION_TOPIC, payload);
    await vi.waitFor(() => expect(h.listCallCount()).toBe(1));
    h.resolveList(snapshot());
    await vi.waitFor(() => expect(titles[0]).toEqual(['磁盘将满', '同步完成']));
    c.stop();
  });

  it('没有 onNotification 出口时既不订阅也不发命令', async () => {
    const h = harness();
    const c = new ShellController({ backend: h.backend });
    c.start([host]);
    await new Promise((r) => setTimeout(r, 5));
    expect(h.listenerCount(NOTIFICATION_TOPIC), '没人展示就不该挂监听').toBe(0);
    expect(h.invocations, '没人展示就不该打命令').toEqual([]);
    c.stop();
  });

  it('连发 5 条信号合并成「在途一次 + 收尾补拉一次」，不多打命令', async () => {
    const h = harness();
    let rendered = 0;
    const c = new ShellController({
      backend: h.backend,
      notificationLimit: 5,
      onNotification: () => {
        rendered++;
      },
    });
    c.start([host]);
    h.resolveListen(NOTIFICATION_TOPIC);

    for (let i = 0; i < 5; i++) h.fire(NOTIFICATION_TOPIC, signal(`n${i}`));
    expect(h.listCallCount(), '突发期只能有一个在途拉取').toBe(1);
    expect(h.invocations[0]?.args).toEqual({ limit: 5 });

    h.resolveList(snapshot());
    await vi.waitFor(() => expect(rendered).toBe(1));
    await vi.waitFor(() => expect(h.listCallCount()).toBe(2));
    h.resolveList(snapshot());
    await vi.waitFor(() => expect(rendered).toBe(2));
    expect(h.listCallCount(), '补拉一次就归于安静').toBe(2);
    c.stop();
  });

  it('stop() 退订并停止拉取：之后的信号不再打命令', async () => {
    const h = harness();
    const seen: NotificationsListResult[] = [];
    const c = new ShellController({
      backend: h.backend,
      onNotification: (snap) => {
        seen.push(snap);
      },
    });
    c.start([host]);
    h.resolveListen(NOTIFICATION_TOPIC);
    h.fire(NOTIFICATION_TOPIC, signal('n1'));
    await vi.waitFor(() => expect(h.listCallCount()).toBe(1));
    h.resolveList(snapshot());
    await vi.waitFor(() => expect(seen).toHaveLength(1));

    c.stop();
    expect(h.unsubscribed(), 'stop 必须真的退订').toBe(1);
    expect(h.listenerCount(NOTIFICATION_TOPIC)).toBe(0);
    h.fire(NOTIFICATION_TOPIC, signal('n2'));
    expect(h.listCallCount(), '退订后的信号不该再打命令').toBe(1);
  });

  it('stop() 早于 listen 的回帧：回帧到达时就地退订，不留悬挂监听', async () => {
    const h = harness();
    const c = new ShellController({
      backend: h.backend,
      onNotification: () => {
        throw new Error('已停止的控制器不该再上屏');
      },
    });
    c.start([host]);
    c.stop();
    h.resolveListen(NOTIFICATION_TOPIC);
    await new Promise((r) => setTimeout(r, 5));
    expect(h.unsubscribed(), '令牌已变，回帧必须就地退订').toBe(1);
    expect(h.listenerCount(NOTIFICATION_TOPIC)).toBe(0);
    expect(h.invocations).toEqual([]);
  });

  it('快照回帧晚于 stop()：结果丢弃，不进接入方回调也不报错', async () => {
    const h = harness();
    const seen: NotificationsListResult[] = [];
    const errors: string[] = [];
    const c = new ShellController({
      backend: h.backend,
      onError: (_err, context) => errors.push(context),
      onNotification: (snap) => {
        seen.push(snap);
      },
    });
    c.start([host]);
    h.resolveListen(NOTIFICATION_TOPIC);
    h.fire(NOTIFICATION_TOPIC, signal('n1'));
    await vi.waitFor(() => expect(h.listCallCount()).toBe(1));
    c.stop();
    h.resolveList(snapshot());
    await new Promise((r) => setTimeout(r, 5));
    expect(seen, '晚到的快照不得上屏').toEqual([]);
    expect(errors).toEqual([]);
  });

  it('订阅失败走 onError：监听拿不到时接线方看得见，不是静默', async () => {
    const h = harness();
    const errors: Array<[string, string]> = [];
    const c = new ShellController({
      backend: h.backend,
      onError: (err, context) => errors.push([context, String((err as Error).message)]),
      onNotification: () => {},
    });
    c.start([host]);
    h.rejectListen(new Error('事件层不可用'));
    await vi.waitFor(() => expect(errors).toEqual([['notification.subscribe', '事件层不可用']]));
    expect(h.invocations, '订阅没成功就不该有拉取').toEqual([]);
    c.stop();
  });

  it('拉取失败走 onError：信号到了但快照取不回，也要有可查出口', async () => {
    const h = harness();
    const errors: string[] = [];
    const c = new ShellController({
      backend: h.backend,
      onError: (_err, context) => errors.push(context),
      onNotification: () => {},
    });
    c.start([host]);
    h.resolveListen(NOTIFICATION_TOPIC);
    h.fire(NOTIFICATION_TOPIC, signal('n1'));
    await vi.waitFor(() => expect(h.listCallCount()).toBe(1));
    h.rejectList(new Error('command not found'));
    await vi.waitFor(() => expect(errors).toEqual(['notification.pull']));
    expect(h.listCallCount(), '失败后不留脏在途标记，也不自动重投').toBe(1);
    c.stop();
  });

  it('接入方回调自己抛错走 notification.render，不把控制器带崩', async () => {
    const h = harness();
    const errors: string[] = [];
    const c = new ShellController({
      backend: h.backend,
      onError: (_err, context) => errors.push(context),
      onNotification: () => {
        throw new Error('渲染层炸了');
      },
    });
    c.start([host]);
    h.resolveListen(NOTIFICATION_TOPIC);
    h.fire(NOTIFICATION_TOPIC, signal('n1'));
    await vi.waitFor(() => expect(h.listCallCount()).toBe(1));
    h.resolveList(snapshot());
    await vi.waitFor(() => expect(errors).toEqual(['notification.render']));
    c.stop();
  });
});

describe('轮 63：窗口几何持久化链路（存档 → 恢复、DOM → 落盘）', () => {
  const KEY = 'tauron.window.state';
  const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

  /** 把环境尺寸钉成固定值：断言不该依赖 happy-dom 的默认 screen/outer 读数。 */
  function pinEnv(dims: {
    screenW: number;
    screenH: number;
    outerW: number;
    outerH: number;
    x: number;
    y: number;
  }): void {
    Object.defineProperty(window.screen, 'width', { value: dims.screenW, configurable: true });
    Object.defineProperty(window.screen, 'height', { value: dims.screenH, configurable: true });
    Object.defineProperty(window, 'outerWidth', { value: dims.outerW, configurable: true });
    Object.defineProperty(window, 'outerHeight', { value: dims.outerH, configurable: true });
    Object.defineProperty(window, 'screenX', { value: dims.x, configurable: true });
    Object.defineProperty(window, 'screenY', { value: dims.y, configurable: true });
  }

  /** invoke 停在闸门上的后端：用来证明 `stop()` 之后剩余回写不再发出。 */
  class GateBackend extends MockBackend {
    private _gate: Promise<void> = Promise.resolve();
    private _release: (() => void) | null = null;
    hold(): void {
      this._gate = new Promise<void>((resolve) => {
        this._release = resolve;
      });
    }
    release(): void {
      this._release?.();
    }
    override async invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
      await this._gate;
      return super.invoke<T>(cmd, args);
    }
  }

  /**
   * 本文件的 happy-dom 环境没有完整 `Storage`（`clear()` 就不存在），而控制器在构造时
   * 探测存储可用性、探不到就整条腿不启动。用一个内存桩把前提钉成事实——而不是
   * 让「腿没启动」冒充「恢复逻辑正确」。
   */
  function installStorage(): void {
    const store = new Map<string, string>();
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => {
          store.set(k, String(v));
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
    });
  }

  let geomBackend: MockBackend;
  let errors: string[];
  let ctl: ShellController;
  let box: HTMLElement;

  /** 先落存档再建控制器——恢复腿在构造函数里读盘，顺序反过来就是另一件事。 */
  function build(saved?: Record<string, unknown>, b?: MockBackend): void {
    if (saved !== undefined) window.localStorage.setItem(KEY, JSON.stringify(saved));
    ctl = new ShellController({
      backend: b ?? geomBackend,
      onError: (_err, context) => {
        errors.push(context);
      },
    });
  }

  beforeEach(() => {
    installStorage();
    pinEnv({ screenW: 1920, screenH: 1080, outerW: 1100, outerH: 700, x: 60, y: 40 });
    errors = [];
    geomBackend = new MockBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_window_set_position', 'host_window_set_size'],
      pluginId: 'p.geom',
    });
    box = document.createElement('div');
    document.body.appendChild(box);
    build();
  });

  afterEach(() => {
    ctl.stop();
    box.remove();
    Reflect.deleteProperty(window, 'localStorage');
  });

  const geometryCalls = (b: MockBackend = geomBackend) =>
    b.invocations.filter(
      (i) => i.cmd === 'host_window_set_position' || i.cmd === 'host_window_set_size',
    );

  it('没有存档时一条几何命令都不发（默认值长得像结论，但不是事实）', async () => {
    ctl.start([box]);
    await flush();
    expect(geometryCalls()).toEqual([]);
    expect(errors).toEqual([]);
  });

  it('有存档且在屏内：先位置后尺寸，两条命令的键名逐字对上线上信封', async () => {
    build({ x: 20, y: 30, width: 900, height: 600, isMaximized: false });
    ctl.start([box]);
    await flush();
    expect(geomBackend.invocations).toEqual([
      { cmd: 'host_window_set_position', args: { x: 20, y: 30 } },
      { cmd: 'host_window_set_size', args: { width: 900, height: 600 } },
    ]);
  });

  it('越界存档不落平台，但必须留痕且不删存档（坏值不拒启）', async () => {
    build({ x: 5000, y: 5000, width: 900, height: 600, isMaximized: false });
    ctl.start([box]);
    await flush();
    expect(geometryCalls()).toEqual([]);
    expect(errors).toEqual(['window.geometry.restore']);
    expect(window.localStorage.getItem(KEY)).not.toBeNull();
  });

  it('stop() 之后剩余回写不再发出（两段式恢复不能只完成前一半）', async () => {
    const gated = new GateBackend({
      capabilities: [...CONTROLLER_CAPS, 'host_window_set_position', 'host_window_set_size'],
    });
    gated.hold();
    build({ x: 20, y: 30, width: 900, height: 600, isMaximized: false }, gated);
    ctl.start([box]);
    ctl.stop();
    gated.release();
    await flush();
    expect(gated.invocations.map((i) => i.cmd)).toEqual(['host_window_set_position']);
    expect(errors).toEqual([]);
  });

  it('resize 与 pagehide 各落一次盘（落的是 webview 可见的当前几何）', async () => {
    ctl.start([box]);
    pinEnv({ screenW: 1920, screenH: 1080, outerW: 1366, outerH: 768, x: 12, y: 34 });
    window.dispatchEvent(new Event('resize'));
    expect(JSON.parse(String(window.localStorage.getItem(KEY)))).toMatchObject({
      x: 12,
      y: 34,
      width: 1366,
      height: 768,
    });
    pinEnv({ screenW: 1920, screenH: 1080, outerW: 800, outerH: 600, x: 1, y: 2 });
    window.dispatchEvent(new Event('pagehide'));
    expect(JSON.parse(String(window.localStorage.getItem(KEY)))).toMatchObject({
      x: 1,
      y: 2,
      width: 800,
      height: 600,
    });
    expect(geometryCalls()).toEqual([]);
  });

  it('环境给不出可用存储时整条腿不启动（不恢复，也不把环境问题刷成用户报错）', async () => {
    // 只有 getItem 的“存储”不算存储：三个方法都在才敢把腿装起来。这份桩故意返回
    // 一个看着合法的存档——腿若启动就会照着它动窗口。
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: { getItem: () => '{"width":900,"height":600,"x":20,"y":30}' },
    });
    build();
    ctl.start([box]);
    await flush();
    expect(geometryCalls()).toEqual([]);
    window.dispatchEvent(new Event('resize'));
    expect(errors).toEqual([]);
  });

  it('写盘失败不静默，也不打断用户操作', () => {
    ctl.start([box]);
    Object.defineProperty(window.localStorage, 'setItem', {
      value: () => {
        throw new Error('QuotaExceeded');
      },
      configurable: true,
    });
    expect(() => window.dispatchEvent(new Event('resize'))).not.toThrow();
    expect(errors).toEqual(['window.geometry.save']);
  });
});
