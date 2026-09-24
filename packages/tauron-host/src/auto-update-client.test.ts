// @vitest-environment happy-dom
// auto-update-client.ts 测试（P2-12：自动更新客户端）

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { AutoUpdateClient, createAutoUpdateClient } from './auto-update-client.js';
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

describe('AutoUpdateClient', () => {
  let backend: MockBackend;
  let client: AutoUpdateClient;

  beforeEach(() => {
    backend = new MockBackend({
      capabilities: [
        'host_market_check',
        'host_market_download',
        'host_market_install',
        'host_window_relaunch',
      ],
      cases: [
        {
          cmd: 'host_market_check',
          result: { available: true, version: '2.0.0', currentVersion: '1.0.0' },
        },
        {
          cmd: 'host_market_download',
          result: undefined,
        },
        {
          cmd: 'host_market_install',
          result: undefined,
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
    client = new AutoUpdateClient({ backend });
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

    it('检查更新失败', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_market_check'],
        cases: [
          {
            cmd: 'host_market_check',
            error: new Error('network error'),
          },
        ],
      });
      const client2 = new AutoUpdateClient({ backend: backend2 });
      await expect(client2.checkUpdate()).rejects.toThrow('network error');
      expect(client2.status).toBe('error');
    });

    it('autoDownload: true 时自动下载', async () => {
      const client2 = new AutoUpdateClient({
        backend,
        config: { autoDownload: true },
      });

      await client2.checkUpdate();
      expect(backend.invocations.some(i => i.cmd === 'host_market_download')).toBe(true);
    });

    it('autoDownload 下载失败不得变成 unhandled rejection', async () => {
      // 断链回归：checkUpdate 的 autoDownload 路径此前裸调 downloadUpdate()——
      // 后台下载 reject 时没人接住，unhandled rejection 直接炸宿主进程。
      const backend2 = new MockBackend({
        capabilities: ['host_market_check', 'host_market_download'],
        cases: [
          {
            cmd: 'host_market_check',
            result: { available: true, version: '2.0.0', currentVersion: '1.0.0' },
          },
          { cmd: 'host_market_download', error: new Error('download failed') },
        ],
      });
      const client2 = new AutoUpdateClient({
        backend: backend2,
        config: { autoDownload: true },
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

    it('下载失败', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_market_check', 'host_market_download'],
        cases: [
          {
            cmd: 'host_market_check',
            result: { available: true, version: '2.0.0', currentVersion: '1.0.0' },
          },
          {
            cmd: 'host_market_download',
            error: new Error('download failed'),
          },
        ],
      });
      const client2 = new AutoUpdateClient({ backend: backend2 });
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
        capabilities: ['host_market_check', 'host_market_download', 'host_market_install'],
        cases: [
          {
            cmd: 'host_market_check',
            result: { available: true, version: '2.0.0', currentVersion: '1.0.0' },
          },
          {
            cmd: 'host_market_download',
            result: undefined,
          },
          {
            cmd: 'host_market_install',
            error: new Error('install failed'),
          },
        ],
      });
      const client2 = new AutoUpdateClient({ backend: backend2 });
      await client2.checkUpdate();
      await client2.downloadUpdate();
      await expect(client2.installUpdate()).rejects.toThrow('install failed');
      expect(client2.status).toBe('error');
    });
  });

  describe('relaunch()', () => {
    it('重启走 relaunch 而非 quit（quit 只退出且跳过恢复对账）', async () => {
      const outcome = await client.relaunch();
      expect(backend.invocations.some(i => i.cmd === 'host_window_relaunch')).toBe(true);
      expect(
        backend.invocations.some(i => i.cmd === 'host_window_quit'),
        'quit 只退出、应用不会回来，且跳过恢复阶段对账',
      ).toBe(false);
      expect(outcome.relaunchRequested).toBe(true);
      expect(outcome.reason).toBeNull();
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
      const outcome = await c.relaunch();
      expect(outcome.relaunchRequested).toBe(false);
      expect(outcome.reason).toBeTruthy();
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
      const client2 = new AutoUpdateClient({
        backend,
        config: { checkIntervalSecs: 60 },
      });
      client2.startAutoCheck();
      expect(internals(client2)._checkTimer).not.toBeNull();
      client2.stopAutoCheck();
    });

    it('checkIntervalSecs: 0 不自动检查', () => {
      const client2 = new AutoUpdateClient({
        backend,
        config: { checkIntervalSecs: 0 },
      });
      client2.startAutoCheck();
      expect(internals(client2)._checkTimer).toBeNull();
    });

    it('stopAutoCheck 停止自动检查', () => {
      const client2 = new AutoUpdateClient({
        backend,
        config: { checkIntervalSecs: 60 },
      });
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
});