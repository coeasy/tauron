import { describe, expect, it, vi } from 'vitest';
import { MemoryTransport } from './memory-transport.js';
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

  it('derives principal and command capabilities, and rejects absent handlers', async () => {
    const transport = new MemoryTransport({
      principal: { kind: 'plugin', id: 'p.demo' },
      commands: { host_ping: () => 'pong' },
    });
    expect(transport.pluginId()).toBe('p.demo');
    expect(transport.principal()).toEqual({ kind: 'plugin', id: 'p.demo' });
    await expect(transport.invoke('host_ping')).resolves.toBe('pong');
    await expect(transport.invoke('host_missing')).rejects.toThrow('command not found');
    expect(() => transport.register('invalid-command', () => undefined)).toThrow('Invalid host command');
  });
});
