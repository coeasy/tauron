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

use crate::error::{ErrorCode, HostError, HostResult};
use crate::manifest::EventDecl;

/// 单插件队列上限（计划 §4.4 关键约束）。
pub const MAX_QUEUE: usize = 1000;
/// 连续溢出达到该次数即熔断该订阅者的该通道。
pub const OVERFLOW_STREAK_LIMIT: usize = 3;

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
#[derive(Debug, Clone)]
struct Queue {
    capacity: usize,
    frames: VecDeque<Frame>,
    dropped_total: u64,
    overflow_streak: usize,
    circuit_open: bool,
}

impl Queue {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frames: VecDeque::new(),
            dropped_total: 0,
            overflow_streak: 0,
            circuit_open: false,
        }
    }

    /// 入队。返回是否入队成功。
    ///
    /// - `Event`：容量满 → 丢最旧，计溢出。
    /// - `Request`：容量满 → **不入队**（可靠语义），由调用方转结构化错误。
    /// - `State`：清掉旧帧，只留最新（快照语义）。
    fn enqueue(&mut self, kind: ChannelKind, frame: Frame) -> EnqueueResult {
        match kind {
            ChannelKind::Event => {
                if self.frames.len() >= self.capacity {
                    if self.circuit_open {
                        self.dropped_total += 1;
                        return EnqueueResult::DroppedCircuitOpen;
                    }
                    self.frames.pop_front();
                    self.dropped_total += 1;
                    self.overflow_streak += 1;
                    if self.overflow_streak >= OVERFLOW_STREAK_LIMIT {
                        self.circuit_open = true;
                    }
                    // 帧仍入队，但记录了一次溢出（丢最旧）。
                    self.frames.push_back(frame);
                    return EnqueueResult::QueuedWithOverflow;
                }
            }
            ChannelKind::Request => {
                if self.frames.len() >= self.capacity {
                    if self.circuit_open {
                        self.dropped_total += 1;
                        return EnqueueResult::DroppedCircuitOpen;
                    }
                    return EnqueueResult::Full;
                }
            }
            ChannelKind::State => {
                // 快照语义：只有最新值有意义。
                self.frames.clear();
            }
        }
        self.frames.push_back(frame);
        EnqueueResult::Queued
    }

    /// 取走全部待投递帧（FIFO）。
    fn drain_all(&mut self) -> Vec<Frame> {
        self.frames.drain(..).collect()
    }

    /// 取走最新一帧（State 快照语义）。
    fn drain_latest(&mut self) -> Option<Frame> {
        self.frames.pop_back()
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
}

/// 一帧。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub topic: String,
    /// 单调递增序号（跨通道独立）。
    pub seq: u64,
    pub payload: Value,
}

type QueueKey = (String, ChannelKind);

/// 事件总线。
///
/// 全部内部状态用锁保护，可被多窗口并发调用。
///
/// **锁顺序（规范顺序，任何路径都不得反序持有）**：
/// `stats` → `topics` → `subs` → `topic_subscribers` → `approvals` → `queues`。
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

    // ── 安装期声明 ────────────────────────────────────────────────

    /// 声明插件发布的 topic（安装期调用一次）。
    ///
    /// 重复声明同名 topic 视为 manifest 错误——同一 topic 的归属必须唯一。
    pub fn declare_topics(&self, publisher: &str, decls: &[EventDecl]) -> HostResult<()> {
        let mut t = self.topics.write();
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

    // ── 用户审批（§4.5 安装期审批流的运行期落地）─────────────────
    //
    // ⚠️ **接入状态**：`approve` 目前**没有任何线上入口**——适配层没有对应命令，
    // `@tauron/host` 也没有对应方法，全仓只有本文件单测调用它。因此运行期
    // `approvals` 表恒空、`is_approved` 分支在生产中不可达，跨插件订阅私有
    // topic 只能靠声明方把 topic 标 `public: true`。
    //
    // 方向是**安全**的（fail-closed：未授权即拒绝，不会误放行），要真正启用
    // 审批流需补：(1) 一条「批准/撤销 (subscriber, topic)」的命令 + TS 方法；
    // (2) 调用方据 `E_AUTH_DENIED` 弹出审批 UI。审批记录已随
    // `dispose_subscriber` 回收（见其单测），无需额外清理。

    /// 用户显式审批：允许 `subscriber` 订阅私有 `topic`。
    ///
    /// 无线上入口，见上方接入状态说明；调用前请确认这是宿主内部策略而非
    /// 用户交互的结果。
    pub fn approve(&self, subscriber: &str, topic: &str) {
        self.approvals.lock().insert((subscriber.to_string(), topic.to_string()), ());
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

        let allowed =
            meta.publisher == subscriber || meta.is_public || self.is_approved(subscriber, topic);
        if !allowed {
            self.stats.lock().rejected_subscribes += 1;
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "插件 `{subscriber}` 无权订阅私有 topic `{topic}`（声明者 `{}`，需标 public: true）",
                    meta.publisher
                ),
            ));
        }

        // 幂等：同 subscriber × window × topic 已存在则复用 token。
        // 上限检查放在幂等**之后**：已达上限时，重复订阅（复用同一 token）
        // 仍必须成功，否则持续重复调用的插件会突然开始失败。
        let token = {
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
        self.stats.lock().publishes += 1;

        // **锁序**：`topics` 读锁必须在取 `stats` 之前释放。写成 `match
        // self.topics.read().get(...)` 时读锁临时量会活到整个 match 结束，
        // 于是出现 `topics → stats` 的反序持有；先 cloned() 到独立语句，
        // 读锁在语句末尾即释放（文档顺序：stats → topics）。
        let meta = self.topics.read().get(topic).cloned();
        let Some(meta) = meta else {
            self.stats.lock().undeclared_publishes += 1;
            return PublishResult { dropped: true, ..Default::default() };
        };
        if meta.publisher != publisher {
            // 发布者越界：不是该 topic 的声明者（R8：以声明元数据判定，不解析 topic 前缀）。
            self.stats.lock().undeclared_publishes += 1;
            return PublishResult { dropped: true, ..Default::default() };
        }

        let tokens = {
            let ts = self.topic_subscribers.lock();
            ts.get(topic).cloned().unwrap_or_default()
        };

        let mut delivered = 0usize;
        let mut overflow = 0usize;

        for token in tokens {
            let subscriber = match self.subs.lock().get(&token).map(|m| m.subscriber.clone()) {
                Some(s) => s,
                // 悬挂订阅：token 在反向索引里但已退订。清理它。
                None => {
                    if let Some(list) = self.topic_subscribers.lock().get_mut(topic) {
                        list.retain(|t| t != &token);
                    }
                    continue;
                }
            };

            let key = (subscriber.clone(), kind);
            let result = {
                let mut qs = self.queues.lock();
                let q = qs.entry(key.clone()).or_insert_with(|| Queue::new(self.capacity));
                let seq = q.frames.len() as u64;
                q.enqueue(kind, Frame { topic: topic.to_string(), seq, payload: payload.clone() })
            };
            match result {
                EnqueueResult::Queued => delivered += 1,
                EnqueueResult::QueuedWithOverflow => {
                    delivered += 1;
                    overflow += 1;
                }
                EnqueueResult::Full => overflow += 1,
                EnqueueResult::DroppedCircuitOpen => overflow += 1,
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
        let res = self.publish(publisher, topic, payload, ChannelKind::Request);
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
        let mut qs = self.queues.lock();
        let key = (target.to_string(), ChannelKind::Request);
        let q = qs.entry(key).or_insert_with(|| Queue::new(self.capacity));
        let seq = q.frames.len() as u64;
        let result = q.enqueue(
            ChannelKind::Request,
            Frame {
                topic: topic.to_string(),
                seq,
                payload,
            },
        );
        match result {
            EnqueueResult::Queued | EnqueueResult::QueuedWithOverflow => Ok(1),
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
            let mut a = self.approvals.lock();
            a.retain(|(sub, _), _| sub != subscriber);
        }
        let mut qs = self.queues.lock();
        qs.retain(|(sub, _), _| sub != subscriber);
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
    fn approved_private_topic_is_subscribable() {
        let b = bus(16);
        declare(&b, "com.a", "plugin:com.a:private", false);
        assert!(b.subscribe("com.b", "w1", "plugin:com.a:private").is_err());
        b.approve("com.b", "plugin:com.a:private");
        let o = b.subscribe("com.b", "w1", "plugin:com.a:private").unwrap();
        assert!(!o.duplicate);
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
        b.approve("com.b", "plugin:com.a:x");

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
            "锁序反转：subscribe 必须先取 subs（规范顺序 stats → topics → subs → \
             topic_subscribers → approvals → queues）"
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
        b.approve("com.c", "plugin:com.a:y");
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
