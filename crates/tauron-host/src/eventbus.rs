//! §4.4 事件总线与发布订阅授权（`core/eventbus`）。
//!
//! 职责：`plugin:<id>:<event>` 路由；**每插件独立有界队列**；publish/subscribe 授权判定。
//!
//! ## 三类通道语义分离（关键约束）
//!
//! 事件（可丢）/ 请求（可靠）/ 状态（快照+diff）**不得共用队列**：
//!
//! | 通道 | 溢出策略 | 理由 |
//! |---|---|---|
//! | [`ChannelKind::Event`] | 丢最旧 + 计数 | 事件可丢，实时性优先 |
//! | [`ChannelKind::Request`] | **硬失败**（`E_CALL_PENDING_FULL`） | 请求必须可靠，丢了就是丢了一个往返 |
//! | [`ChannelKind::State`] | 快照替换（只留最新） | 状态只有最新值有意义，历史帧是噪音 |
//!
//! ## 授权模型（R8）
//!
//! - topic 前缀（`plugin:<id>:`）**只作路由键，不作授权依据**（R8）。
//!   本模块从不解析 topic 字符串来判断归属，一律查声明元数据。
//! - 未声明 `events.publish` 的发布 → **丢弃 + 计数**（不报错给调用方，避免
//!   越界发布成为可用的"探测通道"）。
//! - 订阅他人事件需对方标 `public: true` 或用户显式审批（§4.5）。

use std::collections::{HashMap, VecDeque};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::Value;

use crate::call_graph::{EventCausation, DEFAULT_MAX_CAUSATION_DEPTH};
use crate::error::{ErrorCode, HostError, HostResult};
use crate::manifest::EventDecl;
use crate::ordering::{OrderedEventMeta, OrderingTracker};
use crate::policy::{DecisionError, PolicyAuthority};

/// 单插件队列上限（计划 §4.4 关键约束）。
pub const MAX_QUEUE: usize = 1000;
/// 单帧总开销字节上限（序列化 payload + 元字段固定开销，§33 R3-5 count+bytes）。
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
/// 单 `(订阅者 × 通道)` 队列的总开销字节上限，与 `MAX_QUEUE` 数量上限并存：
/// 数量到顶**或**字节到顶都触发各自的溢出/拒绝策略。
pub const MAX_QUEUE_BYTES: usize = 2 * 1024 * 1024;
/// 帧元字段（topic/sender/receiver/event_id/causation + 定长数值）的固定开销估算。
const FRAME_META_OVERHEAD: usize = 384;

/// 一帧的记账成本：payload 序列化长度 + topic 长度 + 元字段固定开销。
fn frame_cost(topic: &str, payload: &Value) -> usize {
    let payload_bytes = serde_json::to_vec(payload).map(|v| v.len()).unwrap_or(usize::MAX);
    topic.len().saturating_add(FRAME_META_OVERHEAD).saturating_add(payload_bytes)
}
/// 连续溢出达到该次数即熔断该订阅者的该通道。
pub const OVERFLOW_STREAK_LIMIT: usize = 3;

/// topic 名的长度上限（字符数，非字节数）。
///
/// 这条约定此前**只存在于前端已发布面**：`@tauron/host` 的 `TOPIC_MAX_LENGTH = 200`
/// 带着「事件主题命名约定，最长 200 字符」的注释发布给所有 SDK 使用者，而两侧都没有
/// 读取方——声明方（[`EventBus::declare_topics`]）对任意长度的 topic 一律接受，
/// 于是「上限」只是一个写在类型旁边的数字。轮 64 起由这里强制，wire-gate 把两份数字
/// 对钉（改一侧不改另一侧会当场红）。长度按**字符数**计（不是字节数）：非 ASCII 名的
/// UTF-16 计数比字符数大，所以同一串在两种口径下结论可能不同——今天前端没有强制点，
/// 记下口径是为了将来前端也接强制点时不会误当同口径。
pub const MAX_TOPIC_NAME_LENGTH: usize = 200;

/// 订阅表全局上限。
///
/// `(subscriber, window, topic)` 三元组是幂等键，而 `window` 由调用方给定且
/// **不做校验**（`host_events_subscribe` 是 self 档，任何插件都能填任意值）。
/// 没有上限时，一个插件用不断变化的 `window` 就能让 `subs` 与
/// `topic_subscribers` 无限增长（宿主内存耗尽）——其余各表都有上限
/// （topics 有 `evict_overflow`、队列有 `MAX_QUEUE`、pending 有
/// `max_pending_calls`、通知有 `capacity`），此处补齐。
///
/// 取 4096：远高于正常用量（8 插件 × 数十 topic × 1 窗口），又足以在
/// 滥用时快速失败。达限返回 `E_SUBSCRIPTION_FULL`（不确定失败，不可自动重试，
/// 与其余「表满」类错误一致）。
pub const MAX_SUBSCRIPTIONS: usize = 4096;

/// 跨主体审批事实的全局上限。
///
/// 审批表与订阅表同级，但没有「取件/退订」那样的自然回收路径：既有随主体销毁
/// 级联清理，也要防**批了却从不订阅**的孤儿事实无限累积（管理面逐条点击即可
/// 注入）。达上限拒绝**新增**，撤销/重复审批不受影响。
pub const MAX_APPROVALS: usize = 4096;

/// 单插件可同时登记的订阅上限。
pub const MAX_SUBSCRIPTIONS_PER_PLUGIN: usize = 256;

/// 熔断关闭阈值：drain 后深度降到上限的该比例以下即复位。
const CIRCUIT_CLOSE_RATIO: f64 = 0.5;

/// 三类通道。队列按 `(订阅者, 通道)` 分开建，互不共用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelKind {
    /// 可丢事件（FIFO，溢出丢最旧）。
    Event,
    /// 可靠请求（溢出硬失败）。
    Request,
    /// 状态快照（只留最新）。
    State,
}

impl ChannelKind {
    /// 从线上字符串解析（与 `#[serde(rename_all = "kebab-case")]` 一致）。
    ///
    /// 前端 `host_events_drain` 以字符串传参，这里做唯一入口的归一化。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "event" => Some(Self::Event),
            "request" => Some(Self::Request),
            "state" => Some(Self::State),
            _ => None,
        }
    }

    /// 线上字符串（与 serde 序列化一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Request => "request",
            Self::State => "state",
        }
    }
}

/// 一个 topic 的声明元数据（安装期写入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicMeta {
    /// 声明该 topic 的插件 id。
    pub publisher: String,
    /// 是否允许其他插件订阅。
    pub is_public: bool,
}

/// 订阅元数据。
#[derive(Debug, Clone)]
pub struct SubMeta {
    pub token: String,
    pub subscriber: String,
    pub topic: String,
    /// 订阅者所在窗口的标识（用于跨窗重复投递检测）。
    pub window: String,
}

/// 订阅结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeOutcome {
    pub token: String,
    /// `true` = 该订阅已存在（同 subscriber × window × topic），本次为幂等命中。
    /// 用于"跨窗重复投递检测"：同一窗口重复订阅会被识别，不产生第二份队列。
    pub duplicate: bool,
}

/// 发布结果。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishResult {
    /// 实际入队的投递份数。
    pub delivered: usize,
    /// `true` = 发布被丢弃（topic 未声明或发布者越界）。**只计数不报错**。
    pub dropped: bool,
    /// 因队列溢出丢弃的帧数。
    pub overflow: usize,
}

/// 单队列统计。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueStats {
    pub depth: usize,
    pub capacity: usize,
    pub dropped_total: u64,
    pub overflow_streak: usize,
    pub circuit_open: bool,
}

/// 总线级统计。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusStats {
    /// 未声明 topic 的发布次数（含发布者越界）。
    pub undeclared_publishes: u64,
    /// 被拒绝的订阅次数。
    pub rejected_subscribes: u64,
    /// 总发布次数。
    pub publishes: u64,
}

/// 单队列：三类通道共用数据结构，但**策略按 kind 分叉**。
/// 数量与字节双预算（§33 R3-5）：每帧按 [`frame_cost`] 记账，出队即销账。
#[derive(Debug, Clone)]
struct Queue {
    capacity: usize,
    frames: VecDeque<(Frame, usize)>,
    bytes: usize,
    dropped_total: u64,
    overflow_streak: usize,
    circuit_open: bool,
}

impl Queue {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frames: VecDeque::new(),
            bytes: 0,
            dropped_total: 0,
            overflow_streak: 0,
            circuit_open: false,
        }
    }

    /// 入队。返回是否入队成功。
    ///
    /// - 超过单帧预算 [`MAX_FRAME_BYTES`] → **任何通道都不入队**（`TooLarge`，
    ///   零副作用，快照通道的旧帧原样保留）。
    /// - `Event`：数量或字节到顶 → 丢最旧腾位，计溢出。
    /// - `Request`：数量或字节到顶 → **不入队**（可靠语义），由调用方转结构化错误。
    /// - `State`：清掉旧帧，只留最新（快照语义）。
    fn enqueue(&mut self, kind: ChannelKind, frame: Frame, cost: usize) -> EnqueueResult {
        if cost > MAX_FRAME_BYTES {
            return EnqueueResult::TooLarge;
        }
        match kind {
            ChannelKind::Event => {
                if self.frames.len() >= self.capacity
                    || self.bytes.saturating_add(cost) > MAX_QUEUE_BYTES
                {
                    if self.circuit_open {
                        self.dropped_total += 1;
                        return EnqueueResult::DroppedCircuitOpen;
                    }
                    let mut evicted = 0usize;
                    while !self.frames.is_empty()
                        && (self.frames.len() >= self.capacity
                            || self.bytes.saturating_add(cost) > MAX_QUEUE_BYTES)
                    {
                        if let Some((_, old_cost)) = self.frames.pop_front() {
                            self.bytes = self.bytes.saturating_sub(old_cost);
                            evicted += 1;
                        }
                    }
                    self.dropped_total += evicted as u64;
                    self.overflow_streak += 1;
                    if self.overflow_streak >= OVERFLOW_STREAK_LIMIT {
                        self.circuit_open = true;
                    }
                    // 帧仍入队，但记录了一次溢出（丢最旧腾位）。
                    self.bytes = self.bytes.saturating_add(cost);
                    self.frames.push_back((frame, cost));
                    return EnqueueResult::QueuedWithOverflow;
                }
            }
            ChannelKind::Request => {
                if let Some(rejection) = self.reliable_rejection(cost) {
                    if matches!(rejection, EnqueueResult::DroppedCircuitOpen) {
                        self.dropped_total += 1;
                    }
                    return rejection;
                }
            }
            ChannelKind::State => {
                // 快照语义：只有最新值有意义。
                self.bytes = 0;
                self.frames.clear();
            }
        }
        self.bytes = self.bytes.saturating_add(cost);
        self.frames.push_back((frame, cost));
        EnqueueResult::Queued
    }

    /// 可靠通道（`Request`）**入队前**的预算判据：`None` = 可以落帧，否则给出
    /// `enqueue` 将会返回的同一种拒绝结果。
    ///
    /// 这是 Request 分支的唯一判据（`enqueue` 自身也走它），供「先验后投」使用：
    /// 铸造序号、创建队列条目、把帧放进前 N 个订阅者队列都是**不可回滚的副作用**，
    /// 一旦部分投递再报错，调用方按失败重试就会重复投递。判据必须与 `enqueue`
    /// 严格一致，否则会预检放行、落帧被拒，接缝原样复发。
    fn reliable_rejection(&self, cost: usize) -> Option<EnqueueResult> {
        if cost > MAX_FRAME_BYTES {
            return Some(EnqueueResult::TooLarge);
        }
        if self.frames.len() >= self.capacity || self.bytes.saturating_add(cost) > MAX_QUEUE_BYTES {
            if self.circuit_open {
                return Some(EnqueueResult::DroppedCircuitOpen);
            }
            return Some(EnqueueResult::Full);
        }
        None
    }

    /// 取走全部待投递帧（FIFO）。
    fn drain_all(&mut self) -> Vec<Frame> {
        self.bytes = 0;
        self.frames.drain(..).map(|(frame, _)| frame).collect()
    }

    /// 取走最新一帧（State 快照语义）。
    fn drain_latest(&mut self) -> Option<Frame> {
        let popped = self.frames.pop_back();
        if let Some((_, cost)) = popped.as_ref() {
            self.bytes = self.bytes.saturating_sub(*cost);
        }
        popped.map(|(frame, _)| frame)
    }

    /// 摘掉本队列里属于 `topic` 的全部待取帧，并回销它们的字节记账。
    fn remove_topic(&mut self, topic: &str) {
        let mut freed = 0usize;
        let pending = std::mem::take(&mut self.frames);
        for (frame, cost) in pending {
            if frame.topic == topic {
                freed = freed.saturating_add(cost);
            } else {
                self.frames.push_back((frame, cost));
            }
        }
        self.bytes = self.bytes.saturating_sub(freed);
    }

    fn stats(&self) -> QueueStats {
        QueueStats {
            depth: self.frames.len(),
            capacity: self.capacity,
            dropped_total: self.dropped_total,
            overflow_streak: self.overflow_streak,
            circuit_open: self.circuit_open,
        }
    }

    /// drain 后按深度比例复位熔断。
    fn settle(&mut self) {
        if !self.circuit_open {
            return;
        }
        let threshold = (self.capacity as f64 * CIRCUIT_CLOSE_RATIO) as usize;
        if self.frames.len() <= threshold {
            self.circuit_open = false;
            self.overflow_streak = 0;
        }
    }
}

enum EnqueueResult {
    Queued,
    /// 入队成功，但为腾出位置丢掉了最旧的一帧（可丢通道的溢出）。
    QueuedWithOverflow,
    /// 容量满且请求类可靠通道——必须向上报结构化错误。
    Full,
    /// 熔断中，已静默丢弃并计数。
    DroppedCircuitOpen,
    /// 单帧超过字节预算（§33 R3-5）：零副作用拒绝，不挤占任何既有帧。
    TooLarge,
}

/// 一帧。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub topic: String,
    /// V4 A102 per sender→receiver monotonic sequence.
    pub seq: u64,
    pub payload: Value,
    /// V4 A102 sender principal.
    pub sender: String,
    /// V4 A102 receiver principal.
    pub receiver: String,
    /// V4 A102 monotonic state revision for state-channel publications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_revision: Option<u64>,
    /// V4 A78 unique event identifier for this emitted frame.
    pub event_id: String,
    /// V4 A78 stable root causation identifier across an event chain.
    pub causation_id: String,
    /// V4 A78 1-based event hop in the causation chain.
    pub event_hop: u16,
    /// V4 A78 hard depth budget enforced by the Host.
    pub max_causation_depth: u16,
}

fn root_event_causation(event_id: &str) -> EventCausation {
    EventCausation::root(event_id, DEFAULT_MAX_CAUSATION_DEPTH)
        .child(event_id)
        .expect("root event depth is always within the default causation budget")
}

fn next_event_causation(
    parent: Option<&EventCausation>,
    event_id: &str,
) -> HostResult<EventCausation> {
    let Some(parent) = parent else {
        return Ok(root_event_causation(event_id));
    };
    let parent_event_id = parent.parent_id.as_deref().unwrap_or_default();
    if parent.root_id.trim().is_empty() || parent_event_id.trim().is_empty() {
        return Err(HostError::new(
            ErrorCode::E_EVENT_CAUSATION_LIMIT,
            "event causation context is missing causationId/eventId",
        ));
    }
    let budget = parent.budget.clamp(1, DEFAULT_MAX_CAUSATION_DEPTH);
    if parent.depth == 0 || parent.depth > budget {
        return Err(HostError::new(
            ErrorCode::E_EVENT_CAUSATION_LIMIT,
            format!("event causation context has invalid depth {}/{}", parent.depth, budget),
        ));
    }
    let sanitized = EventCausation {
        root_id: parent.root_id.clone(),
        parent_id: Some(parent_event_id.to_string()),
        depth: parent.depth,
        budget,
    };
    sanitized.child(event_id).map_err(|error| {
        HostError::new(
            ErrorCode::E_EVENT_CAUSATION_LIMIT,
            format!("event causation budget rejected publish: {error}"),
        )
    })
}

fn frame_with_causation(
    topic: &str,
    payload: Value,
    causation: &EventCausation,
    ordering: &OrderedEventMeta,
) -> Frame {
    Frame {
        topic: topic.to_string(),
        seq: ordering.sequence,
        payload,
        sender: ordering.sender.clone(),
        receiver: ordering.receiver.clone(),
        state_revision: ordering.state_revision,
        event_id: ordering.event_id.clone(),
        causation_id: causation.root_id.clone(),
        event_hop: causation.depth,
        max_causation_depth: causation.budget,
    }
}

type QueueKey = (String, ChannelKind);

const PRIVATE_SUBSCRIBE_OPERATION: &str = "events.subscribe.private";

fn policy_decision_error(error: DecisionError) -> HostError {
    HostError::new(
        ErrorCode::E_AUTH_DENIED,
        format!("event subscription authorization became stale before commit: {error}"),
    )
}

/// 事件总线。
///
/// 全部内部状态用锁保护，可被多窗口并发调用。
///
/// **锁顺序**：普通索引路径保持 `subs → topic_subscribers`；V4 A81 私有订阅提交
/// 使用授权事务 `approvals (outer) → policy (short) → subs → topic_subscribers`，
/// A81 的撤销级联走同一方向（`topics` 先出临界区），出 `approvals` 之后才作废
/// `queues`——`approvals` 与 `queues` 之间没有嵌套边。
/// 发布路径的 A102 顺序锁固定为 `state_revisions → ordering → queues`；没有反向获取。
/// 没有任何路径在持有 `subs/topic_subscribers` 时再获取 `approvals`，因此不会形成环。
///
/// 可靠发布把「预算预检 → 落帧」收进**一个** `ordering → queues` 临界区，因此它在
/// 持锁期间只**临时**读 `subs`：`subs` 一侧从不获取 `ordering`/`queues`
/// （`subscribe`/`unsubscribe` 只碰 `subs`/`topic_subscribers`，`dispose_subscriber`
/// 先出 `subs` 临界区再取 `queues`），所以这条 `queues → subs` 边不构成环。
///
/// 反序持有会构成死锁环。历史缺陷（已修）：
/// - `subscribe` 曾先取 `topic_subscribers` 再取 `subs`，与 `publish` 的
///   悬挂清理、`unsubscribe` 的 `subs → topic_subscribers` 方向相反；
///   并发下互等即死锁。同一条反序还会**丢订阅**（publish 把刚插入
///   `topic_subscribers`、尚未写入 `subs` 的 token 当悬挂订阅删除）。
/// - `publish` 曾用 `match self.topics.read().get(..)` 使读锁临时量活到
///   整个 match 结束，造成 `topics → stats` 反序；改为先 `cloned()`。
///
/// 新增路径时请按上述顺序取锁，并优先「一个语句内只持有一把锁」。
pub struct EventBus {
    capacity: usize,
    topics: RwLock<HashMap<String, TopicMeta>>,
    subs: Mutex<HashMap<String, SubMeta>>,
    topic_subscribers: Mutex<HashMap<String, Vec<String>>>,
    approvals: Mutex<HashMap<(String, String), ()>>,
    /// V4 A81 grant-version authority for runtime approval/revoke decisions.
    policy: Mutex<PolicyAuthority>,
    /// V4 A102 per sender→receiver ordering authority.
    ordering: Mutex<OrderingTracker>,
    /// V4 A102 state revision source, keyed by (publisher, topic).
    state_revisions: Mutex<HashMap<(String, String), u64>>,
    queues: Mutex<HashMap<QueueKey, Queue>>,
    stats: Mutex<BusStats>,
    next_token: std::sync::atomic::AtomicU64,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            capacity: MAX_QUEUE,
            topics: RwLock::default(),
            subs: Mutex::default(),
            topic_subscribers: Mutex::default(),
            approvals: Mutex::default(),
            policy: Mutex::new(PolicyAuthority::new()),
            ordering: Mutex::new(OrderingTracker::default()),
            state_revisions: Mutex::default(),
            queues: Mutex::default(),
            stats: Mutex::default(),
            next_token: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl EventBus {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity >= 1, "队列上限至少 1");
        Self { capacity, ..Default::default() }
    }

    /// 会话态重置（V4 A91 轮 47）：清空订阅 / 审批 / 队列 / 排序 / 状态修订 /
    /// 统计，**保留** topic 声明与容量。
    ///
    /// 故障边界做确定性修复时用。为什么保留声明：`topics` 是**装配期结构事实**
    /// （`declare_topics` 只在宿主装配、深链注册、安装路径写入），而订阅/审批/队列
    /// 是会话瞬态。发布端对未声明 topic 只**静默丢弃并计数**、不返回错误（防存在性
    /// 探测），所以把声明一起清掉等于让修复动作把可工作的消息面悄悄打哑——比故障
    /// 本身更安静。清会话态是「完整一致」（订阅可重建），清声明不是修复。
    pub fn reset_session_state(&self) {
        *self.subs.lock() = HashMap::new();
        *self.topic_subscribers.lock() = HashMap::new();
        *self.approvals.lock() = HashMap::new();
        *self.policy.lock() = PolicyAuthority::new();
        *self.ordering.lock() = OrderingTracker::default();
        *self.state_revisions.lock() = HashMap::new();
        *self.queues.lock() = HashMap::new();
        *self.stats.lock() = BusStats::default();
    }

    // ── 安装期声明 ────────────────────────────────────────────────

    /// 声明插件发布的 topic（安装期调用一次）。
    ///
    /// 重复声明同名 topic 视为 manifest 错误——同一 topic 的归属必须唯一。
    ///
    /// 名字本身也要过校验（轮 64）：空/纯空白名此前能进声明表，而订阅侧
    /// （`host_events_subscribe`）拒空名——声明得进、订阅不进来，那条 topic 就是
    /// 一张永远无人能取的账；超长名同样在安装期硬拒，因为 `TOPIC_MAX_LENGTH` 是
    /// 已发布给 SDK 使用者的命名约定。
    ///
    /// 三段校验（非空 → 长度 → 归属不冲突）都在**同一次写锁内**、插入之前跑完：
    /// 原来边查边插，第 N 条失败时前 N-1 条已经留在表里，而调用方（安装期）拿到的
    /// 是「整批失败」——部分生效的批次既没被声明方预期，也没有回滚路径。
    pub fn declare_topics(&self, publisher: &str, decls: &[EventDecl]) -> HostResult<()> {
        let mut t = self.topics.write();
        for d in decls {
            if d.topic.trim().is_empty() {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("插件 `{publisher}` 声明了空的 topic 名：订阅侧本就拒收空名，不留一条取不走的事实"),
                ));
            }
            let chars = d.topic.chars().count();
            if chars > MAX_TOPIC_NAME_LENGTH {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!(
                        "topic 名 {} 个字符，超过已发布约定上限 {MAX_TOPIC_NAME_LENGTH}（`@tauron/host` 的 `TOPIC_MAX_LENGTH`）：插件 `{publisher}` 的声明被拒",
                        chars
                    ),
                ));
            }
        }
        for d in decls {
            if let Some(prev) = t.get(&d.topic) {
                if prev.publisher != publisher {
                    return Err(HostError::new(
                        ErrorCode::E_INVALID_MANIFEST,
                        format!(
                            "topic `{}` 已由插件 `{}` 声明，插件 `{}` 不可重复声明（R8：归属以声明元数据为准，不以 topic 前缀推断）",
                            d.topic, prev.publisher, publisher
                        ),
                    ));
                }
            }
        }
        for d in decls {
            t.insert(
                d.topic.clone(),
                TopicMeta { publisher: publisher.to_string(), is_public: d.public },
            );
        }
        Ok(())
    }

    pub fn topic_meta(&self, topic: &str) -> Option<TopicMeta> {
        self.topics.read().get(topic).cloned()
    }

    // ── 用户审批（私有 topic 的运行期授权）───────────────────────

    /// 用户/宿主策略显式审批：允许 `subscriber` 订阅私有 `topic`。
    ///
    /// 这里只维护 EventBus 的最小授权事实；谁有权批准由 adapter/Policy 边界判定。
    ///
    /// 两条防腐约束（否则这是唯一没有回收兄弟的 `(subscriber, topic)` 表）：
    /// - **topic 必须已声明**：给未声明 topic 留审批 = 打错的审批事实永远无人消费，
    ///   且主体永不销毁时不会级联回收；
    /// - **全局上限 [`MAX_APPROVALS`]**：与订阅表同姿态，达限确定性拒绝。
    pub fn approve(&self, subscriber: &str, topic: &str) -> HostResult<()> {
        if self.topics.read().get(topic).is_none() {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("topic `{topic}` 未被任何插件声明，不可审批"),
            ));
        }
        let key = (subscriber.to_string(), topic.to_string());
        let mut approvals = self.approvals.lock();
        if !approvals.contains_key(&key) && approvals.len() >= MAX_APPROVALS {
            return Err(HostError::new(
                ErrorCode::E_SUBSCRIPTION_FULL,
                format!(
                    "审批表已达上限 {MAX_APPROVALS}（当前 {}），先撤销失效审批",
                    approvals.len()
                ),
            ));
        }
        if approvals.insert(key, ()).is_none() {
            self.policy.lock().bump_grant(subscriber);
        }
        Ok(())
    }

    /// 撤销一条审批。返回值只回答「有没有真的删掉一条授权事实」（幂等重试为 `false`），
    /// 撤销**效力**与返回值无关：只要 topic 属「他人声明且非公共」档，每次调用都会——
    ///
    /// 1. 立刻退订该 `(subscriber, topic)` 的全部既有订阅：后续 publish 解析不到
    ///    token，帧再也没有投递目标；
    /// 2. 作废三个通道队列里该 topic 的**待取帧**：撤销前已入队、还没被 drain 的
    ///    内容也拿不到了。
    ///
    /// 公共/自属 topic 的订阅不归授权表管，撤销一条冗余审批不会掐掉它们，与
    /// [`EventBus::subscribe`] 的授权判定同一口径。
    ///
    /// **锁序**：topic 元数据先出临界区，再由 `approvals` 临界区覆盖「删授权 →
    /// bump grant → 退订」，方向与 `subscribe` 的
    /// `topics → approvals → policy → subs → topic_subscribers` 一致，因此并发订阅
    /// 不可能在撤销之后把 token 补回索引。作废队列帧放在出 `approvals` **之后**
    /// 做——`queues` 与 `approvals` 之间全模块不产生嵌套边。
    ///
    /// **诚实边界**：与撤销**并发**的 `publish` 若在撤销前已把 token 解析成订阅者，
    /// 仍可能在这次作废之后落一帧。管理面重放一次 revoke（幂等，效力照走）即可封住
    /// 这条尾巴——`revoke_racing_publish_leaves_no_revoked_content_behind` 钉的就是
    /// 这个可重放的收口。要把并发窗口本身封死，得让发布在 `queues` 临界区内重检审批表，
    /// 那会把 `approvals` 拉进发布热路径；已登记在缺口方案 A81 行。
    pub fn revoke(&self, subscriber: &str, topic: &str) -> bool {
        let meta = self.topics.read().get(topic).cloned();
        let key = (subscriber.to_string(), topic.to_string());
        // 只有「他人声明且非公共」的订阅归授权表管，才随撤销一起消失。公共/自属档
        // 的授权依据不是审批表，撤销一条冗余审批不得顺手掐掉合法订阅。
        if !meta.is_some_and(|meta| meta.publisher != subscriber && !meta.is_public) {
            let had = self.approvals.lock().remove(&key).is_some();
            self.reclaim_grant_when_unapproved(subscriber);
            return had;
        }
        let had_grant = {
            let mut approvals = self.approvals.lock();
            let had_grant = approvals.remove(&key).is_some();
            if had_grant {
                self.policy.lock().bump_grant(subscriber);
            }
            let tokens: Vec<String> = self
                .subs
                .lock()
                .values()
                .filter(|m| m.subscriber == subscriber && m.topic == topic)
                .map(|m| m.token.clone())
                .collect();
            for token in tokens {
                let _ = self.unsubscribe(&token);
            }
            had_grant
        };
        // 即使授权行已不存在（幂等重试）也再作废一次队列：那条「发布方在撤销前就
        // 解析到 token」的竞态窗口，正是靠这最后一次作废封住的。
        self.drop_queued(subscriber, topic);
        self.reclaim_grant_when_unapproved(subscriber);
        had_grant
    }

    /// 撤销后的回收（轮 17）：主体一条审批都不剩时，连 `grants` 行一起删。
    ///
    /// 轮 16 的 `forget` 只挂在 `dispose_subscriber`（关窗 / 卸载）上，于是还留着
    /// 一条不需要 dispose 的增长路径：管理面对**任意** subscriber 串 approve→revoke
    /// 循环（`cmd_events_approve_as` 接受管理员给的字串），每次首条 approve 都留下
    /// 一行版本，revoke 只 bump 不删——表就按「历史见过的主体数」增长，而
    /// `MAX_APPROVALS` 管的是审批行数、管不到它。
    ///
    /// 只在「最后一条审批消失」时回收：提前 forget 会把这个主体在**其他 topic**上
    /// 仍然有效的授权一起打成 `StaleGrant`，那是把回收做成了越权拒绝。
    /// 锁序仍是 approvals→policy，与 `approve`/`dispose_subscriber` 同一方向。
    fn reclaim_grant_when_unapproved(&self, subscriber: &str) {
        if self.approvals.lock().keys().any(|(approved, _)| approved == subscriber) {
            return;
        }
        self.policy.lock().forget(subscriber);
    }

    /// 作废某订阅者某 topic 的全部待取帧（三类通道）。
    ///
    /// 只持 `queues` 一把锁，不嵌套 `subs`/`approvals`。字节预算同步回销，否则被撤销
    /// 的帧会一直占额度，「撤销」就退化成一次变相的队列扩容拒绝。
    fn drop_queued(&self, subscriber: &str, topic: &str) {
        let mut qs = self.queues.lock();
        for (owner, q) in qs.iter_mut() {
            if owner.0 == subscriber {
                q.remove_topic(topic);
            }
        }
    }

    /// 稳定顺序列出全部审批，供宿主管理面审计/展示。
    pub fn approvals(&self) -> Vec<(String, String)> {
        let mut rows: Vec<_> = self.approvals.lock().keys().cloned().collect();
        rows.sort();
        rows
    }

    pub fn is_approved(&self, subscriber: &str, topic: &str) -> bool {
        self.approvals.lock().contains_key(&(subscriber.to_string(), topic.to_string()))
    }

    // ── 订阅 ─────────────────────────────────────────────────────

    /// 订阅 topic。
    ///
    /// 授权判定顺序（任一命中即允许）：
    /// 1. 订阅自己声明的 topic
    /// 2. topic 标 `public: true`
    /// 3. 用户显式审批
    ///
    /// 同 `(subscriber, window, topic)` 重复订阅 → 幂等返回已有 token，
    /// 并置 `duplicate: true`（跨窗重复投递检测）。
    pub fn subscribe(
        &self,
        subscriber: &str,
        window: &str,
        topic: &str,
    ) -> HostResult<SubscribeOutcome> {
        if subscriber.is_empty() || topic.is_empty() {
            return Err(HostError::new(ErrorCode::E_AUTH_DENIED, "subscriber 与 topic 均不可为空"));
        }

        let meta = self.topics.read().get(topic).cloned().ok_or_else(|| {
            HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("topic `{topic}` 未被任何插件声明，不可订阅"),
            )
        })?;

        // Public/self subscriptions are non-revocable at this layer. Private cross-principal
        // subscriptions keep the approval lock until the subscription indices are committed.
        // revoke() takes the same outer lock before bumping GrantVersion, so a stale approval
        // cannot pass revalidation and then race into the table after revocation.
        let private_approval = if meta.publisher == subscriber || meta.is_public {
            None
        } else {
            let approvals = self.approvals.lock();
            if !approvals.contains_key(&(subscriber.to_string(), topic.to_string())) {
                drop(approvals);
                self.stats.lock().rejected_subscribes += 1;
                return Err(HostError::new(
                    ErrorCode::E_AUTH_DENIED,
                    format!(
                        "插件 `{subscriber}` 无权订阅私有 topic `{topic}`（声明者 `{}`，需显式审批）",
                        meta.publisher
                    ),
                ));
            }
            let decision =
                self.policy.lock().decide_scoped(subscriber, PRIVATE_SUBSCRIBE_OPERATION, topic);
            Some((approvals, decision))
        };

        // 幂等：同 subscriber × window × topic 已存在则复用 token。
        // 上限检查放在幂等**之后**：已达上限时，重复订阅（复用同一 token）
        // 仍必须成功，否则持续重复调用的插件会突然开始失败。
        let token = {
            if let Some((_, decision)) = private_approval.as_ref() {
                self.policy
                    .lock()
                    .validate_scoped(decision, subscriber, PRIVATE_SUBSCRIBE_OPERATION, topic)
                    .map_err(policy_decision_error)?;
            }
            let mut s = self.subs.lock();
            if let Some(existing) = s
                .values()
                .find(|m| m.subscriber == subscriber && m.window == window && m.topic == topic)
            {
                return Ok(SubscribeOutcome { token: existing.token.clone(), duplicate: true });
            }
            let plugin_subscriptions =
                s.values().filter(|meta| meta.subscriber == subscriber).count();
            if plugin_subscriptions >= MAX_SUBSCRIPTIONS_PER_PLUGIN {
                return Err(HostError::new(
                    ErrorCode::E_SUBSCRIPTION_FULL,
                    format!(
                        "插件订阅数已达上限 {MAX_SUBSCRIPTIONS_PER_PLUGIN}（当前 {plugin_subscriptions}）"
                    ),
                ));
            }
            if s.len() >= MAX_SUBSCRIPTIONS {
                return Err(HostError::new(
                    ErrorCode::E_SUBSCRIPTION_FULL,
                    format!(
                        "订阅表已达上限 {MAX_SUBSCRIPTIONS}（window 由调用方给定，不得用作无限增长维度）"
                    ),
                ));
            }
            let token = format!(
                "sub-{}",
                self.next_token.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            s.insert(
                token.clone(),
                SubMeta {
                    token: token.clone(),
                    subscriber: subscriber.to_string(),
                    topic: topic.to_string(),
                    window: window.to_string(),
                },
            );

            // **锁序**：先 `subs` 后 `topic_subscribers`（全模块统一顺序）。
            // 两张索引在同一锁序区间内更新，避免并发 publish 把刚登记的订阅
            // 当成悬挂 token 清掉；容量检查与插入也因此具有原子性。
            let mut ts = self.topic_subscribers.lock();
            let entry = ts.entry(topic.to_string()).or_default();
            if !entry.contains(&token) {
                entry.push(token.clone());
            }
            token
        };
        Ok(SubscribeOutcome { token, duplicate: false })
    }

    /// 退订。未知 token 视为已退订（幂等）。
    pub fn unsubscribe(&self, token: &str) -> HostResult<()> {
        let meta = match self.subs.lock().remove(token) {
            Some(m) => m,
            None => return Ok(()),
        };
        if let Some(list) = self.topic_subscribers.lock().get_mut(&meta.topic) {
            list.retain(|t| t != token);
        }
        Ok(())
    }

    // ── 发布 ─────────────────────────────────────────────────────

    /// 发布一帧。
    ///
    /// 越界发布（topic 未声明，或调用方不是声明者）**只丢弃并计数**，
    /// 不返回错误——否则越界发布会变成一条可用的存在性探测通道。
    pub fn publish(
        &self,
        publisher: &str,
        topic: &str,
        payload: Value,
        kind: ChannelKind,
    ) -> PublishResult {
        let event_id = uuid::Uuid::new_v4().to_string();
        let causation = root_event_causation(&event_id);
        self.publish_with_causation(publisher, topic, payload, kind, &causation)
    }

    fn publish_with_causation(
        &self,
        publisher: &str,
        topic: &str,
        payload: Value,
        kind: ChannelKind,
        causation: &EventCausation,
    ) -> PublishResult {
        self.stats.lock().publishes += 1;

        let meta = self.topics.read().get(topic).cloned();
        let Some(meta) = meta else {
            self.stats.lock().undeclared_publishes += 1;
            return PublishResult { dropped: true, ..Default::default() };
        };
        if meta.publisher != publisher {
            self.stats.lock().undeclared_publishes += 1;
            return PublishResult { dropped: true, ..Default::default() };
        }

        let tokens = {
            let ts = self.topic_subscribers.lock();
            ts.get(topic).cloned().unwrap_or_default()
        };

        let state_revision = if kind == ChannelKind::State {
            let mut revisions = self.state_revisions.lock();
            let revision = revisions.entry((publisher.to_string(), topic.to_string())).or_insert(0);
            *revision = revision.saturating_add(1);
            Some(*revision)
        } else {
            None
        };

        let cost = frame_cost(topic, &payload);

        // 投递目标先解析干净（保持 `subs → topic_subscribers` 的既有锁序），
        // 再统一进可靠通道预检——预检必须发生在**任何**序号铸造与队列写入之前。
        let mut subscribers: Vec<String> = Vec::with_capacity(tokens.len());
        for token in &tokens {
            match self.subs.lock().get(token).map(|m| m.subscriber.clone()) {
                Some(subscriber) => subscribers.push(subscriber),
                None => {
                    if let Some(list) = self.topic_subscribers.lock().get_mut(topic) {
                        list.retain(|t| t != token);
                    }
                }
            }
        }

        // A102 锁序 `ordering → queues`：整段发布只取这两把锁并持有到投递结束，
        // 于是「预检 → 落帧」是一个原子窗口，预检通过后不会被并发发布挤成溢出。
        let mut ordering = self.ordering.lock();
        let mut qs = self.queues.lock();

        // **可靠通道零副作用拒绝**：`Request` 帧要么全部投递，要么一份都不投。
        // 早期实现逐订阅者落帧、循环结束后才汇总 overflow，于是「前 N 份已入队、
        // 第 N+1 份越界」会让调用方拿到 `E_CALL_PENDING_FULL` 却已产生**部分投递**
        // ——调用方按「失败」重试即重复投递。单订阅者测试看不出这条接缝。
        if kind == ChannelKind::Request {
            let mut rejected = 0usize;
            for subscriber in &subscribers {
                let will_reject = match qs.get(&(subscriber.clone(), kind)) {
                    Some(q) => q.reliable_rejection(cost).is_some(),
                    // 队列尚未建立 == 队列为空，只有单帧上限能拒它。
                    None => cost > MAX_FRAME_BYTES,
                };
                if will_reject {
                    rejected += 1;
                }
            }
            if rejected > 0 {
                return PublishResult { delivered: 0, overflow: rejected, dropped: false };
            }
        }

        let mut delivered = 0usize;
        let mut overflow = 0usize;
        for subscriber in subscribers {
            let ordered = ordering
                .issue(
                    publisher,
                    &subscriber,
                    causation.parent_id.as_deref().unwrap_or(&causation.root_id),
                    state_revision,
                    Some(&causation.root_id),
                )
                .expect("host-generated ordering metadata is monotonic");
            let q = qs.entry((subscriber, kind)).or_insert_with(|| Queue::new(self.capacity));
            match q.enqueue(
                kind,
                frame_with_causation(topic, payload.clone(), causation, &ordered),
                cost,
            ) {
                EnqueueResult::Queued => delivered += 1,
                EnqueueResult::QueuedWithOverflow => {
                    delivered += 1;
                    overflow += 1;
                }
                EnqueueResult::Full => overflow += 1,
                EnqueueResult::DroppedCircuitOpen => overflow += 1,
                EnqueueResult::TooLarge => overflow += 1,
            }
        }

        PublishResult { delivered, overflow, dropped: false }
    }

    /// 请求类发布（可靠语义）：溢出返回结构化错误而非静默丢弃。
    pub fn publish_request(
        &self,
        publisher: &str,
        topic: &str,
        payload: Value,
    ) -> HostResult<PublishResult> {
        self.publish_request_with_causation(publisher, topic, payload, None)
    }

    /// V4 A78 reliable event publish with an optional parent causation context.
    pub fn publish_request_with_causation(
        &self,
        publisher: &str,
        topic: &str,
        payload: Value,
        parent: Option<&EventCausation>,
    ) -> HostResult<PublishResult> {
        let event_id = uuid::Uuid::new_v4().to_string();
        let causation = next_event_causation(parent, &event_id)?;
        let res = self.publish_with_causation(
            publisher,
            topic,
            payload,
            ChannelKind::Request,
            &causation,
        );
        if res.overflow > 0 {
            return Err(HostError::new(
                ErrorCode::E_CALL_PENDING_FULL,
                format!(
                    "请求类事件 `{topic}` 有 {} 份投递超出队列上限 {}（可靠通道不可丢弃）",
                    res.overflow, self.capacity
                ),
            ));
        }
        Ok(res)
    }

    /// **宿主内部调用投递**：把一帧直接入队到目标插件的 `Request` 通道，**绕过**
    /// topic 声明与发布者鉴权（宿主是受信任的路由器，直接写 `(target, Request)` 队列）。
    ///
    /// 这是 `CallDelivery`（A1）的 Js 型投递落点：复用既有的 `Request` 通道与可靠
    /// 语义（溢出即明确错误），但**不**要求目标插件预先声明 topic——否则插件必须在
    /// 订阅之前就声明好 `__call` 入站 topic，而订阅发生在启动期、声明又依赖激活期，
    /// 时序上凑不到一起。直接写队列则订阅/取件（`host_events_drain(target, "request")`）
    /// 照常工作，topic 仅作为帧上的路由标签用于前端区分。
    ///
    /// 返回入队份数（可靠通道下成功恒为 `1`，满则为 `0` 并转结构化错误）。
    pub fn deliver_inbound(&self, target: &str, topic: &str, payload: Value) -> HostResult<usize> {
        if target.is_empty() {
            return Err(HostError::new(ErrorCode::E_AUTH_DENIED, "调用投递目标不可为空"));
        }
        let event_id = uuid::Uuid::new_v4().to_string();
        let causation = root_event_causation(&event_id);
        let cost = frame_cost(topic, &payload);
        let key = (target.to_string(), ChannelKind::Request);
        let mut ordering = self.ordering.lock();
        let mut qs = self.queues.lock();
        // 预检在**任何**副作用之前：铸造序号、创建队列条目都算副作用，被拒路径
        // 必须一项都不留（§8-3「零悬挂队列」断言同样依赖它——被拒的投递不得留下
        // 空队列条目；早期实现先 `entry().or_insert_with()` 再判溢出，两者都会漏）。
        let rejection = match qs.get(&key) {
            Some(q) => q.reliable_rejection(cost),
            None => {
                if cost > MAX_FRAME_BYTES {
                    Some(EnqueueResult::TooLarge)
                } else {
                    None
                }
            }
        };
        if let Some(reason) = rejection {
            return Err(match reason {
                EnqueueResult::TooLarge => HostError::new(
                    ErrorCode::E_CALL_PENDING_FULL,
                    format!(
                        "调用投递超出单帧字节上限 {MAX_FRAME_BYTES}（`{target}` 的 `{topic}`），零副作用拒绝"
                    ),
                ),
                _ => HostError::new(
                    ErrorCode::E_CALL_PENDING_FULL,
                    format!(
                        "调用投递队列已满，无法投递到 `{target}` 的 `{topic}`（可靠通道不可静默丢弃）"
                    ),
                ),
            });
        }
        let ordered = ordering
            .issue("host", target, &event_id, None, Some(&causation.root_id))
            .expect("host-generated ordering metadata is monotonic");
        let q = qs.entry(key).or_insert_with(|| Queue::new(self.capacity));
        match q.enqueue(
            ChannelKind::Request,
            frame_with_causation(topic, payload, &causation, &ordered),
            cost,
        ) {
            EnqueueResult::Queued | EnqueueResult::QueuedWithOverflow => Ok(1),
            // 预检与落帧处同一临界区，以下分支按不变量不可达；仍返回结构化错误，
            // 不让 release 下的不变量破坏变成 panic 面。
            EnqueueResult::TooLarge => Err(HostError::new(
                ErrorCode::E_CALL_PENDING_FULL,
                format!(
                    "调用投递超出单帧字节上限 {MAX_FRAME_BYTES}（`{target}` 的 `{topic}`），零副作用拒绝"
                ),
            )),
            EnqueueResult::Full | EnqueueResult::DroppedCircuitOpen => Err(HostError::new(
                ErrorCode::E_CALL_PENDING_FULL,
                format!(
                    "调用投递队列已满，无法投递到 `{target}` 的 `{topic}`（可靠通道不可静默丢弃）"
                ),
            )),
        }
    }

    // ── 消费 ─────────────────────────────────────────────────────

    /// 拉取某插件某通道的待投递帧。
    ///
    /// - `Event` / `Request`：FIFO，一次取走全部。
    /// - `State`：只取最新一帧（快照语义）。
    pub fn drain(&self, subscriber: &str, kind: ChannelKind) -> HostResult<Vec<Frame>> {
        let key = (subscriber.to_string(), kind);
        let mut qs = self.queues.lock();
        let Some(q) = qs.get_mut(&key) else {
            return Ok(Vec::new());
        };
        let out = match kind {
            ChannelKind::State => q.drain_latest().into_iter().collect(),
            _ => q.drain_all(),
        };
        q.settle();
        Ok(out)
    }

    /// 队列统计。
    pub fn queue_stats(&self, subscriber: &str, kind: ChannelKind) -> Option<QueueStats> {
        self.queues.lock().get(&(subscriber.to_string(), kind)).map(|q| q.stats())
    }

    /// 总线级统计。
    pub fn stats(&self) -> BusStats {
        self.stats.lock().clone()
    }

    // ── 卸载（§8-3：零悬挂订阅）─────────────────────────────────

    /// 卸载发布者：移除其声明的全部 topic、指向这些 topic 的订阅与审批。
    pub fn dispose_publisher(&self, publisher: &str) -> usize {
        let removed_topics: Vec<String> = {
            let mut t = self.topics.write();
            let mut removed = Vec::new();
            t.retain(|topic, m| {
                if m.publisher == publisher {
                    removed.push(topic.clone());
                    false
                } else {
                    true
                }
            });
            removed
        };
        let mut killed = 0usize;
        for topic in &removed_topics {
            let tokens: Vec<String> = {
                let ts = self.topic_subscribers.lock();
                ts.get(topic).cloned().unwrap_or_default()
            };
            for token in tokens {
                if self.unsubscribe(&token).is_ok() {
                    killed += 1;
                }
            }
            self.topic_subscribers.lock().remove(topic);
        }
        {
            let mut a = self.approvals.lock();
            a.retain(|(_, topic), _| !removed_topics.contains(topic));
        }
        self.state_revisions
            .lock()
            .retain(|(owner, topic), _| owner != publisher && !removed_topics.contains(topic));
        self.ordering.lock().clear_principal(publisher);
        killed
    }

    /// 卸载订阅者：移除其全部订阅与队列（§8-3 零悬挂）。
    pub fn dispose_subscriber(&self, subscriber: &str) -> usize {
        let tokens: Vec<String> = {
            let s = self.subs.lock();
            s.values().filter(|m| m.subscriber == subscriber).map(|m| m.token.clone()).collect()
        };
        for token in &tokens {
            let _ = self.unsubscribe(token);
        }
        {
            // 轮 16 R2 + 轮 17 收口：approve/revoke 会在 PolicyAuthority.grants 里
            // 按主体留版本行，此前它没有任何 dispose 兄弟——清除订阅者时必须一并回收
            // （fail-closed 语义见 `PolicyAuthority::forget`）。
            //
            // 回收必须**落在 approvals 临界区内**：`approve` 是「持 approvals → 取
            // policy」的原子对，若这里出临界区再单独取 policy，并发的 approve 能在
            // 两个窗口之间插进一条审批 + 一行 grants，dispose 就把刚插的行删掉、
            // 留下「有审批无版本行」的错位。锁序 approvals→policy 与全模块一致。
            let mut a = self.approvals.lock();
            a.retain(|(sub, _), _| sub != subscriber);
            self.policy.lock().forget(subscriber);
        }
        let mut qs = self.queues.lock();
        qs.retain(|(sub, _), _| sub != subscriber);
        drop(qs);
        self.ordering.lock().clear_principal(subscriber);
        tokens.len()
    }

    /// 该订阅者是否还有任何悬挂订阅（§8-3 断言用）。
    pub fn has_hanging_subscriptions(&self, subscriber: &str) -> bool {
        self.subs.lock().values().any(|m| m.subscriber == subscriber)
    }

    /// 当前订阅者的订阅数（配额观测口径）。
    pub fn subscription_count(&self, subscriber: &str) -> usize {
        self.subs.lock().values().filter(|meta| meta.subscriber == subscriber).count()
    }

    /// 当前订阅总数（宿主资源诊断口径）。
    pub fn subscription_total(&self) -> usize {
        self.subs.lock().len()
    }

    /// 某订阅者在指定窗口下已订阅的 topic 列表（按 topic 去重、稳定排序）。
    ///
    /// `host_registry_list` 的可见性过滤需要"调用方真实订阅了什么"。
    /// **必须由宿主自行推导**，不得采信前端入参——否则 scoped-read 档
    /// 只要自报他人 public topic 就能越权枚举（§2.1 可见性过滤）。
    pub fn subscribed_topics_of(&self, subscriber: &str, window: &str) -> Vec<String> {
        let subs = self.subs.lock();
        let mut topics: Vec<String> = subs
            .values()
            .filter(|m| m.subscriber == subscriber && m.window == window)
            .map(|m| m.topic.clone())
            .collect();
        topics.sort();
        topics.dedup();
        topics
    }

    /// 该订阅者是否还有任何队列残留（§8-3 断言用）。
    pub fn has_hanging_queues(&self, subscriber: &str) -> bool {
        // 一次加锁查三通道。写成 `a.lock() || b.lock() || c.lock()` 时结果依赖
        // 「懒惰布尔的左操作数临时量何时释放」这一微妙规则，一次加锁彻底消除疑问。
        let key = subscriber.to_string();
        let qs = self.queues.lock();
        [ChannelKind::Event, ChannelKind::Request, ChannelKind::State]
            .iter()
            .any(|k| qs.contains_key(&(key.clone(), *k)))
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bus(cap: usize) -> EventBus {
        EventBus::with_capacity(cap)
    }

    fn declare(b: &EventBus, pub_id: &str, topic: &str, is_public: bool) {
        b.declare_topics(pub_id, &[EventDecl { topic: topic.into(), public: is_public }]).unwrap();
    }

    // ── 会话态重置（V4 A91 轮 47）────────────────────────────────

    #[test]
    fn session_reset_clears_subscriptions_but_keeps_topic_declarations() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", false);
        declare(&b, "com.b", "plugin:com.b:y", true);
        b.subscribe("com.b", "w1", "plugin:com.b:y").unwrap();
        b.approve("com.b", "plugin:com.a:x").unwrap();
        b.publish("com.b", "plugin:com.b:y", Value::from(1), ChannelKind::Event);
        assert_eq!(b.drain("com.b", ChannelKind::Event).unwrap().len(), 1);
        assert!(b.is_approved("com.b", "plugin:com.a:x"));

        b.reset_session_state();

        // 会话瞬态清零：订阅 / 审批 / 队列 / 统计。
        assert!(b.subscribed_topics_of("com.b", "w1").is_empty());
        assert!(!b.is_approved("com.b", "plugin:com.a:x"));
        assert!(b.drain("com.b", ChannelKind::Event).unwrap().is_empty());
        assert_eq!(b.stats().publishes, 0, "统计是会话量，随重置归零");

        // 结构声明保留：清掉的话发布端对未声明 topic 只静默丢弃——修复会把
        // 可工作的消息面悄悄打哑（消费端连错误都看不到）。
        assert!(b.topic_meta("plugin:com.a:x").is_some());
        assert!(b.topic_meta("plugin:com.b:y").is_some());

        // 重建会话即恢复：批准 + 订阅 + 发布照常投递。
        b.approve("com.b", "plugin:com.a:x").unwrap();
        b.subscribe("com.b", "w1", "plugin:com.b:y").unwrap();
        b.publish("com.b", "plugin:com.b:y", Value::from(2), ChannelKind::Event);
        assert_eq!(b.drain("com.b", ChannelKind::Event).unwrap().len(), 1);
    }

    // ── 发布授权 ──────────────────────────────────────────────────

    #[test]
    fn subscribe_table_is_bounded_by_window_abuse() {
        // `window` 由调用方给定且不校验，是幂等键的一部分：必须不能成为
        // 无限增长维度（否则 self 档命令即可耗尽宿主内存）。
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        for i in 0..MAX_SUBSCRIPTIONS / MAX_SUBSCRIPTIONS_PER_PLUGIN {
            let subscriber = format!("com.b{i}");
            for window in 0..MAX_SUBSCRIPTIONS_PER_PLUGIN {
                b.subscribe(&subscriber, &format!("w{window}"), "plugin:com.a:x").unwrap();
            }
        }
        let e = b.subscribe("com.overflow", "w-overflow", "plugin:com.a:x").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_SUBSCRIPTION_FULL);
        assert!(!e.retryable, "表满属确定性失败，不应被自动重试");

        // 达限后**幂等重复订阅仍须成功**（复用 token），否则持续重复调用的
        // 插件会在上限处突然开始失败。
        let again = b.subscribe("com.b0", "w0", "plugin:com.a:x").unwrap();
        assert!(again.duplicate, "上限不应影响幂等复用路径");

        // 容量随回收释放：卸载订阅者后必须能重新订阅（否则上限会随生命周期
        // 累积成永久占满，正常使用也会被拒）。
        b.dispose_subscriber("com.b0");
        b.subscribe("com.b0", "w-after-dispose", "plugin:com.a:x").expect("回收后应重新可订阅");
    }

    #[test]
    fn subscription_quota_is_per_plugin_and_isolated() {
        let b = bus(8);
        declare(&b, "owner", "plugin:owner:public", true);
        for i in 0..MAX_SUBSCRIPTIONS_PER_PLUGIN {
            b.subscribe("plugin.a", &format!("window-{i}"), "plugin:owner:public").unwrap();
        }
        assert_eq!(b.subscription_count("plugin.a"), MAX_SUBSCRIPTIONS_PER_PLUGIN);
        let error = b.subscribe("plugin.a", "overflow", "plugin:owner:public").unwrap_err();
        assert_eq!(error.code, ErrorCode::E_SUBSCRIPTION_FULL);

        b.subscribe("plugin.b", "window", "plugin:owner:public")
            .expect("plugin.a 的配额不能阻断 plugin.b");
        assert_eq!(b.subscription_count("plugin.b"), 1);
    }

    #[test]
    fn ordering_sequence_survives_drain_and_is_per_receiver() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w", "plugin:com.a:x").unwrap();
        b.subscribe("com.c", "w", "plugin:com.a:x").unwrap();

        b.publish("com.a", "plugin:com.a:x", Value::from(1), ChannelKind::Event);
        let first_b = b.drain("com.b", ChannelKind::Event).unwrap().remove(0);
        let first_c = b.drain("com.c", ChannelKind::Event).unwrap().remove(0);
        assert_eq!((first_b.seq, first_c.seq), (1, 1));
        assert_eq!(
            (&first_b.sender, &first_b.receiver),
            (&"com.a".to_string(), &"com.b".to_string())
        );

        b.publish("com.a", "plugin:com.a:x", Value::from(2), ChannelKind::Event);
        let second_b = b.drain("com.b", ChannelKind::Event).unwrap().remove(0);
        assert_eq!(second_b.seq, 2, "drain must not reset the sender→receiver sequence");
        assert_eq!(second_b.causation_id, second_b.event_id);
    }

    #[test]
    fn state_revision_is_monotonic_and_shared_across_receivers() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:state", true);
        b.subscribe("com.b", "w", "plugin:com.a:state").unwrap();
        b.subscribe("com.c", "w", "plugin:com.a:state").unwrap();

        b.publish("com.a", "plugin:com.a:state", Value::from(1), ChannelKind::State);
        let first_b = b.drain("com.b", ChannelKind::State).unwrap().remove(0);
        let first_c = b.drain("com.c", ChannelKind::State).unwrap().remove(0);
        assert_eq!(first_b.state_revision, Some(1));
        assert_eq!(first_c.state_revision, Some(1));

        b.publish("com.a", "plugin:com.a:state", Value::from(2), ChannelKind::State);
        let second = b.drain("com.b", ChannelKind::State).unwrap().remove(0);
        assert_eq!(second.state_revision, Some(2));
        assert_eq!(second.seq, 2);
    }

    #[test]
    fn undeclared_topic_is_dropped_and_counted() {
        let b = bus(16);
        let r = b.publish("com.a", "plugin:com.a:nope", Value::Null, ChannelKind::Event);
        assert!(r.dropped, "未声明 topic 的发布应被丢弃");
        assert_eq!(r.delivered, 0);
        assert_eq!(b.stats().undeclared_publishes, 1);
    }

    #[test]
    fn cross_plugin_publish_is_dropped_not_reported() {
        // R8：以声明元数据判定归属，不以 topic 前缀推断。
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:ready", false);
        let r = b.publish("com.b", "plugin:com.a:ready", Value::Null, ChannelKind::Event);
        assert!(r.dropped);
        assert_eq!(r.delivered, 0);
    }

    #[test]
    fn declared_topic_with_subscriber_delivers() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:ready", true);
        b.subscribe("com.b", "w1", "plugin:com.a:ready").unwrap();
        let r = b.publish("com.a", "plugin:com.a:ready", Value::from(7), ChannelKind::Event);
        assert!(!r.dropped);
        assert_eq!(r.delivered, 1);
        let frames = b.drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, Value::from(7));
        assert_eq!(frames[0].topic, "plugin:com.a:ready");
    }

    #[test]
    fn duplicate_topic_declaration_is_rejected() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", false);
        let e = b
            .declare_topics("com.b", &[EventDecl { topic: "plugin:com.a:x".into(), public: false }])
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("不可重复声明"));
    }

    #[test]
    fn redeclaration_by_same_owner_is_idempotent() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", false);
        declare(&b, "com.a", "plugin:com.a:x", true);
        assert!(b.topic_meta("plugin:com.a:x").unwrap().is_public);
    }

    // ── 订阅授权 ──────────────────────────────────────────────────

    #[test]
    fn own_topic_subscribe_needs_no_public_flag() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", false);
        let o = b.subscribe("com.a", "w1", "plugin:com.a:x").unwrap();
        assert!(!o.duplicate);
    }

    #[test]
    fn public_topic_is_subscribable() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:ready", true);
        assert!(b.subscribe("com.b", "w1", "plugin:com.a:ready").is_ok());
    }

    #[test]
    fn private_topic_subscription_is_rejected() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        let e = b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("无权订阅"));
        assert_eq!(b.stats().rejected_subscribes, 1);
    }

    #[test]
    fn revoke_invalidates_private_subscription_decision_token() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        b.approve("com.b", "plugin:com.a:private").unwrap();
        let token = b.policy.lock().decide_scoped(
            "com.b",
            PRIVATE_SUBSCRIBE_OPERATION,
            "plugin:com.a:private",
        );
        b.policy
            .lock()
            .validate_scoped(&token, "com.b", PRIVATE_SUBSCRIBE_OPERATION, "plugin:com.a:private")
            .unwrap();

        assert!(b.revoke("com.b", "plugin:com.a:private"));
        assert!(matches!(
            b.policy.lock().validate_scoped(
                &token,
                "com.b",
                PRIVATE_SUBSCRIBE_OPERATION,
                "plugin:com.a:private"
            ),
            Err(DecisionError::StaleGrant { .. })
        ));
        assert!(!b.revoke("com.b", "plugin:com.a:private"));
    }

    #[test]
    fn revoke_cascades_to_existing_subscriptions_and_queued_frames() {
        // A81 的效力面：撤销之后「还在收帧的订阅」消失，撤销前已入队的帧也取不到。
        // 缺这一条时 revoke 只是 new-calls-only——管理面上撤销成功，数据面继续漏。
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        declare(&b, "com.a", "plugin:com.a:shared", true);
        b.approve("com.b", "plugin:com.a:private").unwrap();
        b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap();
        b.subscribe("com.b", "w2", "plugin:com.a:private").unwrap();
        b.subscribe("com.b", "w1", "plugin:com.a:shared").unwrap();
        assert_eq!(b.subscription_count("com.b"), 3);

        // 撤销前先把帧落进两个通道（不 drain，否则作废路径无从证明）。
        let events = b.publish("com.a", "plugin:com.a:private", Value::from(1), ChannelKind::Event);
        assert_eq!(events.delivered, 2, "同订阅者两个窗口各落一份");
        b.publish_request("com.a", "plugin:com.a:private", Value::from(2)).unwrap();
        b.publish("com.a", "plugin:com.a:shared", Value::from(3), ChannelKind::Event);

        assert!(b.revoke("com.b", "plugin:com.a:private"));
        assert_eq!(b.subscription_count("com.b"), 1, "私有订阅随撤销退订，公共订阅不受影响");
        let leftover = b.drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(leftover.len(), 1, "队列里只剩未被撤销的 topic");
        assert_eq!(leftover[0].topic, "plugin:com.a:shared");
        assert!(b.drain("com.b", ChannelKind::Request).unwrap().is_empty(), "待取的请求帧同样作废");

        // 作废不是「一次性」：之后重新发布也找不到投递目标。
        let after = b.publish("com.a", "plugin:com.a:private", Value::from(4), ChannelKind::Event);
        assert_eq!(after.delivered, 0);
        assert!(b.drain("com.b", ChannelKind::Event).unwrap().is_empty());
    }

    #[test]
    fn revoke_racing_publish_leaves_no_revoked_content_behind() {
        // A81 的 revoke × in-flight 对抗面：发布线程与撤销线程并发跑。
        // 两条断言各守一件事：
        // ① 锁序不成环——三个发布者都能 join 回来（成环时这条测试直接挂住）；
        // ② 撤销与并发发布留下的那条尾巴是**可封住**的：重放一次 revoke 之后，
        //    队列里不得再有被撤销 topic 的帧。
        let b = std::sync::Arc::new(bus(64));
        declare(&b, "com.a", "plugin:com.a:private", false);
        b.approve("com.b", "plugin:com.a:private").unwrap();
        b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap();

        let publishers: Vec<_> = (0..3u32)
            .map(|i| {
                let b = b.clone();
                std::thread::spawn(move || {
                    for n in 0..400 {
                        b.publish(
                            "com.a",
                            "plugin:com.a:private",
                            Value::from(i * 1000 + n),
                            ChannelKind::Event,
                        );
                    }
                })
            })
            .collect();
        // 在发布仍在飞行时撤销：此时可能有 publish 已经解析出 token。
        assert!(b.revoke("com.b", "plugin:com.a:private"));
        for handle in publishers {
            handle.join().expect("并发发布不得 panic");
        }
        assert_eq!(b.subscription_count("com.b"), 0, "并发下撤销也要退干净");
        assert!(!b.revoke("com.b", "plugin:com.a:private"), "幂等重放只回答授权事实");
        let leftover = b.drain("com.b", ChannelKind::Event).unwrap();
        assert!(
            leftover.iter().all(|frame| frame.topic != "plugin:com.a:private"),
            "重放撤销后被撤销 topic 的帧必须已作废（实际残留 {:?}）",
            leftover.iter().map(|f| f.topic.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn revoke_of_a_redundant_public_approval_leaves_the_subscription_intact() {
        // 审批表可以给公共 topic 留一条冗余事实；撤销它**不该**顺手掐掉订阅——
        // 公共/自属订阅的授权依据不是审批表（与 subscribe 同一口径）。
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:shared", true);
        b.approve("com.b", "plugin:com.a:shared").unwrap();
        b.subscribe("com.b", "w1", "plugin:com.a:shared").unwrap();
        b.publish("com.a", "plugin:com.a:shared", Value::from(1), ChannelKind::Event);

        assert!(b.revoke("com.b", "plugin:com.a:shared"));
        assert_eq!(b.subscription_count("com.b"), 1);
        assert_eq!(b.drain("com.b", ChannelKind::Event).unwrap().len(), 1);
        assert!(b.subscribe("com.b", "w2", "plugin:com.a:shared").is_ok());
    }

    #[test]
    fn approve_rejects_undeclared_topic_and_leaves_no_orphan_fact() {
        let b = bus(16);
        let e = b.approve("com.b", "plugin:com.a:ghost").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("未被任何插件声明"));
        assert!(b.approvals().is_empty(), "被拒的审批不得留下授权事实");
    }

    #[test]
    fn duplicate_approve_does_not_churn_grant_version() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        b.approve("com.b", "plugin:com.a:private").unwrap();
        let version = b.policy.lock().grant_version("com.b");
        b.approve("com.b", "plugin:com.a:private").unwrap();
        assert_eq!(b.policy.lock().grant_version("com.b"), version);
    }

    /// 轮 16 R2：`grants` 表此前只有插入点（approve/revoke），任何 dispose 都不清它——
    /// 按主体无界滞留。清除必须 fail-closed：旧 token 不能因为「行没了」而复活。
    #[test]
    fn dispose_subscriber_clears_policy_grant_row_fail_closed() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        b.approve("com.b", "plugin:com.a:private").unwrap();
        let token = b.policy.lock().decide_scoped(
            "com.b",
            PRIVATE_SUBSCRIBE_OPERATION,
            "plugin:com.a:private",
        );
        assert!(b.policy.lock().grant_version("com.b") > 0);

        b.dispose_subscriber("com.b");
        assert_eq!(b.policy.lock().grant_version("com.b"), 0, "grants 行随订阅者回收");
        assert!(
            matches!(
                b.policy.lock().validate_scoped(
                    &token,
                    "com.b",
                    PRIVATE_SUBSCRIBE_OPERATION,
                    "plugin:com.a:private",
                ),
                Err(DecisionError::StaleGrant { .. })
            ),
            "被清除主体的在途 token 必须判 stale，而不是当无版本放行"
        );
    }

    /// 轮 17：dispose 之外的第二条回收路径——approve→revoke 循环不得在
    /// `PolicyAuthority.grants` 里留行。`cmd_events_approve_as` 接受管理员给的
    /// 任意 subscriber 串，旧实现每见一个新串就永久留一行（`MAX_APPROVALS` 只数
    /// 审批行、管不到版本表）。同时钉住反向过度回收：还有审批在手时不得提前删。
    #[test]
    fn revoke_of_last_approval_reclaims_grant_row() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        declare(&b, "com.c", "plugin:com.c:private", false);

        b.approve("com.b", "plugin:com.a:private").unwrap();
        assert!(
            b.policy.lock().grant_version("com.b") > 0,
            "首条 approve 建行（版本 ≥1 ⇔ 行存在：版本取自全局发号器）"
        );

        b.approve("com.b", "plugin:com.c:private").unwrap();
        let version = b.policy.lock().grant_version("com.b");
        assert!(version > 0);
        assert!(b.revoke("com.b", "plugin:com.a:private"));
        let after_first_revoke = b.policy.lock().grant_version("com.b");
        assert!(
            after_first_revoke > version,
            "撤销本身要抬版本（旧 token 作废），但**不得删行**：还剩一条审批，\
             删行等于把别的 topic 上仍然有效的授权打成 StaleGrant。版本仍 >0 即行仍在\
             （本表的最小版本是 1，版本 0 与「行不存在」同义）"
        );

        assert!(b.revoke("com.b", "plugin:com.c:private"));
        assert_eq!(
            b.policy.lock().grant_version("com.b"),
            0,
            "最后一条审批消失即回收版本行：表里不留历史主体"
        );
    }

    #[test]
    fn approved_private_topic_is_subscribable() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        assert!(b.subscribe("com.b", "w1", "plugin:com.a:private").is_err());
        b.approve("com.b", "plugin:com.a:private").unwrap();
        let o = b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap();
        assert!(!o.duplicate);
    }

    #[test]
    fn private_topic_approval_can_be_listed_and_revoked() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);

        b.approve("com.b", "plugin:com.a:private").unwrap();
        assert_eq!(b.approvals(), vec![("com.b".to_string(), "plugin:com.a:private".to_string())]);
        assert!(b.subscribe("com.b", "w1", "plugin:com.a:private").is_ok());

        assert!(b.revoke("com.b", "plugin:com.a:private"));
        assert!(!b.revoke("com.b", "plugin:com.a:private"), "重复撤销应幂等");
        assert!(b.approvals().is_empty());
        assert!(!b.is_approved("com.b", "plugin:com.a:private"));

        // 撤销的两条效力：既有订阅当场消失（A81 级联），新订阅回到 fail-closed。
        assert_eq!(b.subscription_count("com.b"), 0, "既有订阅随撤销退订");
        assert!(b.subscribe("com.b", "w2", "plugin:com.a:private").is_err());
    }

    #[test]
    fn private_topic_full_chain_reject_approve_subscribe_revoke_reject() {
        // §8.1 五步全链路：拒绝 → 审批 → 订阅成功 → 撤销 → 再拒绝，单测贯通。
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);

        // 1) 未审批：fail-closed 拒绝，且不产生任何授权事实
        let e = b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(b.approvals().is_empty());

        // 2) 审批：授权事实可见
        b.approve("com.b", "plugin:com.a:private").unwrap();
        assert!(b.is_approved("com.b", "plugin:com.a:private"));
        assert_eq!(b.approvals(), vec![("com.b".to_string(), "plugin:com.a:private".to_string())]);

        // 3) 订阅放行
        let o = b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap();
        assert!(!o.duplicate);

        // 4) 撤销：授权事实即刻消失，第 3 步的既有订阅同时退订，重复撤销幂等
        assert!(b.revoke("com.b", "plugin:com.a:private"));
        assert!(!b.revoke("com.b", "plugin:com.a:private"));
        assert!(!b.is_approved("com.b", "plugin:com.a:private"));
        assert!(b.approvals().is_empty());
        assert_eq!(b.subscription_count("com.b"), 0);

        // 5) 撤销后新订阅回到 fail-closed
        let e = b.subscribe("com.b", "w3", "plugin:com.a:private").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("无权订阅"));
    }

    #[test]
    fn subscribe_to_unknown_topic_is_rejected() {
        let b = bus(16);
        let e = b.subscribe("com.b", "w1", "plugin:x:nowhere").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("未被任何插件声明"));
    }

    #[test]
    fn subscribe_with_empty_args_is_rejected() {
        let b = bus(16);
        assert!(b.subscribe("", "w1", "t").is_err());
        assert!(b.subscribe("com.a", "w1", "").is_err());
    }

    // ── 跨窗重复投递检测 ──────────────────────────────────────────

    #[test]
    fn duplicate_subscription_in_same_window_is_idempotent() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", true);
        let o1 = b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        let o2 = b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        assert!(!o1.duplicate);
        assert!(o2.duplicate, "同窗口重复订阅应被识别");
        assert_eq!(o1.token, o2.token);
        // 只有一份队列：不产生第二份。
        declare_publish_and_drain(&b);
    }

    fn declare_publish_and_drain(b: &EventBus) {
        b.publish("com.a", "plugin:com.a:x", Value::from(1), ChannelKind::Event);
        let frames = b.drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 1, "重复订阅不得导致重复投递");
    }

    #[test]
    fn different_windows_get_separate_queues() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", true);
        let o1 = b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        let o2 = b.subscribe("com.b", "w2", "plugin:com.a:x").unwrap();
        assert!(!o1.duplicate && !o2.duplicate);
        assert_ne!(o1.token, o2.token);
    }

    // ── topic 名预算（轮 64：已发布的 `TOPIC_MAX_LENGTH` 第一次有强制点）──

    #[test]
    fn blank_topic_name_is_rejected_at_declare_time() {
        let b = EventBus::default();
        for bad in ["", "   "] {
            let err = b
                .declare_topics("com.a", &[EventDecl { topic: bad.into(), public: true }])
                .expect_err("空白 topic 名必须安装期硬拒");
            assert!(err.message.contains("空的 topic 名"), "{:?}", err.message);
        }
    }

    #[test]
    fn topic_name_boundary_takes_exactly_the_published_budget() {
        let b = EventBus::default();
        let exact = "t".repeat(MAX_TOPIC_NAME_LENGTH);
        b.declare_topics("com.a", &[EventDecl { topic: exact.clone(), public: false }])
            .expect("正好等于上限的名字应被接受");
        assert_eq!(b.topic_meta(&exact).unwrap().publisher, "com.a");

        let over = "x".repeat(MAX_TOPIC_NAME_LENGTH + 1);
        let err = b
            .declare_topics("com.b", &[EventDecl { topic: over, public: false }])
            .expect_err("超出已发布约定的名字必须拒, 不能静默收下");
        assert!(err.message.contains("超过已发布约定上限"), "{:?}", err.message);
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn a_rejected_batch_leaves_no_partially_declared_topic() {
        let b = EventBus::default();
        let err = b
            .declare_topics(
                "com.a",
                &[
                    EventDecl { topic: "com.a.first".into(), public: true },
                    EventDecl { topic: "  ".into(), public: true },
                ],
            )
            .expect_err("批次里第 2 条非法应整批失败");
        assert!(err.message.contains("空的 topic 名"), "{:?}", err.message);
        assert!(
            b.topic_meta("com.a.first").is_none(),
            "被拒批次的第一条不得留下声明事实（调用方拿到的是整批失败）"
        );
    }

    // ── 队列上限边界（999 / 1000 / 1001）─────────────────────────

    #[test]
    fn event_queue_boundary_is_exact() {
        let b = bus(3);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();

        // 999/1000/1001 的等价小规模版本：2/3/4。
        for i in 0..2 {
            let r = b.publish("com.a", "plugin:com.a:x", Value::from(i), ChannelKind::Event);
            assert_eq!(r.delivered, 1, "第 {i} 帧应入队");
            assert_eq!(r.overflow, 0);
        }
        let r = b.publish("com.a", "plugin:com.a:x", Value::from(2), ChannelKind::Event);
        assert_eq!(r.delivered, 1, "第 3 帧（=容量）应入队");
        let r = b.publish("com.a", "plugin:com.a:x", Value::from(3), ChannelKind::Event);
        assert_eq!(r.delivered, 1, "第 4 帧应入队并丢最旧");
        assert_eq!(r.overflow, 1, "第 4 帧应产生 1 次溢出");

        let st = b.queue_stats("com.b", ChannelKind::Event).unwrap();
        assert_eq!(st.depth, 3, "深度必须恒等于容量");
        assert_eq!(st.capacity, 3);
        assert_eq!(st.dropped_total, 1);
        assert_eq!(st.overflow_streak, 1);

        let frames = b.drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].payload, Value::from(1), "FIFO：最旧的一帧已被丢弃");
        assert_eq!(frames[2].payload, Value::from(3));
    }

    #[test]
    fn event_queue_holds_exactly_max_items_at_full_boundary() {
        let cap = MAX_QUEUE;
        let b = bus(cap);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for _ in 0..cap {
            b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        }
        let st = b.queue_stats("com.b", ChannelKind::Event).unwrap();
        assert_eq!(st.depth, cap, "第 {cap} 帧应正好填满");
        assert_eq!(st.dropped_total, 0);
        // 第 cap+1 帧触发溢出。
        let r = b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        assert_eq!(r.overflow, 1);
        assert_eq!(b.queue_stats("com.b", ChannelKind::Event).unwrap().depth, cap);
    }

    // ── 字节预算（§33 R3-5：count + bytes 双配额）──────────────────

    fn big_payload(tag: usize) -> Value {
        Value::from(format!("p{tag}|{}", "x".repeat(200_000)))
    }

    fn oversized_payload() -> Value {
        Value::from("y".repeat(MAX_FRAME_BYTES))
    }

    #[test]
    fn byte_budget_evicts_oldest_before_count_cap_is_reached() {
        // 每帧 ≈200KB，数量上限 16 远未触及；2MiB 字节上限先在 10 帧处生效。
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for i in 0..11 {
            let r = b.publish("com.a", "plugin:com.a:x", big_payload(i), ChannelKind::Event);
            assert_eq!(r.delivered, 1);
            assert_eq!(r.overflow, if i < 10 { 0 } else { 1 }, "第 11 帧应因字节预算丢最旧");
        }
        let frames = b.drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 10, "字节上限（2MiB ÷ ≈200KB）先于数量上限（16）封顶");
        let first = frames[0].payload.as_str().unwrap();
        let last = frames[9].payload.as_str().unwrap();
        assert!(first.starts_with("p1|"), "最旧一帧（p0）应已被驱逐：{first}");
        assert!(last.starts_with("p10|"));
    }

    #[test]
    fn oversized_event_frame_is_rejected_with_zero_side_effects() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.publish("com.a", "plugin:com.a:x", Value::from(42), ChannelKind::Event);
        let r = b.publish("com.a", "plugin:com.a:x", oversized_payload(), ChannelKind::Event);
        assert_eq!(r.delivered, 0, "超预算帧不得入队");
        assert_eq!(r.overflow, 1);
        let frames = b.drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, Value::from(42), "既有帧不得被超预算发布波及");
    }

    #[test]
    fn oversized_state_publish_keeps_previous_snapshot() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:state", true);
        b.subscribe("com.b", "w1", "plugin:com.a:state").unwrap();
        b.publish("com.a", "plugin:com.a:state", Value::from("snap-1"), ChannelKind::State);
        let r = b.publish("com.a", "plugin:com.a:state", oversized_payload(), ChannelKind::State);
        assert_eq!(r.delivered, 0);
        let frames = b.drain("com.b", ChannelKind::State).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, Value::from("snap-1"), "快照不得被超大帧清空");
    }

    #[test]
    fn request_channel_fails_structured_when_byte_budget_full() {
        // 数量上限 64 未触及，字节先到顶 → 可靠通道返回结构化错误而非静默丢弃。
        let b = bus(64);
        declare(&b, "com.a", "plugin:com.a:req", true);
        b.subscribe("com.b", "w1", "plugin:com.a:req").unwrap();
        for i in 0..10 {
            b.publish_request("com.a", "plugin:com.a:req", big_payload(i)).unwrap();
        }
        let e = b.publish_request("com.a", "plugin:com.a:req", big_payload(10)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_PENDING_FULL);
        let frames = b.drain("com.b", ChannelKind::Request).unwrap();
        assert_eq!(frames.len(), 10, "被拒的可靠投递不得改变队列");
    }

    // ── 熔断 ─────────────────────────────────────────────────────

    #[test]
    fn consecutive_overflow_opens_circuit() {
        let b = bus(2);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for _ in 0..2 {
            b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        }
        // OVERFLOW_STREAK_LIMIT 次连续溢出。
        for i in 0..OVERFLOW_STREAK_LIMIT {
            b.publish("com.a", "plugin:com.a:x", Value::from(i), ChannelKind::Event);
        }
        let st = b.queue_stats("com.b", ChannelKind::Event).unwrap();
        assert!(st.circuit_open, "连续溢出应触发熔断");
        assert_eq!(st.overflow_streak, OVERFLOW_STREAK_LIMIT);

        // 熔断中：帧直接丢弃并计数，不入队。
        let before = st.depth;
        let r = b.publish("com.a", "plugin:com.a:x", Value::from(99), ChannelKind::Event);
        assert_eq!(r.overflow, 1);
        let st2 = b.queue_stats("com.b", ChannelKind::Event).unwrap();
        assert_eq!(st2.depth, before, "熔断中不得入队");
        assert!(st2.dropped_total > st.dropped_total);
    }

    #[test]
    fn circuit_closes_after_drain_below_half() {
        let b = bus(2);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for _ in 0..2 {
            b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        }
        for _ in 0..OVERFLOW_STREAK_LIMIT {
            b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        }
        assert!(b.queue_stats("com.b", ChannelKind::Event).unwrap().circuit_open);
        // 消费掉积压 → 深度降到 0 ≤ 容量/2 → 熔断复位。
        b.drain("com.b", ChannelKind::Event).unwrap();
        let st = b.queue_stats("com.b", ChannelKind::Event).unwrap();
        assert!(!st.circuit_open, "drain 后应复位熔断");
        assert_eq!(st.overflow_streak, 0);
        // 复位后可正常入队，不再丢弃。
        let r = b.publish("com.a", "plugin:com.a:x", Value::from(1), ChannelKind::Event);
        assert_eq!(r.overflow, 0);
        assert_eq!(r.delivered, 1);
    }

    // ── 慢消费者 ─────────────────────────────────────────────────

    #[test]
    fn slow_consumer_frames_are_fresh_after_drain() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        // 消费者完全不消费，发送超过容量（溢出 2 次，未达熔断阈值）。
        let mut total_overflow = 0usize;
        for i in 0..10 {
            total_overflow +=
                b.publish("com.a", "plugin:com.a:x", Value::from(i), ChannelKind::Event).overflow;
        }
        let frames = b.drain("com.b", ChannelKind::Event).unwrap();
        // 只剩最新的 8 帧（最旧的 2 帧被持续丢弃）。
        assert_eq!(frames.len(), 8);
        assert_eq!(frames[0].payload, Value::from(2usize));
        assert_eq!(frames[7].payload, Value::from(9usize));
        assert_eq!(total_overflow, 2);
        let st = b.queue_stats("com.b", ChannelKind::Event).unwrap();
        assert_eq!(st.depth, 0);
        assert_eq!(st.dropped_total, 2);
    }

    // ── 三类通道语义分离 ─────────────────────────────────────────

    #[test]
    fn event_causation_is_propagated_and_depth_is_bounded() {
        let b = EventBus::default();
        b.declare_topics("com.a", &[EventDecl { topic: "plugin:com.a:x".into(), public: true }])
            .unwrap();
        b.subscribe("com.b", "w", "plugin:com.a:x").unwrap();

        b.publish_request_with_causation("com.a", "plugin:com.a:x", Value::from(1), None).unwrap();
        let first = b.drain("com.b", ChannelKind::Request).unwrap().remove(0);
        assert_eq!(first.event_hop, 1);
        assert_eq!(first.causation_id, first.event_id);

        let parent = EventCausation {
            root_id: first.causation_id.clone(),
            parent_id: Some(first.event_id.clone()),
            depth: first.event_hop,
            budget: 2,
        };
        b.publish_request_with_causation("com.a", "plugin:com.a:x", Value::from(2), Some(&parent))
            .unwrap();
        let second = b.drain("com.b", ChannelKind::Request).unwrap().remove(0);
        assert_eq!(second.causation_id, first.causation_id);
        assert_eq!(second.event_hop, 2);

        let exhausted = EventCausation {
            root_id: second.causation_id.clone(),
            parent_id: Some(second.event_id.clone()),
            depth: second.event_hop,
            budget: 2,
        };
        let err = b
            .publish_request_with_causation(
                "com.a",
                "plugin:com.a:x",
                Value::from(3),
                Some(&exhausted),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_EVENT_CAUSATION_LIMIT);
        assert!(b.drain("com.b", ChannelKind::Request).unwrap().is_empty());
    }

    #[test]
    fn three_channels_have_separate_queues() {
        let b = bus(3);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.publish("com.a", "plugin:com.a:x", Value::from("event"), ChannelKind::Event);
        b.publish_request("com.a", "plugin:com.a:x", Value::from("req")).unwrap();
        b.publish("com.a", "plugin:com.a:x", Value::from("state"), ChannelKind::State);

        for kind in [ChannelKind::Event, ChannelKind::Request, ChannelKind::State] {
            let st = b.queue_stats("com.b", kind).unwrap();
            assert_eq!(st.depth, 1, "每种通道各有独立队列：{kind:?}");
        }
        assert_eq!(b.drain("com.b", ChannelKind::Event).unwrap()[0].payload, Value::from("event"));
        assert_eq!(b.drain("com.b", ChannelKind::Request).unwrap()[0].payload, Value::from("req"));
    }

    #[test]
    fn request_channel_overflow_is_a_hard_failure() {
        let b = bus(2);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for _ in 0..2 {
            b.publish_request("com.a", "plugin:com.a:x", Value::Null).unwrap();
        }
        // 第 3 份：可靠通道不可丢弃 → 结构化错误。
        let e = b.publish_request("com.a", "plugin:com.a:x", Value::Null).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_PENDING_FULL);
        assert!(e.message.contains("不可丢弃"));
        // 前两帧完好保留。
        assert_eq!(b.drain("com.b", ChannelKind::Request).unwrap().len(), 2);
    }

    #[test]
    fn request_overflow_is_all_or_nothing_across_subscribers() {
        // R3-5「零副作用拒绝」在多订阅者下的真身：早期实现逐订阅者落帧、循环结束
        // 才汇总 overflow，于是排在前面、队列还空的订阅者**已经拿到帧**，调用方却
        // 收到 `E_CALL_PENDING_FULL`。调用方按「失败」重试即重复投递——单订阅者
        // 测试（上面那条）看不出这条接缝。
        let b = bus(2);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.subscribe("com.c", "w1", "plugin:com.a:x").unwrap();
        for _ in 0..2 {
            b.publish_request("com.a", "plugin:com.a:x", Value::Null).unwrap();
        }
        // 只清空 com.b：com.c 满、com.b 空，越界发生在第二个目标上。
        assert_eq!(b.drain("com.b", ChannelKind::Request).unwrap().len(), 2);

        let e = b.publish_request("com.a", "plugin:com.a:x", Value::from(3)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_PENDING_FULL);
        assert_eq!(
            b.queue_stats("com.b", ChannelKind::Request).unwrap().depth,
            0,
            "被拒的发布不得给任何订阅者留下帧（部分投递 = 重试即重复）"
        );

        // 序号同样是副作用：预检在铸造序号之前完成，所以被拒的那次不占号。
        assert_eq!(b.drain("com.c", ChannelKind::Request).unwrap().len(), 2);
        b.publish_request("com.a", "plugin:com.a:x", Value::from(4)).unwrap();
        let retried = b.drain("com.b", ChannelKind::Request).unwrap();
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].seq, 3, "前两次成功占 1/2，被拒那次不占号 → 第三次成功仍是 3");
    }

    #[test]
    fn request_channel_does_not_allow_lossy_publish() {
        // 直接 publish 请求类也能看到 overflow 计数（但推荐用 publish_request）。
        let b = bus(1);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.publish_request("com.a", "plugin:com.a:x", Value::Null).unwrap();
        let e = b.publish_request("com.a", "plugin:com.a:x", Value::Null).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_PENDING_FULL);
    }

    #[test]
    fn state_channel_keeps_only_latest_snapshot() {
        let b = bus(4);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for i in 0..4 {
            b.publish("com.a", "plugin:com.a:x", Value::from(i), ChannelKind::State);
        }
        let st = b.queue_stats("com.b", ChannelKind::State).unwrap();
        assert_eq!(st.depth, 1, "状态通道只保留最新一帧");
        let frames = b.drain("com.b", ChannelKind::State).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, Value::from(3));
    }

    #[test]
    fn state_channel_never_overflows() {
        let b = bus(1);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        for _ in 0..100 {
            let r = b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::State);
            assert_eq!(r.overflow, 0, "快照替换语义不产生溢出");
            assert_eq!(r.delivered, 1);
        }
        assert_eq!(b.queue_stats("com.b", ChannelKind::State).unwrap().depth, 1);
    }

    #[test]
    fn drain_unknown_subscriber_is_empty_not_error() {
        let b = bus(8);
        assert!(b.drain("nobody", ChannelKind::Event).unwrap().is_empty());
        assert!(b.queue_stats("nobody", ChannelKind::Event).is_none());
    }

    #[test]
    fn drain_state_returns_empty_when_no_snapshot() {
        let b = bus(8);
        assert!(b.drain("com.b", ChannelKind::State).unwrap().is_empty());
    }

    // ── 卸载与零悬挂（§8-3）──────────────────────────────────────

    #[test]
    fn dispose_subscriber_leaves_no_hanging_state() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.subscribe("com.b", "w2", "plugin:com.a:x").unwrap();
        b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::State);
        b.approve("com.b", "plugin:com.a:x").unwrap();

        let killed = b.dispose_subscriber("com.b");
        assert_eq!(killed, 2);
        assert!(!b.has_hanging_subscriptions("com.b"), "§8-3：卸载后零悬挂订阅");
        assert!(!b.has_hanging_queues("com.b"), "§8-3：卸载后零悬挂队列");
        // 审批记录也随之清理。
        assert!(!b.is_approved("com.b", "plugin:com.a:x"));
        // 发布者仍在，topic 声明不受影响。
        assert!(b.topic_meta("plugin:com.a:x").is_some());
    }

    // ── 锁序（并发死锁回归）─────────────────────────────────────
    //
    // 动态检测「A 持 X 等 Y」需要精确竞态时序：主线程占住 subs 时，subscribe
    // 会在**重复检查**那次 subs 上加锁就阻塞，根本推进不到反转点，因此
    // 动态断言对两种顺序都为真（无判别力）。这里改为对源码断言取锁顺序，
    // 确定性地锁定不变量。

    /// `subscribe` 必须先取 `subs` 再取 `topic_subscribers`。
    ///
    /// 反序会与 `publish` 的悬挂清理分支、`unsubscribe` 构成死锁环；
    /// 同一条反序还会丢订阅（publish 把刚写入反向索引、尚未进 subs 的
    /// token 当悬挂订阅删除）。
    #[test]
    fn subscribe_lock_order_is_subs_then_topic_subscribers() {
        let src = include_str!("eventbus.rs");
        let body = src
            .split("pub fn subscribe(")
            .nth(1)
            .and_then(|s| s.split("pub fn unsubscribe(").next())
            .expect("subscribe 函数体");

        let subs_lock = body.find("self.subs.lock()").expect("subscribe 里应有 subs 锁");
        let subs_insert = body.find("s.insert(").expect("subscribe 里应有 subs 插入");
        let reverse_index =
            body.find("self.topic_subscribers.lock()").expect("subscribe 里应有反向索引写入");

        assert!(
            subs_lock < subs_insert && subs_insert < reverse_index,
            "锁序反转：订阅索引提交必须保持 subs → topic_subscribers；私有授权事务的 approvals 是外层锁"
        );
    }

    /// `publish` 必须在取 `stats` 之前释放 `topics` 读锁。
    ///
    /// `match self.topics.read().get(..)` 的读锁临时量活到整个 match 结束，
    /// 会让 `stats.lock()` 在持锁状态下执行，形成 `topics → stats` 反序。
    #[test]
    fn publish_releases_topics_before_touching_stats() {
        let src = include_str!("eventbus.rs");
        let body = src
            .split("pub fn publish(")
            .nth(1)
            .and_then(|s| s.split("pub fn publish_request(").next())
            .expect("publish 函数体");

        // 必须用独立语句 cloned() 出 meta：读锁在语句末尾释放，之后才可能取 stats。
        assert!(
            body.contains("let meta = self.topics.read().get(topic).cloned();"),
            "publish 必须用独立语句 cloned() 出 meta，使 topics 读锁立即释放"
        );
        // 反例形态（读锁临时量活到 match 结束，导致持 topics 锁取 stats）不得回归。
        assert!(
            !body.contains("match self.topics.read()"),
            "不得用 match 持有 topics 读锁跨越 stats 加锁（读锁临时量活到 match 结束）"
        );
    }

    /// 悬挂 token（在 `topic_subscribers` 但不在 `subs`）必须被清理，
    /// 且清理不得误伤随后建立的合法订阅。
    #[test]
    fn dangling_token_is_cleaned_without_losing_later_subscription() {
        let b = bus(4);
        // public：允许 com.b（非声明者）订阅。
        declare(&b, "com.a", "plugin:com.a:ready", true);

        // 手工制造悬挂态：只写反向索引。
        b.topic_subscribers
            .lock()
            .entry("plugin:com.a:ready".to_string())
            .or_default()
            .push("sub-ghost".to_string());

        let r = b.publish("com.a", "plugin:com.a:ready", Value::Null, ChannelKind::Event);
        assert_eq!(r.delivered, 0, "悬挂 token 不产生投递");

        let leftover_empty = b
            .topic_subscribers
            .lock()
            .get("plugin:com.a:ready")
            .map(|v| v.is_empty())
            .unwrap_or(true);
        assert!(leftover_empty, "悬挂 token 应被 publish 清理");

        // 清理后新订阅仍能正常收到事件（无残留干扰）。
        assert!(!b.subscribe("com.b", "w-1", "plugin:com.a:ready").unwrap().duplicate);
        let r2 = b.publish("com.a", "plugin:com.a:ready", Value::Null, ChannelKind::Event);
        assert_eq!(r2.delivered, 1, "重新订阅后必须能收到事件");
    }

    #[test]
    fn dispose_publisher_kills_dependent_subscriptions() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        declare(&b, "com.a", "plugin:com.a:y", false);
        b.approve("com.c", "plugin:com.a:y").unwrap();
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.subscribe("com.c", "w1", "plugin:com.a:y").unwrap();

        let killed = b.dispose_publisher("com.a");
        assert_eq!(killed, 2, "指向被卸载 topic 的订阅应全部解绑");
        assert!(b.topic_meta("plugin:com.a:x").is_none());
        assert!(b.topic_meta("plugin:com.a:y").is_none());
        assert!(!b.has_hanging_subscriptions("com.b"));
        assert!(!b.has_hanging_subscriptions("com.c"));
        assert!(!b.is_approved("com.c", "plugin:com.a:y"));
    }

    #[test]
    fn dispose_publisher_does_not_touch_other_publishers() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        declare(&b, "com.d", "plugin:com.d:x", true);
        b.dispose_publisher("com.a");
        assert!(b.topic_meta("plugin:com.d:x").is_some());
    }

    #[test]
    fn unsubscribe_is_idempotent() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        let o = b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        assert!(b.unsubscribe(&o.token).is_ok());
        assert!(b.unsubscribe(&o.token).is_ok(), "重复退订应幂等");
        assert!(b.unsubscribe("never-existed").is_ok());
    }

    #[test]
    fn publish_after_unsubscribe_delivers_nothing() {
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        let o = b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.unsubscribe(&o.token).unwrap();
        let r = b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        assert_eq!(r.delivered, 0, "退订后不得再投递");
    }

    #[test]
    fn dangling_subscription_in_reverse_index_is_cleaned_on_publish() {
        // 模拟反向索引悬挂：token 在 topic_subscribers 里但 subs 里没有。
        // 正常路径不会产生这种状态，这里直接构造以验证清理逻辑。
        let b = bus(8);
        declare(&b, "com.a", "plugin:com.a:x", true);
        b.topic_subscribers.lock().insert("plugin:com.a:x".into(), vec!["ghost-token".into()]);
        let r = b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        assert_eq!(r.delivered, 0);
        // 悬挂 token 已被清出反向索引。
        let list = b.topic_subscribers.lock().get("plugin:com.a:x").cloned().unwrap_or_default();
        assert!(!list.contains(&"ghost-token".to_string()));
    }

    // ── 统计 ─────────────────────────────────────────────────────

    #[test]
    fn stats_track_all_counters() {
        let b = bus(2);
        declare(&b, "com.a", "plugin:com.a:x", true);
        declare(&b, "com.a", "plugin:com.a:private", false);
        b.subscribe("com.b", "w1", "plugin:com.a:x").unwrap();
        b.publish("com.a", "plugin:com.a:x", Value::Null, ChannelKind::Event);
        b.publish("com.a", "plugin:com.a:nope", Value::Null, ChannelKind::Event);
        // 订阅"已声明但私有"的 topic 才会计入 rejected_subscribes
        //（订阅完全未声明的 topic 走的是另一条分支）。
        assert!(b.subscribe("com.c", "w1", "plugin:com.a:private").is_err());
        let s = b.stats();
        assert_eq!(s.publishes, 2);
        assert_eq!(s.undeclared_publishes, 1);
        assert_eq!(s.rejected_subscribes, 1);
    }

    #[test]
    fn capacity_must_be_at_least_one() {
        // 保护性断言：0 容量会让"可靠通道"永不可用。
        // 用 panic 捕获验证，不依赖 unwrap 链。
        let caught = std::panic::catch_unwind(|| {
            let _ = EventBus::with_capacity(0);
        });
        assert!(caught.is_err(), "容量 0 应被拒绝");
    }

    #[test]
    fn publish_result_serializes_for_ipc() {
        let v = serde_json::to_value(PublishResult { delivered: 3, overflow: 1, dropped: false })
            .unwrap();
        assert_eq!(v["delivered"], 3);
        assert_eq!(v["overflow"], 1);
        assert_eq!(v["dropped"], false);
    }
}
