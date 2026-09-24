// ──────────────────────────────────────────────────────────────────────────
// 宿主命令客户端：镜像计划 §2.1 的框架服务命令面。
//
// 约束（§4.9 / ADR-17）：
// - **唯一** `@tauri-apps/api` 引用点在 `tauri-backend.ts`；这里只依赖 {@link Backend}。
// - `self` 档命令的 pluginId 一律**不**由调用方传入，由宿主从 webview label 解析。
// - 二进制载荷走**帧**的 `argsRaw` + {@link ChannelPort}，禁止 base64 塞进 JSON（§4.8 R6）。
// - 每个方法都用 {@link translate_at_boundary} 把未知错误收口成 {@link HostException}（R2-c：边界显式）。
// ──────────────────────────────────────────────────────────────────────────
import type { Backend, ChannelPort, Principal } from './backend.js';
import { HOST_ERROR_CODES, translate_at_boundary } from './errors.js';
import type {
  EventFrame,
  EventSelector,
  JsonValue,
  PendingCallInfo,
  PluginDescriptor,
  RegistryAdminOp,
  Subscription,
  TopicDescriptor,
} from './events.js';
import type { StreamFrame, StreamKind } from './stream.js';
import type { LifecycleEvent } from './lifecycle.js';

/** 插件 → 自己 C/D 后端的调用请求。 */
export interface PluginCallRequest {
  /**
   * 调用方生成的关联 id（必填，宿主线格式要求）。
   *
   * **权威 callId 由宿主铸造**：宿主返回的 {@link PendingCallInfo.callId} 才是
   * 取消（{@link HostClient.cancel}）与终帧确认（{@link HostClient.callEnd}）
   * 应当使用的 id，本字段仅用于调用方关联。
   */
  callId: string;
  /** 目标方法名。 */
  method: string;
  /** JSON 载荷；与 {@link argsRaw} 二选一。 */
  argsJson?: JsonValue;
  /**
   * 二进制载荷；与 {@link argsJson} 二选一。
   *
   * **单独传入会抛 `TypeError`**：二进制通道（§4.8 R6）尚未接线，宿主线格式
   * 没有对应字段，静默丢弃会以 `null` 参数执行。与 `argsJson` 同时给出时以
   * `argsJson` 为准。
   */
  argsRaw?: Uint8Array;
  /**
   * 调用形态。闭集词表，与 Rust `HostPluginCallReq.kind` 校验一致
   * （非法值宿主返回 `E_AUTH_DENIED`）。
   *
   * 宿主不据此改变行为：`stream` 的帧由前端 Channel 承载。
   */
  kind: 'unary' | 'stream';
}

export interface HostClientOptions {
  backend: Backend;
}

/**
 * 贡献条目输入（self 档）。
 *
 * **没有 `pluginId` 字段**：署名只由宿主从 webview label 解析（§4.1），
 * 调用方给不了、给了也被线形忽略——否则任何插件都能把菜单/面板入口
 * 署名到别的插件。
 */
export interface ContributeEntryInput {
  kind: string;
  id: string;
  label: string;
}

/**
 * 宿主服务客户端。
 *
 * 实例不持有状态；所有方法都是"一次 invoke"，便于在插件代码里按需构造。
 */
export class HostClient {
  private readonly backend: Backend;

  constructor(options: HostClientOptions) {
    this.backend = options.backend;
  }

  private call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return this.backend
      .invoke<T>(cmd, args)
      .catch((err: unknown) => {
        // R2-c：宿主拒绝是**边界事件**，必须经显式翻译（而不是就地 new）。
        // 直接 new 会把「哪个边界、原始码是谁的词表」全部丢掉。
        throw translate_at_boundary(err, 'plugin-webview→host').error;
      });
  }

  // ── C/D 调用面（self 档，D2 定稿）───────────────────────────────

  /**
   * 调用本插件自己的 C/D 后端。
   *
   * 返回宿主受理后的 pending 簿记（{@link PendingCallInfo}）——其中
   * `callId` 是宿主铸造的**权威 id**，`cancel`/`callEnd` 必须用它。
   * `onFrame` 接收流式帧（{@link StreamFrame}）；二进制帧经 `argsRaw`
   * 直达（不经 base64），通道见 {@link FrameSink.port}。
   */
  async pluginCall(
    req: PluginCallRequest,
    onFrame?: (frame: StreamFrame) => void,
  ): Promise<PendingCallInfo> {
    // 二进制载荷没有线上表示：`argsRaw` 是**帧**的字段（`host_stream_write` 的
    // `argsRaw`，R5 已接线），不是调用参数的字段。**静默丢弃**会让调用以
    // `null` 参数执行，调用方以为传了载荷——宁可显式失败。
    // 与 `argsJson` 同时给出时按既有契约以 `argsJson` 为准（互斥由调用方保证）。
    if (req.argsRaw !== undefined && req.argsJson === undefined) {
      throw new TypeError(
        'pluginCall: 调用参数不支持二进制（argsRaw 属帧字段）：请用 argsJson，' +
          '或改用流式路径 HostRpc.stream / host_stream_write 的 argsRaw（R5 已接线）。',
      );
    }

    const channel = new FrameSink(onFrame, this.backend);
    return this.call<PendingCallInfo>('host_plugin_call', {
      req: {
        callId: req.callId,
        method: req.method,
        kind: req.kind,
        ...(req.argsJson !== undefined ? { argsJson: req.argsJson } : {}),
      },
      // 必须传**真实通道对象**：Rust 侧把它解析成 Channel 并登记为该调用的帧载体。
      channel: channel.port,
    });
  }

  /**
   * 订阅一次调用的流式帧（R5 / P0-1）。
   *
   * 与 {@link pluginCall} 的 `onFrame` 是两条入口：本方法为**已有**调用（通常是
   * 别处发起、或在重连后接管）新建通道并开流，返回的退订函数会**关流**。
   * 未带载体的调用在这里以 `E_CALL_NOT_FOUND` 显式失败——不会静默挂住。
   */
  openStream(callId: string, onFrame: (frame: StreamFrame) => void): () => void {
    const sink = new FrameSink(onFrame, this.backend);
    let streamId: string | null = null;
    let closed = false;
    const close = (): void => {
      if (closed) return;
      closed = true;
      if (streamId !== null) {
        void this.call('host_stream_close', { req: { streamId, kind: 'end' } }).catch(() => {
          // 关流失败不抛出：退订是尽力而为，句柄最终会被调用回收兜底。
        });
      }
    };
    void this.call<{ streamId: string }>('host_stream_open', {
      req: { callId },
      channel: sink.port,
    })
      .then((opened) => {
        streamId = opened.streamId;
        if (closed) close();
      })
      .catch(() => {
        // 开流失败（如未注册插件）：把失败留给调用方通过后续 write/callEnd 暴露，
        // 但退订仍是幂等的关流尝试。
        closed = true;
      });
    return close;
  }

  /** 传输无关的原始请求入口（{@link toHostRpc} 的 `request` 落地于此）。 */
  async request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return this.call<T>(cmd, args);
  }

  /** stream 模式的终帧确认。 */
  async callEnd(req: {
    callId: string;
    ok: boolean;
    errorCode?: string;
    seq?: number;
  }): Promise<void> {
    await this.call('host_call_end', {
      req: {
        callId: req.callId,
        ok: req.ok,
        ...(req.errorCode !== undefined ? { errorCode: req.errorCode } : {}),
        ...(req.seq !== undefined ? { seq: req.seq } : {}),
      },
    });
  }

  /**
   * 注册贡献（self 档：线形**不带** pluginId，宿主从 webview label 解析署名）。
   *
   * 与 `PluginDefinition.contributes` 的声明式配置配对——`createPlugin`
   * 激活时自动调用本方法。重复 `id` 宿主返回 `E_PLUGIN_EXISTS`；
   * 贡献是声明式附加物，声明方按 best-effort 处理（告警不阻断激活）。
   */
  async contributesRegister(entry: ContributeEntryInput): Promise<void> {
    await this.call('host_contributes_register', { entry });
  }

  /** 取消传播到 sidecar/supervisor。 */
  async cancel(callId: string): Promise<void> {
    await this.call('host_cancel', { callId });
  }

  // ── 生命周期（self 档）────────────────────────────────────────

  /**
   * 上报生命周期**事件**（不是状态）。
   *
   * 宿主是状态的唯一写入者（§4.3 单一写入者原则）：插件只能上报事件
   * （{@link LifecycleEvent}，SCREAMING_SNAKE_CASE 线名），由宿主决定迁移。
   * 上报状态线名会被宿主拒绝。
   */
  async lifecycleReport(evt: { event: LifecycleEvent; reason?: string }): Promise<void> {
    await this.call('host_lifecycle_report', {
      evt: {
        event: evt.event,
        ...(evt.reason !== undefined ? { reason: evt.reason } : {}),
      },
    });
  }

  // ── 事件总线（self 档，D1 定稿）────────────────────────────────

  /** 发布跨插件事件。唯一入口——不得绕过本方法直调官方 `emit`。 */
  async eventsPublish(
    evt: { topic: string; payload: JsonValue },
  ): Promise<{ delivered: number; dropped: boolean }> {
    return this.call('host_events_publish', {
      evt: { topic: evt.topic, payload: evt.payload },
    });
  }

  /** 订阅事件选择器。跨插件订阅需审批（§4.4）。 */
  async eventsSubscribe(subs: EventSelector[]): Promise<Subscription> {
    return this.call('host_events_subscribe', { sub: subs });
  }

  /**
   * 拉取本插件的待投递**事件帧**（订阅链路的取件步骤）。
   *
   * `host_events_subscribe` 只是把帧排进每订阅者队列；不调用本方法
   * 队列只进不出，订阅者永远收不到事件。`kind` 为 `event` / `request` /
   * `state`；`host_events_publish` 走可靠语义，帧在 `request` 通道。
   * 返回的是事件总线帧 {@link EventFrame}（topic/seq/payload）——不是
   * 流式帧 {@link StreamFrame}（后者经 Channel 的 onFrame 承载）。
   */
  async eventsDrain(kind: 'event' | 'request' | 'state' = 'request'): Promise<EventFrame[]> {
    return this.call('host_events_drain', { kind });
  }

  /**
   * 按 token 退订。
   *
   * 窗口销毁时的**隐式回收由宿主负责**：宿主需在 App Builder 的
   * `on_window_event(Destroyed)` 里调用
   * `tauron_adapter::tauri::cleanup_closed_window`（见 `examples/minimal-app`）。
   * 宿主未接线时，订阅会一直保留到卸载（`host_registry_admin`）或进程结束。
   */
  async eventsUnsubscribe(token: string): Promise<void> {
    await this.call('host_events_unsubscribe', { token });
  }

  // ── 注册表（scoped-read 档）──────────────────────────────────

  /**
   * 列出可见插件。
   *
   * 宿主按订阅关系过滤：自己 + 已订阅的 public topic 对应的插件（§2.1 scoped-read）。
   * `scope` 只是语义提示，**两种取值都不能突破调用方真实订阅**——可见性过滤由宿主
   * 按 webview 身份与事件总线订阅推导，调用方无法自报订阅来扩大可见范围。
   */
  async registryList(scope: 'public' | 'visible' = 'visible'): Promise<PluginDescriptor[]> {
    return this.call('host_registry_list', { scope });
  }

  /**
   * 当前调用方的身份主体（R4）。
   *
   * 主窗是一等主体（`{ kind: 'main-window' }`）而不是「`pluginId === null`」；
   * 畸形 `plugin-` label 归 `{ kind: 'invalid' }`，调用方应拒绝而不是按主窗放行。
   */
  get principal(): Principal {
    return this.backend.principal();
  }

  /**
   * **派生便利方法**：插件身份的 id；主窗等非插件主体返回 `null`。
   *
   * 唯一事实源是 {@link HostClient.principal}。
   */
  get pluginId(): string | null {
    const p = this.backend.principal();
    return p.kind === 'plugin' ? p.id : null;
  }
}

/**
 * 主窗特权命令客户端（§2.1 `host_registry_admin`，D15/D35）。
 *
 * **不得**被插件代码使用：该命令不在插件 capability 模板内，
 * 因此插件侧 invoke 会以 `E_AUTH_DENIED` 失败（门禁 §8-3）。
 */
export class AdminClient {
  private readonly backend: Backend;

  constructor(options: HostClientOptions) {
    this.backend = options.backend;
  }

  async registryAdmin(op: RegistryAdminOp): Promise<void> {
    await this.backend
      .invoke('host_registry_admin', { op })
      .catch((err: unknown) => {
        // R2-c：管理面同属插件 webview → 宿主的边界，走同一条显式翻译。
        throw translate_at_boundary(err, 'plugin-webview→host').error;
      });
  }
}

/**
 * FrameSink：把一次调用的**真实** Channel 端口与 `onFrame` 回调绑在一起。
 *
 * **R5 修正（重要）**：旧实现把 `FrameSink` **自己**当 `channel` 参数传给 invoke
 * （只暴露一个 `message` 字段），而 Rust 侧需要的是 Tauri `Channel` 的线值
 * （`Channel.toJSON()` → `__CHANNEL__:<id>`）。于是帧永远送不到前端，且
 * **不报错**——最难查的那类静默断链。现在：{@link FrameSink.port} 是必须原样
 * 传进 invoke 的通道对象，`onmessage` 由本类挂上以示真实收帧入口存在。
 */
export class FrameSink {
  /** 真实通道对象：必须**原样**作为 `channel` 参数传给 invoke。 */
  readonly port: ChannelPort<StreamFrame>;
  private readonly onFrame: ((frame: StreamFrame) => void) | undefined;

  constructor(
    onFrame: ((frame: StreamFrame) => void) | undefined,
    backend: Backend,
  ) {
    this.onFrame = onFrame;
    this.port = backend.channel<StreamFrame>();
    this.port.onmessage = (frame: StreamFrame) => {
      this.onFrame?.(frame);
    };
  }

  /** 测试注入：模拟宿主推送一帧（走与真机相同的 `onmessage` 入口）。 */
  sink(frame: StreamFrame): void {
    this.port.onmessage?.(frame);
  }
}

export { HOST_ERROR_CODES };
export type {
  EventFrame,
  EventSelector,
  JsonValue,
  PendingCallInfo,
  PluginDescriptor,
  RegistryAdminOp,
  StreamFrame,
  StreamKind,
  Subscription,
  TopicDescriptor,
};
export type { LifecycleEvent, LifecycleState } from './lifecycle.js';
