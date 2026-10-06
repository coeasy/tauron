// ──────────────────────────────────────────────────────────────────────────
// 事件契约类型（§4.4 总线 / §4.9-3 越界检查的 JS 侧形状）。
//
// 跨插件事件发布的**唯一通道**是 `host_events_publish`（计划 D1）。
// 插件不得直调官方 `emit` 做跨插件发布——否则绕过总线授权。
// ──────────────────────────────────────────────────────────────────────────

/** JSON 值（跨 IPC 的载荷只能是 JSON）。 */
export type JsonValue =
  null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

/**
 * 事件主题命名约定：`<插件id>.<名字>`，最长 200 字符。
 *
 * 这个数字此前只是写在类型旁边的约定——两侧都不强制它。轮 64 起强制点在 Rust
 * `tauron-host::eventbus` 的 `MAX_TOPIC_NAME_LENGTH`（声明期硬拒超长名），wire-gate
 * 逐轮把两侧数字对钉，改一侧不改另一侧会当场红。长度按字符数计，非 ASCII 名的
 * UTF-16 计数更大，两种口径不必等同——将来前端也接强制点时别当成同口径。
 */
export const TOPIC_MAX_LENGTH = 200;

export interface TopicDescriptor {
  topic: string;
  /** 是否对其他插件可见。false = 私有（仅本插件可订阅）。 */
  public: boolean;
}

/** 订阅选择器。 */
export interface EventSelector {
  topic: string;
}

/** 宿主分配的订阅 token。 */
export interface Subscription {
  token: string;
  selectors: EventSelector[];
}

/** publish 结果。`dropped` = 越界（未声明 publish 权限）被静默丢弃并计数。 */
export interface PublishResult {
  delivered: number;
  dropped: boolean;
}

/**
 * 事件总线帧（`host_events_drain` 的返回单元，Rust `Frame` 的线形）。
 *
 * 与流式帧 {@link StreamFrame} 是**两种帧**：事件帧经总线队列取件（topic 事件
 * 投递，拉模型），流式帧经 Channel 承载（一次调用的连续输出）。
 */
export interface EventFrame {
  /** 事件 topic（约定 `plugin:<插件id>:<事件名>`，与框架层 `eventNamespace` 一致）。 */
  topic: string;
  /** V4 A102 per sender→receiver monotonic sequence. */
  seq: number;
  /** JSON 载荷。 */
  payload: JsonValue;
  /** V4 A102 sender principal. */
  sender?: string;
  /** V4 A102 receiver principal. */
  receiver?: string;
  /** V4 A102 monotonic state revision for state-channel publications. */
  stateRevision?: number;
  /** V4 A78 event id. Always present on V4 hosts; optional for N-1 source compatibility. */
  eventId?: string;
  /** V4 A78 root causation id shared by the full event chain. */
  causationId?: string;
  /** V4 A78 1-based event hop. */
  eventHop?: number;
  /** V4 A78 Host-enforced maximum causation depth. */
  maxCausationDepth?: number;
}

/**
 * V4 A102 接收端顺序违规（{@link EventOrderingWatcher.observe} 的判定结果）。
 *
 * 三种判据与 Rust `tauron_host::OrderingError` 一一对应。
 */
export type EventOrderingViolation =
  | { kind: 'duplicate'; sender: string; receiver: string; seq: number }
  | { kind: 'gap'; sender: string; receiver: string; expected: number; actual: number }
  | { kind: 'revision-regression'; previous: number; actual: number };

/**
 * 接收端顺序守卫（A102）：宿主为每条 `sender → receiver` 流铸造从 1 起的单调
 * `seq`，本类把它在**消费侧**用起来——重复投递、丢帧、状态倒退都能当场判出来。
 *
 * 为什么需要它：此前 `seq`/`sender`/`receiver` 只是**字段**，取件泵把帧交给业务
 * 时就再没人看过顺序。可靠通道是 at-least-once（重试可能重投），无接收端去重等于
 * 「同一事件被处理两次且无从发现」。
 *
 * 与 Rust 侧 {@link OrderingTracker}（conformance oracle）的一处**有意分歧**：
 * oracle 遇到 gap 后不重同步，于其后每一帧都继续报 gap——那是验收视图，异常要全部
 * 可见。运行视图正相反：一次丢帧若永久毒化这条流，后续每帧都成噪音，真正的重复/
 * 倒退反而被埋掉。因此这里报告一次后把期望值抬到 `seq + 1`。
 *
 * 没有 `sender`/`receiver` 的帧（N-1 宿主）不参与判定：判定依据都不存在。
 *
 * **只做可见性，不替业务丢帧**：可靠通道是 at-least-once，但「收到两次该不该再执行
 * 一次」是业务的幂等语义，不是底座的；且宿主重启后 `seq` 会从 1 重新铸造，按 `seq`
 * 丢帧会把一条正常的新流全部吃掉。真正的去重键是 A78 `eventId`，登记为后续项。
 */
export class EventOrderingWatcher {
  private readonly expectedByStream = new Map<string, number>();
  private readonly latestRevisionByStream = new Map<string, number>();

  /** 丢弃全部已观察的流状态（宿主重连/换会话时调用，避免跨会话误判重复）。 */
  reset(): void {
    this.expectedByStream.clear();
    this.latestRevisionByStream.clear();
  }

  /** 观察一帧；返回 `null` 表示顺序正常。 */
  observe(frame: EventFrame): EventOrderingViolation | null {
    const { sender, receiver } = frame;
    if (sender == null || receiver == null) return null;
    const key = `${sender}\u0000${receiver}`;
    const expected = this.expectedByStream.get(key) ?? 1;

    if (frame.seq < expected) {
      return { kind: 'duplicate', sender, receiver, seq: frame.seq };
    }
    if (frame.seq > expected) {
      // 报告一次即重同步：丢的那帧回不来，但后续帧必须能回到正常判定。
      this.expectedByStream.set(key, frame.seq + 1);
      this.trackRevision(key, frame.stateRevision);
      return { kind: 'gap', sender, receiver, expected, actual: frame.seq };
    }
    this.expectedByStream.set(key, expected + 1);
    return this.trackRevision(key, frame.stateRevision);
  }

  private trackRevision(key: string, revision?: number): EventOrderingViolation | null {
    if (revision == null) return null;
    const previous = this.latestRevisionByStream.get(key);
    if (previous != null && revision < previous) {
      // 不回退记录值：倒退之后的帧仍应按已知最新 revision 判定。
      return { kind: 'revision-regression', previous, actual: revision };
    }
    this.latestRevisionByStream.set(key, revision);
    return null;
  }
}

// R5 收敛：此前的 `CallFrame`（`kind: 'response'|'progress'|'cancel'`，
// `value`/`data`/`errorCode`）与 Rust 侧的帧类型**不是同一个词表**——前端按
// `CallFrame` 写、宿主按 `StreamFrame` 发，两边都「有类型」却对不上。现在只有
// 一份帧类型（`StreamFrame`，`data|end|error` + `argsJson`/`argsRaw`），
// 与 `tauron_host::stream::StreamFrame` 同构，由 wire-gate 锁死。

/**
 * 宿主受理调用后返回的 pending 簿记（Rust `PendingCall` 的线形）。
 *
 * **权威 `callId` 由宿主铸造**（Rust `Uuid::new_v4`）；调用方传入的
 * {@link PluginCallRequest.callId} 仅作关联用途。取消（`HostClient.cancel`）与
 * 终帧确认（`HostClient.callEnd`）都必须使用本返回值里的 `callId`。
 * 调用结果不在本返回值里：stream 帧经 Channel 送达 `onFrame`。
 */
export interface PendingCallInfo {
  callId: string;
  pluginId: string;
  cmd: string;
  args: JsonValue;
  /** 宿主分配的单调序号（§2.1）。 */
  seq: number;
  /** 进程启动参考时间的毫秒时间戳。 */
  createdAt: number;
  expiresAt: number;
  /**
   * 发起主体（0.4-A1 跨主体调用）：`"main"`（主窗）或插件 id。
   * self 档调用时等于 `pluginId`；跨主体调用时配额仍记在发起方名下。
   */
  caller?: string;
  /** 执行主体（0.4-A1 跨主体调用）：插件 id。self 档调用时等于 `pluginId`。 */
  target?: string;
  /** V4 A77 root synchronous request id. */
  rootCallId?: string;
  /** V4 A77 parent request id for delegated calls. */
  parentCallId?: string;
  /** V4 A77 1-based bounded synchronous hop count. */
  hopCount?: number;
  /** V4 A88 process runtime generation captured when the call was accepted. */
  runtimeGeneration?: number;
  /**
   * 结算状态（0.4-A1）：`pending` = 已登记等待执行方回填；`settled` = 结果已在此。
   * 只有两态——宿主不区分「执行中」（那是执行方的私事），TTL 兜底回收。
   */
  state?: 'pending' | 'settled';
  /** 执行方回填的结果载荷（仅 `settled` 且成功时存在）。 */
  result?: JsonValue;
  /** 执行方回填的失败码（仅 `settled` 且失败时存在）。 */
  errorCode?: string;
}

/** 插件 descriptor（`host_registry_list` 的返回项）。 */
export interface PluginDescriptor {
  id: string;
  name: string;
  version: string;
  /** 生命周期状态（Rust `State` 的 SCREAMING_SNAKE_CASE 线名，见 `LIFECYCLE_STATES`）。 */
  state: string;
  /** 是否被安全模式禁用（D25/D28）。 */
  disabledBySafemode: boolean;
  /** 插件类型（js/process/wasm/rust）。 */
  pluginType?: string;
  /** 该插件声明的 public 事件 topic（供订阅方发现）。 */
  publishedTopics?: string[];
}

/** 注册表管理操作（计划 D15 定稿枚举）。 */
export type RegistryAdminOpKind = 'disable' | 'enable' | 'uninstall' | 'purge';

/**
 * A83（轮 43）：uninstall/purge 的审批令牌（Rust `AdminReviewToken` 镜像）。
 *
 * 由 `host_registry_admin` 的预览路径（`preview: true`）铸发，把「用户审阅过的
 * 破坏性操作事实」绑到 commit：一次性（nonce）、有界（TTL）、commit 时重核
 * 插件 id / 操作 / 版本——预览后插件换版本，令牌作废，必须重新预览。
 */
export interface AdminReviewToken {
  pluginId: string;
  /** 被审阅的破坏性操作。 */
  op: RegistryAdminOpKind;
  /** 预览时插件的安装版本。 */
  version: string;
  issuedAt: number;
  expiresAt: number;
  nonce: string;
}

export interface RegistryAdminOp {
  op: RegistryAdminOpKind;
  id: string;
  /**
   * A83（轮 43）：`true` = 只预览——宿主铸发一次性令牌并返回将被破坏的事实，
   * **不改任何状态**。仅 uninstall/purge 有效，与 `reviewToken` 互斥。
   */
  preview?: boolean;
  /** A83（轮 43）：预览返回的一次性令牌，commit 时原样带回。 */
  reviewToken?: AdminReviewToken;
}

/**
 * `host_registry_admin` 的线返回（轮 43 判别形）。
 *
 * Rust 侧 `RegistryAdminResponse` 用 internally-tagged 平铺：`executed` 分支保留
 * 既有 `TransitionOutcome` 的**全部顶层字段**，只多 `kind` 判别字段——老读法
 * （直接取 `.from` / `.to`）不受影响。`review` 分支不执行任何操作，携带预览事实
 * 与一次性令牌；确认后须原样提交令牌。
 */
export type RegistryAdminOutcome =
  | {
      kind: 'executed';
      /** 生命周期事件（SCREAMING_SNAKE_CASE 线名）。 */
      event: string;
      from: string;
      to: string;
      /** 实际迁移深度（含链式）。 */
      depth: number;
      /** 是否未匹配到任何规则。 */
      illegal: boolean;
      /** 按序执行的动作。 */
      actions: string[];
    }
  | {
      kind: 'review';
      op: RegistryAdminOpKind;
      pluginId: string;
      version: string;
      /** 预览时刻的生命周期状态（SCREAMING_SNAKE_CASE）。 */
      state: string;
      reviewToken: AdminReviewToken;
    };
