// ──────────────────────────────────────────────────────────────────────────
// Backend provider 抽象（ADR-16）。
//
// core 的全部运行期逻辑只依赖这个接口；`@tauri-apps/api` 的真实实现被隔离在
// `tauri-backend.ts` 单一文件里（门禁 §8-1：本包内唯一 import 点）。
//
// MockBackend **仅供契约测试**，不作为 Web 交付（§4.9 关键约束）。
// ──────────────────────────────────────────────────────────────────────────

import { parseStreamKind } from './stream.js';
import type { StreamFrame, StreamKind } from './stream.js';

/**
 * Tauri `Channel<T>` 的消息端口形状（避免 core 直接依赖 @tauri-apps/api）。
 *
 * **R5 修正**：此前这里要求 `message: MessagePort`，而真实 Tauri `Channel` 上
 * 根本没有该字段（`tauri-backend` 用 `as unknown as` 把这个不一致藏了起来）。
 * 于是「把 `FrameSink` 自己当 channel 传进 invoke」在类型上看着成立、在线上却
 * 不成立——Rust 侧收到的是一个普通对象，帧永远送不出去，且不报错。
 * 现在只声明真实 Channel 真正拥有的东西：`onmessage`（收帧入口）与 `id`。
 */
export interface ChannelPort<T = unknown> {
  /** 宿主推送帧的入口（Tauri `Channel.onmessage`）。 */
  onmessage?: ((payload: T) => void) | null;
  /** 通道标识（Tauri `Channel.id`）；mock 用自增 id，用于断言"传的是哪一个通道"。 */
  readonly id?: string | number;
}

/** 订阅句柄：resolve 返回退订函数。 */
export type Unlisten = () => void;

/**
 * 调用方的身份主体（R4 / D1）。
 *
 * 此前身份模型只有 `pluginId(): string | null`，主窗只能表达为「`null`」——
 * 它的特权来自「没有身份」，既无法给主窗分级（kiosk / 受限主窗），也无法表达
 * 「主窗绑定在哪个 origin / lease 上」。
 *
 * 与 Rust 侧 `tauron_host::authz::Principal` 对应：
 * - {@link Principal} `'plugin'` ↔ `Principal::Plugin`
 * - `'main-window'` ↔ `Principal::MainWindow`
 * - `'invalid'` ↔ `Principal::Invalid`（畸形 `plugin-` label，**不得**按主窗放行）
 *
 * `origin` 是 webview **自报**值，仅用于诊断/展示；真正的 origin 强制在宿主侧
 * 用 `WebviewWindow::url()` 校验（webview 自报不可信）。
 */
export type Principal =
  | { kind: 'plugin'; id: string }
  | { kind: 'main-window'; origin: string | null }
  | { kind: 'invalid'; label: string };

/**
 * 宿主能力编排层所需的最小底层接口。
 *
 * 真实实现见 {@link TauriBackend}；测试用 {@link MockBackend}。
 */
export interface Backend {
  /** 调用宿主命令；`cmd` 为**已加前缀**的完整命令名。 */
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;

  /** 监听宿主/webview 事件。 */
  listen(event: string, handler: (payload: unknown) => void): Promise<Unlisten>;

  /** 创建流式通道（C/D 类调用的 progress/cancel 帧载体）。 */
  channel<T = unknown>(): ChannelPort<T>;

  /**
   * 当前调用方的身份主体（R4）。
   *
   * 计划 §2.1：`self` 档命令的身份**只**从 webview label 解析（`plugin-<id>`），
   * 调用方不得自行传入（防跨插件冒充）。
   */
  principal(): Principal;

  /**
   * **派生便利方法**：身份为插件时返回其 id，否则 `null`。
   *
   * R4 之前这是唯一身份访问器；现在它的唯一事实源是 {@link Backend.principal}，
   * 主窗由 `{ kind: 'main-window' }` 显式表达而不是「`null`」。
   */
  pluginId(): string | null;

  /**
   * 该 Backend 可用的框架服务命令集合（未加前缀的裸命令名）。
   *
   * 用于 {@link capabilities} 的探测：命令未注册时 `available()` 为 `false`，
   * 而不是等到 invoke 失败才暴露。
   */
  capabilities(): ReadonlySet<string>;
}

// ──────────────────────────────────────────────────────────────────────────
// 契约测试用 mock（不进入交付产物）
// ──────────────────────────────────────────────────────────────────────────

export interface MockInvokeCase {
  /** 完整命令名。 */
  cmd: string;
  /** 匹配参数（子集匹配）。缺省则匹配任意参数。 */
  args?: Record<string, unknown>;
  /** 返回 Promise resolve 该值。 */
  result?: unknown;
  /** 或 reject 该错误（优先于 result）。 */
  error?: unknown;
}

export interface MockBackendOptions {
  /** 预置的 invoke 行为。按顺序取第一条 args 子集匹配的用例。 */
  cases?: MockInvokeCase[];
  /** 可用命令集合。 */
  capabilities?: string[];
  /** 身份主体（R4）。优先于 `pluginId`。 */
  principal?: Principal;
  /** 插件身份 id 的便捷写法（等价于 `principal: { kind: 'plugin', id }`）。 */
  pluginId?: string | null;
}

/**
 * 记录式 mock Backend。
 *
 * 每次 `invoke` 都会追加到 {@link invocations}，便于断言"调了哪个命令、带什么参数"。
 */
export class MockBackend implements Backend {
  readonly invocations: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
  readonly subscriptions = new Map<string, Set<(p: unknown) => void>>();

  private readonly cases: MockInvokeCase[];
  private readonly caps: ReadonlySet<string>;
  private readonly principalValue: Principal;

  constructor(options: MockBackendOptions = {}) {
    this.cases = options.cases ?? [];
    this.caps = new Set(options.capabilities ?? []);
    this.principalValue =
      options.principal ??
      (options.pluginId != null
        ? { kind: 'plugin', id: options.pluginId }
        : { kind: 'main-window', origin: null });
  }

  async invoke<T = unknown>(
    cmd: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    this.invocations.push(args === undefined ? { cmd } : { cmd, args });
    // 与真实 Tauri 行为一致：命令未注册 → "command not found"。
    if (!this.caps.has(cmd)) {
      throw new Error(`command not found: ${cmd}`);
    }
    // 流式命令由内核直接处理（R5）：不预置 case 也必须有真实行为，否则
    // 「mock 通过、真机不通过」——那正是 R5 之前 channel 断链的翻版。
    const streamed = this.runStreamKernel(cmd, args);
    if (streamed) return streamed.value as T;
    const hit = this.findCase(cmd, args);
    if (hit?.error !== undefined) throw hit.error;
    if (hit) return hit.result as T;
    // 已注册但无预置行为：视作成功返回空（宿主命令多数是 void）。
    return undefined as T;
  }

  /** 按顺序取第一条 args 子集匹配的用例。 */
  private findCase(cmd: string, args?: Record<string, unknown>): MockInvokeCase | undefined {
    return this.cases.find((c) => {
      if (c.cmd !== cmd) return false;
      if (!c.args) return true;
      return Object.entries(c.args).every(
        ([k, v]) => args && JSON.stringify(args[k]) === JSON.stringify(v),
      );
    });
  }

  // ── 流式内核（R5）────────────────────────────────────────────────────
  //
  // 语义与 Rust `tauron_host::stream` 对齐：宿主铸 `seq`、终帧后句柄失效、
  // 未知句柄报错、帧经**调用方传入的** Channel 派发。这样 HostRpc 的 mock
  // 往返与真机往返走的是同一条调用序列。

  private readonly channels = new Map<string, ChannelPort<StreamFrame>>();
  private readonly callChannels = new Map<string, string>();
  private readonly streams = new Map<string, { callId: string; seq: number; closed: boolean }>();
  private nextChannelId = 1;
  private nextStreamId = 1;

  /** 返回 `undefined` = 不是流式命令（继续走常规 invoke 逻辑）。 */
  private runStreamKernel(
    cmd: string,
    args?: Record<string, unknown>,
  ): { value: unknown } | undefined {
    if (cmd !== 'host_plugin_call' && !cmd.startsWith('host_stream_')) return undefined;
    const req = (args?.req ?? {}) as Record<string, unknown>;
    const passed = args?.channel as ChannelPort<StreamFrame> | undefined;

    if (cmd === 'host_plugin_call') {
      // 带 channel 的调用：登记「callId → 通道」，后续 write 经它派发。
      //
      // 键必须是**宿主铸的** callId：真机上 `stream_bind` 用的就是 `PendingCall`
      // 里的 id，而不是调用方自报的 `req.callId`（后者只是关联用途）。mock 因此
      // 也要取预置结果里的 callId，否则「调用一个 id、开流另一个 id」这种在真机
      // 上不可能发生的事，会在 mock 里假装发生。
      if (passed?.id !== undefined) {
        const preset = this.findCase(cmd, args)?.result as { callId?: unknown } | undefined;
        const authoritative =
          typeof preset?.callId === 'string' ? preset.callId : String(req.callId ?? '');
        this.callChannels.set(authoritative, String(passed.id));
      }
      return undefined; // 调用本身的返回交给 cases/默认逻辑
    }

    if (cmd === 'host_stream_open') {
      const callId = String(req.callId ?? '');
      if (passed?.id !== undefined) {
        this.callChannels.set(callId, String(passed.id));
      }
      if (!this.callChannels.has(callId)) {
        throw new Error(`E_CALL_NOT_FOUND: 调用 \`${callId}\` 没有帧载体`);
      }
      const streamId = `st-mock-${this.nextStreamId++}`;
      this.streams.set(streamId, { callId, seq: 0, closed: false });
      return { value: { streamId, callId } };
    }

    const streamId = String(req.streamId ?? '');
    const state = this.streams.get(streamId);
    if (!state) {
      throw new Error(`E_CALL_NOT_FOUND: 流 \`${streamId}\` 不存在或已终结`);
    }
    if (state.closed) {
      throw new Error(`E_CALL_NOT_FOUND: 流 \`${streamId}\` 已终结（终帧后句柄失效）`);
    }
    const kind = cmd === 'host_stream_close' ? parseStreamKind(req.kind) : 'data';
    if (cmd === 'host_stream_close' && (kind === null || kind === 'data')) {
      throw new Error(`E_INVALID_MANIFEST: 关流只接受终帧种类（end/error），收到 \`${String(req.kind)}\``);
    }
    const frame: StreamFrame = {
      seq: ++state.seq,
      kind: kind as StreamKind,
      ...(req.argsJson !== undefined ? { argsJson: req.argsJson } : {}),
      ...(req.argsRaw !== undefined ? { argsRaw: req.argsRaw as Uint8Array } : {}),
    };
    if (frame.kind !== 'data') state.closed = true;
    const channelId = this.callChannels.get(state.callId);
    const channel = channelId !== undefined ? this.channels.get(channelId) : undefined;
    channel?.onmessage?.(frame);
    return { value: frame };
  }

  async listen(
    event: string,
    handler: (payload: unknown) => void,
  ): Promise<Unlisten> {
    let set = this.subscriptions.get(event);
    if (!set) {
      set = new Set();
      this.subscriptions.set(event, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  }

  /** emit：模拟宿主向已订阅 handler 派发事件（测试专用辅助）。 */
  emit(event: string, payload: unknown): void {
    this.subscriptions.get(event)?.forEach((h) => h(payload));
  }

  channel<T = unknown>(): ChannelPort<T> {
    // mock 通道**真的**能收帧（R5）：流式内核经 `onmessage` 派发，
    // 与真实 Tauri Channel 同一入口，因此 mock 往返能验证整条链路。
    const id = `ch-${this.nextChannelId++}`;
    const port: ChannelPort<T> = { id, onmessage: null };
    this.channels.set(id, port as ChannelPort<StreamFrame>);
    return port;
  }

  principal(): Principal {
    return this.principalValue;
  }

  pluginId(): string | null {
    const p = this.principalValue;
    return p.kind === 'plugin' ? p.id : null;
  }

  capabilities(): ReadonlySet<string> {
    return this.caps;
  }
}
