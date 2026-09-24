// ──────────────────────────────────────────────────────────────────────────
// §4.4 事件总线契约（Rust `tauron_host::eventbus` 的线上协议镜像）。
//
// 三类通道语义分离，队列不共用（计划 §4.4 关键约束）：
//   - `event`   可丢，FIFO，溢出丢最旧 + 计数
//   - `request` 可靠，溢出硬失败（`E_CALL_PENDING_FULL`）
//   - `state`   快照替换，只留最新
//
// 授权模型（R8）：topic 前缀（`plugin:<id>:`）**只作路由键，不作授权依据**。
// 归属一律以宿主声明元数据为准——前端不得靠解析 topic 字符串判断归属。
// ──────────────────────────────────────────────────────────────────────────

import type { JsonValue } from './events.js';

/** 三类通道。线名与 Rust `ChannelKind` 的 kebab-case 序列化一致。 */
export const CHANNEL_KINDS = ['event', 'request', 'state'] as const;
export type ChannelKind = (typeof CHANNEL_KINDS)[number];

/** 单插件队列上限（计划 §4.4 关键约束）。 */
export const MAX_QUEUE = 1000;

/** 连续溢出达到该次数即熔断该订阅者的该通道。 */
export const OVERFLOW_STREAK_LIMIT = 3;

/** 总线上的一帧。 */
export interface BusFrame {
  topic: string;
  /** 单调递增序号（跨通道独立）。 */
  seq: number;
  payload: JsonValue;
}

/** 发布结果。`overflow` = 因队列溢出丢弃的份数。 */
export interface BusPublishResult {
  delivered: number;
  /** `true` = 发布被丢弃（topic 未声明或发布者越界）。只计数不报错。 */
  dropped: boolean;
  overflow: number;
}

/** 单队列统计。 */
export interface QueueStats {
  depth: number;
  capacity: number;
  droppedTotal: number;
  overflowStreak: number;
  circuitOpen: boolean;
}

/** 总线级统计。 */
export interface BusStats {
  undeclaredPublishes: number;
  rejectedSubscribes: number;
  publishes: number;
}

/** 订阅结果。 */
export interface BusSubscribeOutcome {
  token: string;
  /** `true` = 同 subscriber × window × topic 已存在（跨窗重复投递检测）。 */
  duplicate: boolean;
}

/** 订阅元数据（宿主返回给订阅者）。 */
export interface BusSubscriptionMeta {
  token: string;
  subscriber: string;
  topic: string;
  window: string;
}

/** topic 声明元数据。`isPublic` 对应 manifest 的 `events.publish[].public`。 */
export interface BusTopicMeta {
  publisher: string;
  isPublic: boolean;
}
