// ──────────────────────────────────────────────────────────────────────────
// 事件契约类型（§4.4 总线 / §4.9-3 越界检查的 JS 侧形状）。
//
// 跨插件事件发布的**唯一通道**是 `host_events_publish`（计划 D1）。
// 插件不得直调官方 `emit` 做跨插件发布——否则绕过总线授权。
// ──────────────────────────────────────────────────────────────────────────

/** JSON 值（跨 IPC 的载荷只能是 JSON）。 */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

/** 事件主题命名约定：`<插件id>.<名字>`，最长 200 字符。 */
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
  /** 单调递增序号（跨通道独立）。 */
  seq: number;
  /** JSON 载荷。 */
  payload: JsonValue;
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

export interface RegistryAdminOp {
  op: RegistryAdminOpKind;
  id: string;
}
