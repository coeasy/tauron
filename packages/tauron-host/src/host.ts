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
import type { StreamFrame, StreamHandle, StreamKind, StreamWriteInput } from './stream.js';
import type { PluginReportableEvent } from './lifecycle.js';

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
 * 贡献对账结果（`host_contributes_reconcile` 成功时的返回体，0.4-W3）。
 *
 * 只在**无分叉**时返回——有分叉时宿主抛 `E_CONTRIBUTES_DRIFT`，诊断细节在
 * 错误的 `message` 里。因此 `missing` / `extra` 按构造恒为空数组；保留字段是
 * 为了让调用方不必按分支读两种形状。
 */
export interface ContributesReconcileReport {
  pluginId: string;
  /** manifest 声明的贡献数（`kind:id` 去重后）。 */
  declared: number;
  /** activate 期实际注册的贡献数（`kind:id` 去重后）。 */
  registered: number;
  /** 声明了但没注册（成功时恒为空）。 */
  missing: string[];
  /** 注册了但没声明（成功时恒为空）。 */
  extra: string[];
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
    return this.backend.invoke<T>(cmd, args).catch((err: unknown) => {
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
    // 已知残留（诚实标注，**不是**遗忘）：这里的通道没有释放点，进程内传输
    // （`MemoryTransport` / `MockBackend`）会为每次 `pluginCall` 留一个条目。
    // 原因是**这一侧没有可靠的"调用已结束"信号**：帧载体是宿主在 `plugin_call`
    // 时绑定的，而流的终态由**写入方**发 `host_stream_close` 决定，发起方看不见。
    // 该调用若后续被 `openStreamHandle` 接管，宿主用的可能仍是这里绑定的载体——
    // 提前 `dispose()` 会把接管后的帧静默丢掉，比泄漏更糟。
    // `openStreamHandle` 那条路径没有这个问题（流的终态就在自己手里），已释放。
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
   *
   * 需要**写帧**或拿到 `streamId` 时用 {@link openStreamHandle}；本方法是它的
   * 简写（只要退订函数）。两者共用同一份实现，不存在第二套开流逻辑。
   */
  openStream(callId: string, onFrame: (frame: StreamFrame) => void): () => void {
    return this.openStreamHandle(callId, onFrame).close;
  }

  /**
   * 开流并返回可写的句柄（R5 / P0-1 的**完整**入口）。
   *
   * 补齐此前缺失的一段：`host_stream_write` 在 Rust 侧已注册、在
   * {@link pluginCall} 的错误文案里也被当作"已接线"的替代路径来推荐，
   * 但 TS SDK **没有任何方法能调到它**——开流之后既拿不到 `streamId`、
   * 也没有写帧出口，`open → write → close` 这条链在前端是断的。
   *
   * `write()` 支持 `argsRaw`：字节不经 base64 夹带 JSON（§4.8 R6）。
   * `close()` 幂等；**在开流落定之前调用也安全**——关流命令会在 `streamId`
   * 到手后补发，不会留下无人关闭的宿主侧流。句柄最终还有调用回收兜底。
   */
  openStreamHandle(callId: string, onFrame: (frame: StreamFrame) => void): StreamHandle {
    const sink = new FrameSink(onFrame, this.backend);
    let streamId: string | null = null;
    // 「已请求关闭」与「关闭已送出」必须分开：开流是异步的，调用方完全可能在
    // `ready` 落定**之前**就 `close()`（例如开流后立刻取消）。若只用一个
    // `closed` 标志，close() 会因 `streamId` 还是 null 而不发命令，随后开流
    // 落定时又因 `closed` 已为 true 而提前返回——`host_stream_close` **永远
    // 不会发出**，宿主侧那条流就泄漏了。两个标志各自幂等即可闭合这条路径。
    let closeRequested = false;
    let closeSent = false;

    /** 幂等送出 `host_stream_close`（仅在「已请求关闭」且 `streamId` 已知时才有意义）。 */
    const sendClose = (): void => {
      if (!closeRequested || closeSent || streamId === null) return;
      closeSent = true;
      void this.call('host_stream_close', { req: { streamId, kind: 'end' } })
        .then(() => {
          // 关流成功 = 流已终态，之后不会再有帧 → 通道可以释放（见 FrameSink.dispose）。
          sink.dispose();
        })
        .catch(() => {
          // 关流失败不抛出：退订是尽力而为，句柄最终会被调用回收兜底。
          // **不释放通道**——流可能仍在推帧，清了 onmessage 会把后续帧静默丢掉。
        });
    };

    const close = (): void => {
      closeRequested = true;
      sendClose();
    };

    const ready = this.call<{ streamId: string }>('host_stream_open', {
      req: { callId },
      channel: sink.port,
    })
      .then((opened) => {
        streamId = opened.streamId;
        // 补发：调用方可能在开流落定前就要求关闭（见上面 `closeRequested` 注释）。
        sendClose();
        return opened.streamId;
      })
      .catch((err: unknown) => {
        // 开流失败（如未注册插件）：把失败留给调用方通过后续 write/close 暴露，
        // 但退订仍是幂等的关流尝试。
        closeRequested = true;
        // 流从未存在 → 不会有帧到达 → 通道立刻释放（否则这条失败路径每次都漏一个）。
        sink.dispose();
        throw err;
      });

    // `openStream()`（只要退订函数的简写入口）此前**吞掉**开流失败，保持"退订
    // 永远可用"。现在 `ready` 会 reject 以便 `write()` 如实报错，但没人 await 它时
    // 就成了 unhandled rejection——这里补一个空 catch 兜住这个副作用，
    // `ready` 本身仍然 reject（`write()` 依赖它拿到真实失败原因）。
    void ready.catch(() => {});

    const write = (frame: StreamWriteInput): Promise<StreamFrame> => {
      if (closeRequested) {
        return Promise.reject(new Error('openStreamHandle: 流已关闭，不得再写帧'));
      }
      return ready.then((id) =>
        this.call<StreamFrame>('host_stream_write', {
          req: {
            streamId: id,
            ...(frame.argsJson !== undefined ? { argsJson: frame.argsJson } : {}),
            ...(frame.argsRaw !== undefined ? { argsRaw: frame.argsRaw } : {}),
          },
        }),
      );
    };

    return { ready, write, close };
  }

  /**
   * 向已开的流写一帧（`host_stream_write`）。
   *
   * {@link openStreamHandle} 的 `write()` 就是转调本方法；需要凭 `streamId`
   * 直接写（例如句柄来自别处）时用这个。
   */
  async writeStream(streamId: string, frame: StreamWriteInput = {}): Promise<StreamFrame> {
    return this.call<StreamFrame>('host_stream_write', {
      req: {
        streamId,
        ...(frame.argsJson !== undefined ? { argsJson: frame.argsJson } : {}),
        ...(frame.argsRaw !== undefined ? { argsRaw: frame.argsRaw } : {}),
      },
    });
  }

  /** 传输无关的原始请求入口（{@link toHostRpc} 的 `request` 落地于此）。 */
  async request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return this.call<T>(cmd, args);
  }

  // ── 跨主体调用（0.4-A1）───────────────────────────────────────

  /**
   * 发起一次**跨主体**调用（宿主 → 插件 / 插件 → 插件，0.4-A1）。
   *
   * 与 {@link pluginCall}（调自己的 C/D 后端）不同，本方法的执行方是**另一个
   * 插件**：宿主按目标插件的形态选投递通路（js → 事件总线 request 通道、
   * process → sidecar stdin 帧回路），无通路时以结构化 `Unsupported` 失败。
   * 发起主体由宿主从 webview label 解析（防冒充），主窗即 `"main"`。
   *
   * 返回宿主铸造的权威簿记：`takeCallResult` 用其中的 `callId` 取件。
   */
  async callPlugin(target: string, method: string, argsJson?: JsonValue): Promise<PendingCallInfo> {
    return this.call<PendingCallInfo>('host_call_plugin', {
      req: {
        target,
        method,
        ...(argsJson !== undefined ? { argsJson } : {}),
      },
    });
  }

  /**
   * 执行方回填一次跨主体调用的结果（0.4-A1 的结算入口，self 档）。
   *
   * 只有该调用的 `target` 插件能回填（宿主校验身份 == target）；对已结算的
   * 调用重复回填返回 `E_CALL_ALREADY_SETTLED`（幂等拒绝，不覆盖）。
   */
  async reportCallResult(req: {
    callId: string;
    ok: boolean;
    result?: JsonValue;
    errorCode?: string;
  }): Promise<PendingCallInfo> {
    return this.call<PendingCallInfo>('host_call_result', {
      req: {
        callId: req.callId,
        ok: req.ok,
        ...(req.result !== undefined ? { result: req.result } : {}),
        ...(req.errorCode !== undefined ? { errorCode: req.errorCode } : {}),
      },
    });
  }

  /**
   * 发起方取走一次已结算的结果（0.4-A1 的回执取件，self 档）。
   *
   * 只有发起方（`call.caller`）能取。`settled` 取走即删（一次性语义）；
   * `pending` 返回副本、条目保留（调用方据此知道「还没好」，可稍后再取）。
   */
  async takeCallResult(callId: string): Promise<PendingCallInfo> {
    return this.call<PendingCallInfo>('host_call_take', { req: { callId } });
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

  /**
   * 对账「manifest 声明」与「activate 期注册」（self 档，0.4-W3）。
   *
   * 为什么需要：`contributesRegister` 只是往宿主表里增删，**没有任何东西比对
   * 声明与事实**——声明了却漏注册（入口点了没反应）与注册了却没声明（来源不明
   * 的入口）都无人发现。本方法把这份落差变成可检出的错误。
   *
   * 纯只读（不改状态）。无分叉返回报告；有分叉宿主抛
   * `E_CONTRIBUTES_DRIFT`（诊断细节在 `message` 里）。身份由宿主从 label 解析，
   * 因此**只能对账自己**。
   */
  async contributesReconcile(): Promise<ContributesReconcileReport> {
    return await this.call<ContributesReconcileReport>('host_contributes_reconcile', {});
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
   * （SCREAMING_SNAKE_CASE 线名），由宿主决定迁移。上报状态线名会被宿主拒绝。
   *
   * 事件类型收窄为 {@link PluginReportableEvent}（不是完整的 {@link LifecycleEvent}）：
   * `host_lifecycle_report` 是 self 档，插件不得自报 `ENABLE`/`SAFEMODE_EXIT`/
   * `UNINSTALL`/`INSTALL_OK` 等越权事件——Rust 侧以 `E_AUTH_DENIED` 硬拒，这里
   * 在编译期就把它们挡在类型外，让越权调用根本写不出来。用户放行走
   * `AdminClient.registryAdmin({ op: 'ENABLE' })`。
   */
  async lifecycleReport(evt: { event: PluginReportableEvent; reason?: string }): Promise<void> {
    await this.call('host_lifecycle_report', {
      evt: {
        event: evt.event,
        ...(evt.reason !== undefined ? { reason: evt.reason } : {}),
      },
    });
  }

  // ── 事件总线（self 档，D1 定稿）────────────────────────────────

  /** 发布跨插件事件。唯一入口——不得绕过本方法直调官方 `emit`。 */
  async eventsPublish(evt: {
    topic: string;
    payload: JsonValue;
  }): Promise<{ delivered: number; dropped: boolean }> {
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
    await this.backend.invoke('host_registry_admin', { op }).catch((err: unknown) => {
      // R2-c：管理面同属插件 webview → 宿主的边界，走同一条显式翻译。
      throw translate_at_boundary(err, 'plugin-webview→host').error;
    });
  }

  /** Install a verified local package after the host UI has shown and collected approval. */
  async registryInstall(
    packagePath: string,
    approvedPermissions: string[],
    reviewToken: string,
  ): Promise<{
    pluginId: string;
    version: string;
    installPath: string;
    approvedPermissions: string[];
  }> {
    return this.backend
      .invoke<{
        pluginId: string;
        version: string;
        installPath: string;
        approvedPermissions: string[];
      }>('host_registry_install', { packagePath, reviewToken, approvedPermissions })
      .catch((err: unknown) => {
        throw translate_at_boundary(err, 'plugin-webview→host').error;
      });
  }

  async registryInstallPreview(packagePath: string): Promise<{
    pluginId: string;
    pluginName: string;
    version: string;
    reviewToken: string;
    packageDigest: string;
    permissions: Array<{
      permission: string;
      risk: string;
      description: string;
      defaultChecked: boolean;
    }>;
  }> {
    return this.backend
      .invoke<{
        pluginId: string;
        pluginName: string;
        version: string;
        reviewToken: string;
        packageDigest: string;
        permissions: Array<{
          permission: string;
          risk: string;
          description: string;
          defaultChecked: boolean;
        }>;
      }>('host_registry_install_preview', { packagePath })
      .catch((err: unknown) => {
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
  private readonly backend: Backend;

  constructor(onFrame: ((frame: StreamFrame) => void) | undefined, backend: Backend) {
    this.onFrame = onFrame;
    this.backend = backend;
    this.port = backend.channel<StreamFrame>();
    this.port.onmessage = (frame: StreamFrame) => {
      this.onFrame?.(frame);
    };
  }

  /** 测试注入：模拟宿主推送一帧（走与真机相同的 `onmessage` 入口）。 */
  sink(frame: StreamFrame): void {
    this.port.onmessage?.(frame);
  }

  /**
   * 释放通道（幂等）：清掉收帧入口，并在传输层支持时注销该通道。
   *
   * **只能在确认不会再有帧到达之后调用**——清了 `onmessage` 之后到达的帧会被
   * 静默丢弃（`sendChannel` 返回 `false`），这正是「静默断链」最难查的形态。
   * 目前的调用点只有「关流成功」（流已终态）与「开流失败」（流从未存在）。
   *
   * 为什么必须显式释放：进程内传输（`MemoryTransport` / `MockBackend`）的通道
   * 存在自己的 Map 里，而 `onmessage` 闭包强引用整条流的状态——不释放就是每次
   * 调用累积一个不可回收的对象图。真实 Tauri `Channel` 由 Tauri 管生命周期，
   * 其 `Backend` 不实现 `closeChannel`，此处为 no-op（口径见 `Backend.closeChannel`）。
   */
  dispose(): void {
    this.port.onmessage = null;
    const id = this.port.id;
    if (id !== undefined) this.backend.closeChannel?.(id);
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
