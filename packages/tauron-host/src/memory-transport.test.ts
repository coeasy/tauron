import { describe, expect, it, vi } from 'vitest';
import { MemoryTransport } from './memory-transport.js';
import { FrameSink } from './host.js';
import { ShellClient } from './shell-client.js';

describe('MemoryTransport', () => {
  it('serves registered host commands through a production client', async () => {
    const transport = new MemoryTransport({
      commands: {
        host_settings_get: (args) => (args?.key === 'theme' ? 'dark' : null),
        host_capabilities: () => ({
          families: ['settings'],
          commands: ['host_settings_get'],
          unsupported: [],
          pluginRuntime: false,
        }),
      },
    });
    const client = new ShellClient({ backend: transport });

    await expect(client.settingsGet('theme')).resolves.toEqual('dark');
    await expect(client.capabilities()).resolves.toMatchObject({ families: ['settings'] });
    expect(client.supports('host_settings_get')).toBe(true);
  });

  it('delivers events, supports unlisten, and routes channel frames', async () => {
    const transport = new MemoryTransport();
    const listener = vi.fn();
    const unlisten = await transport.listen('host://ready', listener);
    transport.emit('host://ready', { ok: true });
    unlisten();
    transport.emit('host://ready', { ok: false });
    expect(listener).toHaveBeenCalledTimes(1);
    expect(listener).toHaveBeenCalledWith({ ok: true });

    const port = transport.channel<{ seq: number }>();
    const onmessage = vi.fn();
    port.onmessage = onmessage;
    expect(transport.sendChannel(port.id!, { seq: 1 })).toBe(true);
    expect(onmessage).toHaveBeenCalledWith({ seq: 1 });
    expect(transport.closeChannel(port.id!)).toBe(true);
    expect(transport.sendChannel(port.id!, { seq: 2 })).toBe(false);
  });

  it('FrameSink.dispose 注销通道（否则每次调用都漏一个不可回收的闭包图）', () => {
    // `closeChannel` 此前不在 `Backend` 接口上，因此**没有任何生产调用方**：
    // `MemoryTransport.channels` 只增不减。这条测试走的是真实释放路径
    // （`FrameSink.dispose` → `Backend.closeChannel`），不是直接调 closeChannel。
    const transport = new MemoryTransport();
    const sink = new FrameSink(() => {}, transport);
    const id = sink.port.id!;

    expect(transport.sendChannel(id, { seq: 1 })).toBe(true);
    sink.dispose();
    expect(transport.sendChannel(id, { seq: 2 }), '释放后通道必须不再收帧').toBe(false);
    // 幂等：重复释放不抛错（关流成功与开流失败两条路径都可能碰到）。
    expect(() => sink.dispose()).not.toThrow();
  });

  it('derives principal and command capabilities, and rejects absent handlers', async () => {
    const transport = new MemoryTransport({
      principal: { kind: 'plugin', id: 'p.demo' },
      commands: { host_ping: () => 'pong' },
    });
    expect(transport.pluginId()).toBe('p.demo');
    expect(transport.principal()).toEqual({ kind: 'plugin', id: 'p.demo' });
    await expect(transport.invoke('host_ping')).resolves.toBe('pong');
    await expect(transport.invoke('host_missing')).rejects.toThrow('command not found');
    expect(() => transport.register('invalid-command', () => undefined)).toThrow(
      'Invalid host command',
    );
  });
});
