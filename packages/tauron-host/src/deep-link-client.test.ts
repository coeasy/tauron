// @vitest-environment happy-dom
// deep-link-client.ts 测试（P2-14：深链接客户端）

import { describe, it, expect, beforeEach } from 'vitest';
import { DeepLinkClient, createDeepLinkClient } from './deep-link-client.js';
import type { DeepLinkEvent } from './deep-link-client.js';
import { MockBackend } from './backend.js';

describe('DeepLinkClient', () => {
  let backend: MockBackend;
  let client: DeepLinkClient;

  beforeEach(() => {
    backend = new MockBackend({
      capabilities: ['host_deep_link_register'],
      cases: [
        { cmd: 'host_deep_link_register', result: { supported: false, reason: 'OS provider missing', fallback: 'internal-event-routing' } },
      ],
    });
    client = new DeepLinkClient({
      backend,
      config: {
        protocol: 'tauron',
        enabled: true,
        handlers: [
          { path: '/open', handler: 'file-open' },
          { path: '/share', handler: 'file-share' },
        ],
      },
    });
  });

  describe('基本功能', () => {
    it('创建实例', () => {
      expect(client).toBeDefined();
      expect(client.isRegistered).toBe(false);
      expect(client.protocol).toBe('tauron');
    });

    it('createDeepLinkClient 工厂函数', () => {
      const c = createDeepLinkClient({
        backend,
        config: { protocol: 'tauron', enabled: true },
      });
      expect(c).toBeInstanceOf(DeepLinkClient);
    });
  });

  describe('register()', () => {
    it('注册深链接协议', async () => {
      const result = await client.register();
      expect(client.isRegistered).toBe(true);
      expect(result).toMatchObject({ supported: false, fallback: 'internal-event-routing' });
      expect(backend.invocations.some(i => i.cmd === 'host_deep_link_register')).toBe(true);
    });

    it('enabled: false 不注册', async () => {
      const client2 = new DeepLinkClient({
        backend,
        config: { protocol: 'tauron', enabled: false },
      });
      await client2.register();
      expect(client2.isRegistered).toBe(false);
    });
  });

  describe('unregister()', () => {
    it('取消注册', async () => {
      await client.register();
      expect(client.isRegistered).toBe(true);
      await client.unregister();
      expect(client.isRegistered).toBe(false);
    });
  });

  describe('parseUrl()', () => {
    it('解析完整 URL', () => {
      const event = client.parseUrl('tauron://my-plugin/path/to/file?foo=bar&baz=qux#section');
      expect(event.protocol).toBe('tauron');
      expect(event.host).toBe('my-plugin');
      expect(event.path).toBe('/path/to/file');
      expect(event.query.foo).toBe('bar');
      expect(event.query.baz).toBe('qux');
      expect(event.hash).toBe('#section');
    });

    it('解析简单 URL', () => {
      const event = client.parseUrl('tauron://my-plugin/action');
      expect(event.host).toBe('my-plugin');
      expect(event.path).toBe('/action');
      expect(Object.keys(event.query).length).toBe(0);
    });

    it('解析无查询 URL', () => {
      const event = client.parseUrl('tauron://my-plugin');
      expect(event.host).toBe('my-plugin');
      expect(event.path).toBe('');
    });

    it('解析无效 URL', () => {
      const event = client.parseUrl('invalid-url');
      expect(event.url).toBe('invalid-url');
      expect(event.host).toBe('');
      expect(event.path).toBe('');
    });
  });

  describe('matchHandler()', () => {
    it('匹配处理器', () => {
      expect(client.matchHandler('/open')).toBe('file-open');
      expect(client.matchHandler('/share')).toBe('file-share');
      expect(client.matchHandler('/open/file.png')).toBe('file-open');
    });

    it('无匹配返回 null', () => {
      expect(client.matchHandler('/unknown')).toBeNull();
    });
  });

  describe('subscribe()', () => {
    it('订阅深链接事件', async () => {
      const events: DeepLinkEvent[] = [];
      client.subscribe((event) => events.push(event));

      // 先注册，然后模拟收到事件
      await client.register();
      
      const handler = backend.subscriptions.get('deep-link');
      if (handler) {
        for (const fn of handler) {
          fn({ url: 'tauron://plugin/action' });
        }
      }

      expect(events.length).toBe(1);
      expect(events[0]!.url).toBe('tauron://plugin/action');
    });

    it('unsubscribe 取消订阅', async () => {
      let count = 0;
      const unsub = client.subscribe(() => count++);

      // 先注册
      await client.register();

      const handler = backend.subscriptions.get('deep-link');
      if (handler) {
        for (const fn of handler) {
          fn({ url: 'tauron://plugin/action' });
        }
      }
      expect(count).toBe(1);

      unsub();

      // 再次触发不应调用
      if (handler) {
        for (const fn of handler) {
          fn({ url: 'tauron://plugin/action2' });
        }
      }
      expect(count).toBe(1);
    });
  });

  describe('destroy()', () => {
    it('清理资源', async () => {
      await client.register();
      await client.destroy();
      expect(client.isRegistered).toBe(false);
    });
  });
});
