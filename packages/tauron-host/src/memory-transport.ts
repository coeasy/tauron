import type { ChannelPort, HostTransport, Principal, Unlisten } from './backend.js';

export type MemoryCommandHandler = (args?: Record<string, unknown>) => unknown | Promise<unknown>;

export interface MemoryTransportOptions {
  commands?: Record<string, MemoryCommandHandler>;
  principal?: Principal;
  capabilities?: string[];
}

/** 纯进程内宿主传输，供非 Tauri 壳和嵌入式宿主复用相同客户端协议。 */
export class MemoryTransport implements HostTransport {
  private readonly commands = new Map<string, MemoryCommandHandler>();
  private readonly caps: Set<string>;
  private readonly principalValue: Principal;
  private readonly listeners = new Map<string, Set<(payload: unknown) => void>>();
  private readonly channels = new Map<string, ChannelPort<unknown>>();
  private nextChannelId = 1;

  constructor(options: MemoryTransportOptions = {}) {
    this.principalValue = options.principal ?? { kind: 'main-window', origin: 'memory://host' };
    this.caps = new Set(options.capabilities ?? Object.keys(options.commands ?? {}));
    for (const [command, handler] of Object.entries(options.commands ?? {})) {
      if (!/^host_[a-z0-9_]+$/.test(command)) throw new TypeError(`Invalid host command: ${command}`);
      this.commands.set(command, handler);
    }
  }

  register(command: string, handler: MemoryCommandHandler): void {
    if (!/^host_[a-z0-9_]+$/.test(command)) throw new TypeError(`Invalid host command: ${command}`);
    if (this.commands.has(command)) throw new Error(`Command already registered: ${command}`);
    this.commands.set(command, handler);
    this.caps.add(command);
  }

  async invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    if (!this.caps.has(cmd)) throw new Error(`command not found: ${cmd}`);
    const handler = this.commands.get(cmd);
    if (!handler) throw new Error(`No memory handler registered: ${cmd}`);
    return (await handler(args)) as T;
  }

  async listen(event: string, handler: (payload: unknown) => void): Promise<Unlisten> {
    let group = this.listeners.get(event);
    if (!group) this.listeners.set(event, (group = new Set()));
    group.add(handler);
    return () => {
      group!.delete(handler);
      if (group!.size === 0) this.listeners.delete(event);
    };
  }

  emit(event: string, payload: unknown): void {
    for (const listener of [...(this.listeners.get(event) ?? [])]) listener(payload);
  }

  channel<T = unknown>(): ChannelPort<T> {
    const id = `memory-${this.nextChannelId++}`;
    const port: ChannelPort<unknown> = { id };
    this.channels.set(id, port);
    return port as ChannelPort<T>;
  }

  sendChannel<T = unknown>(id: string | number, payload: T): boolean {
    const port = this.channels.get(String(id));
    if (!port?.onmessage) return false;
    port.onmessage(payload);
    return true;
  }

  closeChannel(id: string | number): boolean {
    return this.channels.delete(String(id));
  }

  principal(): Principal {
    return this.principalValue;
  }

  pluginId(): string | null {
    return this.principalValue.kind === 'plugin' ? this.principalValue.id : null;
  }

  capabilities(): ReadonlySet<string> {
    return this.caps;
  }
}
