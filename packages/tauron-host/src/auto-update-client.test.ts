// @vitest-environment happy-dom
// auto-update-client.ts 测试（P2-12：自动更新客户端；轮 29 起检查走真通道 host_updater_check）

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { AutoUpdateClient, createAutoUpdateClient } from './auto-update-client.js';
import type { UpdaterCheckOutcome } from './shell-client.js';
import { MockBackend } from './backend.js';

/**
 * 私有实现细节的结构访问器。
 *
 * `_setStatus` / `_checkTimer` 是私有成员（生产代码不应外露）。测试需要观察
 * 「状态广播不重复触发」与「定时器生命周期」，因此这里用结构断言读取，
 * 而不是把生产代码的可见性放宽成 public。
 */
function internals(c: AutoUpdateClient): {
  _checkTimer: unknown;
  _setStatus(status: string): void;
} {
  return c as unknown as { _checkTimer: unknown; _setStatus(status: string): void };
}

/** 宿主 `host_updater_check` 的线上形状（Rust `UpdaterCheckOutcome`）。 */
function outcome(over: Partial<UpdaterCheckOutcome> = {}): UpdaterCheckOutcome {
  return {
    available: false,
    version: null,
    url: null,
    releasedAt: null,
    degraded: false,
    reason: null,
    ...over,
  };
}

/** 「有更新可用」的真通道用例。 */
const AVAILABLE = {
  cmd: 'host_updater_check',
  result: outcome({ available: true, version: '2.0.0' }),
};

/** 客户端必带的当前版本：真通道把它当必填入参。 */
const CFG = { currentVersion: '1.0.0' };

describe('AutoUpdateClient', () => {
  let backend: MockBackend;
  let client: AutoUpdateClient;

  beforeEach(() => {
    backend = new MockBackend({
      capabilities: [
        'host_updater_check',
        'host_market_check',
        'host_market_download',
        'host_market_install',
        'host_window_relaunch',
      ],
      cases: [
        AVAILABLE,
        {
          cmd: 'host_market_download',
          result: { ok: true, simulated: false, version: '2.0.0', reason: null },
        },
        {
          cmd: 'host_market_install',
          result: { ok: true, simulated: false, version: '2.0.0', reason: null },
        },
        {
          cmd: 'host_window_relaunch',
          result: {
            reconcile: { applied: 1, events: [] },
            relaunchRequested: true,
            reason: null,
          },
        },
      ],
    });
    client = new AutoUpdateClient({ backend, config: CFG });
  });

  describe('基本功能', () => {
    it('创建实例', () => {
      expect(client).toBeDefined();
      expect(client.status).toBe('idle');
      expect(client.info).toBeNull();
    });

    it('createAutoUpdateClient 工厂函数', () => {
      const c = createAutoUpdateClient({ backend });
      expect(c).toBeInstanceOf(AutoUpdateClient);
    });

    it('默认配置', () => {
      expect(client).toBeDefined();
    });
  });

  describe('checkUpdate()', () => {
    it('检查更新成功（有更新）', async () => {
      const info = await client.checkUpdate();
      expect(info.available).toBe(true);
      expect(client.status).toBe('available');
      expect(client.info?.version).toBe('2.0.0');
    });

    it('轮 29：可用性结论取自真通道，不取自宿主桩', async () => {
      await client.checkUpdate();
      expect(backend.invocations.map(({ cmd }) => cmd)).toEqual(['host_updater_check']);
    });

    it('轮 29：config.currentVersion 作为入参下发', async () => {
      await client.checkUpdate();
      expect(backend.invocations[0]?.args).toEqual({ currentVersion: '1.0.0' });
    });

    it('轮 29：未声明 currentVersion 时抛错且不打任何命令', async () => {
      const c = new AutoUpdateClient({ backend });
      await expect(c.checkUpdate()).rejects.toThrow(/currentVersion/);
      expect(c.status).toBe('error');
      expect(backend.invocations).toEqual([]);
    });

    it('轮 29：通道未装配（Unsupported）不等于"已是最新"', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check'],
        cases: [
          {
            cmd: 'host_updater_check',
            result: {
              supported: false,
              reason: 'updater provider is not configured',
              fallback: '注入 EndpointClient',
            },
          },
        ],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      const info = await c.checkUpdate();
      expect(info.available).toBe(false);
      expect(info.degraded).toBe(true);
      expect(info.reason).toBe('updater provider is not configured');
      // 状态落 error 而不是 idle：idle 在 UI 上就是"查过了，没有更新"。
      expect(c.status).toBe('error');
    });

    it('轮 29：宿主答"已是最新"才是 idle（degraded 为假）', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check'],
        cases: [{ cmd: 'host_updater_check', result: outcome({ reason: '已是最新版本' }) }],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      const info = await c.checkUpdate();
      expect(info.available).toBe(false);
      expect(info.degraded).toBe(false);
      expect(c.status).toBe('idle');
    });

    it('轮 29：签名非法等降级路径落 error，不静默空闲', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check'],
        cases: [
          {
            cmd: 'host_updater_check',
            result: outcome({ degraded: true, reason: '更新清单签名非法（已拒绝）' }),
          },
        ],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      const info = await c.checkUpdate();
      expect(info.degraded).toBe(true);
      expect(info.reason).toContain('签名非法');
      expect(c.status).toBe('error');
    });

    it('轮 29：宿主返回空结果时抛错，不当作"没有更新"', async () => {
      const backend2 = new MockBackend({ capabilities: ['host_updater_check'] });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      await expect(c.checkUpdate()).rejects.toThrow(/空结果/);
      expect(c.status).toBe('error');
    });

    // ── 轮 33：`available: false` 不是一个事实，只是一句结论，得带出处 ──
    //
    // 宿主侧"没有更新"至少对应四种情况：已是最新、不在灰度批次、被崩溃门禁停发、
    // 端点未装配。后三种只写在 `host_updater_status` 里，而那条命令此前在 TS 侧零消费者。
    // 补读数的口径与 `<oc-updater-dialog>` 的检查按钮共用同一个
    // `enrichUpdaterInfoWithChannel`（两条入口各拼一句就是第三面镜像）。

    /** `host_updater_status` 的线上形状（Rust `UpdaterStatus`）。 */
    const channel = (over: Record<string, unknown> = {}) => ({
      available: true,
      state: null,
      stateSimulated: false,
      grayscalePercent: 100,
      crashGateStopped: false,
      reason: null,
      ...over,
    });

    it('轮 33：答"没有更新"时把通道事实补进 reason，且 store 里也是补过的', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_updater_status'],
        cases: [
          { cmd: 'host_updater_check', result: outcome({ reason: '不在灰度批次' }) },
          {
            cmd: 'host_updater_status',
            result: channel({
              grayscalePercent: 30,
              state: 'installed:2.0.0',
              stateSimulated: true,
            }),
          },
        ],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      const info = await c.checkUpdate();
      expect(info.reason).toBe(
        '更新通道已装配｜灰度 30%｜崩溃门禁未停发｜宿主账本 installed:2.0.0（模拟推进，未真的装上）｜不在灰度批次',
      );
      // 订阅方读 `client.info`（托盘徽标、设置页），读到的必须是同一份，不能是半句。
      expect(c.info?.reason).toBe(info.reason);
      expect(c.status).toBe('idle');
    });

    it('轮 33：有更新时不多打诊断命令（一次点击不为附加读数多付一条 IPC）', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_updater_status'],
        cases: [
          { cmd: 'host_updater_check', result: outcome({ available: true, version: '2.0.0' }) },
        ],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      await c.checkUpdate();
      expect(backend2.invocations.map(({ cmd }) => cmd)).toEqual(['host_updater_check']);
    });

    it('轮 33：诊断缺席/报错/答空都不改结论与状态（附加读数不得顶掉主答案）', async () => {
      const check = { cmd: 'host_updater_check', result: outcome({ reason: '已是最新版本' }) };
      const variants = [
        // ① 命令不存在（旧宿主）：mock 抛 command not found。
        new MockBackend({ capabilities: ['host_updater_check'], cases: [check] }),
        // ② 命令在但报错。
        new MockBackend({
          capabilities: ['host_updater_check', 'host_updater_status'],
          cases: [check, { cmd: 'host_updater_status', error: new Error('boom') }],
        }),
        // ③ 命令答了个空：空不得渲染成"通道已装配、灰度 NaN%"。
        new MockBackend({
          capabilities: ['host_updater_check', 'host_updater_status'],
          cases: [check, { cmd: 'host_updater_status', result: undefined }],
        }),
      ];
      for (const b of variants) {
        const c = new AutoUpdateClient({ backend: b, config: CFG });
        const info = await c.checkUpdate();
        expect(info.degraded).toBe(false);
        expect(info.reason).toBe('已是最新版本');
        expect(c.status).toBe('idle');
      }
    });

    it('轮 29：发布时间与当前版本随真通道结果映射出来', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check'],
        cases: [
          {
            cmd: 'host_updater_check',
            result: outcome({ available: true, version: '2.0.0', releasedAt: '2026-10-01' }),
          },
        ],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      const info = await c.checkUpdate();
      expect(info.publishedAt).toBe('2026-10-01');
      expect(info.currentVersion).toBe('1.0.0');
    });

    it('检查更新失败', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check'],
        cases: [{ cmd: 'host_updater_check', error: new Error('network error') }],
      });
      const client2 = new AutoUpdateClient({ backend: backend2, config: CFG });
      await expect(client2.checkUpdate()).rejects.toThrow('network error');
      expect(client2.status).toBe('error');
    });

    it('autoDownload: true 时自动下载', async () => {
      const client2 = new AutoUpdateClient({ backend, config: { ...CFG, autoDownload: true } });

      await client2.checkUpdate();
      expect(backend.invocations.some((i) => i.cmd === 'host_market_download')).toBe(true);
    });

    it('autoDownload 下载失败不得变成 unhandled rejection', async () => {
      // 断链回归：checkUpdate 的 autoDownload 路径此前裸调 downloadUpdate()——
      // 后台下载 reject 时没人接住，unhandled rejection 直接炸宿主进程。
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_market_download'],
        cases: [AVAILABLE, { cmd: 'host_market_download', error: new Error('download failed') }],
      });
      const client2 = new AutoUpdateClient({
        backend: backend2,
        config: { ...CFG, autoDownload: true },
      });

      await client2.checkUpdate(); // 内部自动下载（失败必须被吸收）
      await new Promise((r) => setTimeout(r, 0)); // 让后台 rejection 落地
      expect(client2.status).toBe('error');
    });
  });

  describe('downloadUpdate()', () => {
    it('下载更新成功', async () => {
      await client.checkUpdate();
      await client.downloadUpdate();
      expect(client.status).toBe('downloaded');
    });

    it('无可用更新时抛出错误', async () => {
      await expect(client.downloadUpdate()).rejects.toThrow('No update available');
    });

    it('检查是真通道、下载仍是宿主桩：如实抛错而不假装已下载', async () => {
      // 轮 29 的实际形状：能如实报"有更新可用"，还不能真的把更新装上。
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_market_download'],
        cases: [
          AVAILABLE,
          {
            cmd: 'host_market_download',
            result: { ok: true, simulated: true, reason: '未接入下载器' },
          },
        ],
      });
      const c = new AutoUpdateClient({ backend: backend2, config: CFG });
      const info = await c.checkUpdate();
      expect(info.available).toBe(true);
      expect(c.status).toBe('available');
      await expect(c.downloadUpdate()).rejects.toThrow('未接入下载器');
      expect(c.status).toBe('error');
    });

    it('下载失败', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_market_download'],
        cases: [AVAILABLE, { cmd: 'host_market_download', error: new Error('download failed') }],
      });
      const client2 = new AutoUpdateClient({ backend: backend2, config: CFG });
      await client2.checkUpdate();
      await expect(client2.downloadUpdate()).rejects.toThrow('download failed');
      expect(client2.status).toBe('error');
    });
  });

  describe('installUpdate()', () => {
    it('安装更新成功', async () => {
      await client.checkUpdate();
      await client.downloadUpdate();
      await client.installUpdate();
      expect(client.status).toBe('ready');
    });

    it('未下载时抛出错误', async () => {
      await expect(client.installUpdate()).rejects.toThrow('Update not downloaded');
    });

    it('安装失败', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_market_download', 'host_market_install'],
        cases: [
          AVAILABLE,
          {
            cmd: 'host_market_download',
            result: { ok: true, simulated: false, version: '2.0.0', reason: null },
          },
          { cmd: 'host_market_install', error: new Error('install failed') },
        ],
      });
      const client2 = new AutoUpdateClient({ backend: backend2, config: CFG });
      await client2.checkUpdate();
      await client2.downloadUpdate();
      await expect(client2.installUpdate()).rejects.toThrow('install failed');
      expect(client2.status).toBe('error');
    });

    it('宿主返回模拟结果时不得推进为已安装', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_updater_check', 'host_market_download', 'host_market_install'],
        cases: [
          AVAILABLE,
          { cmd: 'host_market_download', result: { ok: true, simulated: false, reason: null } },
          {
            cmd: 'host_market_install',
            result: { ok: true, simulated: true, reason: '未接入安装器' },
          },
        ],
      });
      const client2 = new AutoUpdateClient({ backend: backend2, config: CFG });
      await client2.checkUpdate();
      await client2.downloadUpdate();
      await expect(client2.installUpdate()).rejects.toThrow('未接入安装器');
      expect(client2.status).toBe('error');
    });
  });

  describe('relaunch()', () => {
    it('重启走 relaunch 而非 quit（quit 只退出且跳过恢复对账）', async () => {
      const res = await client.relaunch();
      expect(backend.invocations.some((i) => i.cmd === 'host_window_relaunch')).toBe(true);
      expect(
        backend.invocations.some((i) => i.cmd === 'host_window_quit'),
        'quit 只退出、应用不会回来，且跳过恢复阶段对账',
      ).toBe(false);
      expect(res.relaunchRequested).toBe(true);
      expect(res.reason).toBeNull();
    });

    it('降级宿主 relaunchRequested=false 时如实返回，不谎报已重启', async () => {
      const degraded = new MockBackend({
        capabilities: ['host_window_relaunch'],
        cases: [
          {
            cmd: 'host_window_relaunch',
            result: {
              reconcile: { applied: 0, events: [] },
              relaunchRequested: false,
              reason: '宿主无重启原语',
            },
          },
        ],
      });
      const c = new AutoUpdateClient({ backend: degraded });
      const res = await c.relaunch();
      expect(res.relaunchRequested).toBe(false);
      expect(res.reason).toBeTruthy();
    });
  });

  describe('subscribe()', () => {
    it('订阅状态变化', async () => {
      const statuses: string[] = [];
      client.subscribe((s) => statuses.push(s));

      await client.checkUpdate();
      expect(statuses).toContain('checking');
      expect(statuses).toContain('available');
    });

    it('unsubscribe 取消订阅', () => {
      let count = 0;
      const unsub = client.subscribe(() => count++);
      // subscribe 不立即通知，所以 count 仍为 0
      expect(count).toBe(0);

      // 触发状态变化
      internals(client)._setStatus('checking');
      expect(count).toBe(1);

      // 取消订阅
      unsub();

      // 再次触发不应调用
      internals(client)._setStatus('available');
      expect(count).toBe(1);
    });
  });

  describe('startAutoCheck()/stopAutoCheck()', () => {
    it('startAutoCheck 开始自动检查', () => {
      const client2 = new AutoUpdateClient({ backend, config: { ...CFG, checkIntervalSecs: 60 } });
      client2.startAutoCheck();
      expect(internals(client2)._checkTimer).not.toBeNull();
      client2.stopAutoCheck();
    });

    it('checkIntervalSecs: 0 不自动检查', () => {
      const client2 = new AutoUpdateClient({ backend, config: { ...CFG, checkIntervalSecs: 0 } });
      client2.startAutoCheck();
      expect(internals(client2)._checkTimer).toBeNull();
    });

    it('stopAutoCheck 停止自动检查', () => {
      const client2 = new AutoUpdateClient({ backend, config: { ...CFG, checkIntervalSecs: 60 } });
      client2.startAutoCheck();
      expect(internals(client2)._checkTimer).not.toBeNull();
      client2.stopAutoCheck();
      expect(internals(client2)._checkTimer).toBeNull();
    });
  });

  describe('destroy()', () => {
    it('清理资源', () => {
      client.startAutoCheck();
      client.destroy();
      expect(internals(client)._checkTimer).toBeNull();
    });
  });

  describe('自发调用有界（轮 2：并发堆叠回归）', () => {
    /**
     * `host_updater_check` 可延迟的后端。
     *
     * 为什么要自己控速：这条回归测的是「上一次还在飞时不要叠下一次」，
     * 而后端默认**立即**返回——两次自发调用永远撞不到一起，测试就空跑了。
     */
    class SlowBackend extends MockBackend {
      readonly gate: Promise<void>;
      private release!: () => void;

      constructor() {
        super({
          capabilities: ['host_updater_check', 'host_market_download'],
          cases: [AVAILABLE, { cmd: 'host_market_download', result: { ok: true } }],
        });
        this.gate = new Promise<void>((resolve) => {
          this.release = resolve;
        });
      }

      override async invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
        // 先让父类把这次调用**记账**，再把这一次调用挂住——否则在飞期间
        // `invocations` 是空的，断言就变成"什么都没发生"的同义反复。
        const value = await super.invoke<T>(cmd, args);
        if (cmd === 'host_updater_check') {
          await this.gate;
        }
        return value;
      }

      unblock(): void {
        this.release();
      }

      count(cmd: string): number {
        return this.invocations.filter((entry) => entry.cmd === cmd).length;
      }
    }

    afterEach(() => {
      vi.useRealTimers();
    });

    it('检查在飞时定时器不再叠下一次检查', async () => {
      vi.useFakeTimers();
      const slow = new SlowBackend();
      const auto = new AutoUpdateClient({
        backend: slow,
        config: { ...CFG, checkIntervalSecs: 60 },
      });
      auto.startAutoCheck();
      // 三拍定时 + 一次立即检查：只有第一次真的落到后端。
      await vi.advanceTimersByTimeAsync(180_000);
      expect(slow.count('host_updater_check')).toBe(1);
      // 先撤掉 interval（自续的定时器不能用 runAllTimers 收尾），再放开后端。
      auto.stopAutoCheck();
      slow.unblock();
      await vi.advanceTimersByTimeAsync(180_000);
      expect(slow.count('host_updater_check')).toBe(1);
    });

    it('autoDownload：同一时刻只允许一个下载在飞', async () => {
      const slow = new SlowBackend();
      const auto = new AutoUpdateClient({
        backend: slow,
        config: { ...CFG, autoDownload: true },
      });
      // 两次并发检查都会报 `available`；自发下载只能被起一次。
      const first = auto.checkUpdate();
      const second = auto.checkUpdate();
      slow.unblock();
      await Promise.all([first, second]);
      // 让 `downloadUpdate` 的整条微任务链跑完（真实事件循环的一拍，不是猜次数）。
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(slow.count('host_updater_check')).toBe(2);
      expect(slow.count('host_market_download')).toBe(1);
    });
  });
});
