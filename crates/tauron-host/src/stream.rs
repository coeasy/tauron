//! 流式帧（R5 / P0-1）：帧表 + 宿主铸 `seq` + 句柄生命周期。
//!
//! # 为什么需要它
//!
//! R5 之前「流式」只有**形状**没有实现：`plugin_call` 的 `kind` 会在 wire 层被校验成
//! `unary`/`stream`，`call_end` 的注释也叫「stream 终帧确认」，但**没有任何一条路径
//! 能把帧送到接收方**——`channel` 参数只被当成不透明 JSON 收下，写完就丢。于是
//! 一个 `kind: 'stream'` 的调用在前端表现为「回调永不触发、也不报错」，是最难查的
//! 那类断链（静默、无日志、无异常）。
//!
//! 本模块给帧一条真实出路：句柄表 + 载体（[`StreamSink`]）+ 宿主铸 `seq`。
//!
//! # 不变量（方案 R5）
//!
//! 1. **同锁域**：句柄表与 pending-call 表同属 [`crate::registry::Registry`]，锁序固定
//!    `pending → streams`（见 `Registry` 的字段文档）。
//! 2. **`seq` 单调且跨 `kind` 连续**：每个流从 1 开始，每帧 +1，**终帧也占一个 seq**
//!    ——接收方因此可以断言「收到 seq=4 的 end 就说明前面 3 帧都到过」。
//! 3. **终帧后句柄失效**：`end`/`error` 之后任何 `write`/`close` 都是
//!    `E_CALL_NOT_FOUND`，不会静默成功。
//! 4. **身份校验**：句柄绑定订阅者（插件 id），跨插件写是 `E_AUTH_DENIED`——
//!    句柄 id 用 UUID v4，不靠「猜不到」来兜底，而是显式拒绝。
//!
//! # 载体失败时的语义
//!
//! `seq` 在派发**之前**就已占用：载体报错不会回滚 seq。宁可留一个 seq 空洞（并向上
//! 报错），也不重发——重发会让同 seq 出现两次，接收方的去重假设就废了。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, HostError, HostResult};

/// 帧种类（闭集）。未知取值一律拒绝，不做「默认 data」之类的兜底。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    /// 数据帧（可在流中途出现多次）。
    Data,
    /// 正常终帧。
    End,
    /// 错误终帧（`argsJson` 承载错误信息）。
    Error,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::End => "end",
            Self::Error => "error",
        }
    }

    /// 解析线值（大小写敏感，空串/未知 → `None`）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "data" => Some(Self::Data),
            "end" => Some(Self::End),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    /// `end` / `error` 是终帧：发出后句柄失效。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::End | Self::Error)
    }
}

/// 一帧。线形态 `camelCase`，与 TS `StreamFrame` 同构（由 wire-gate 锁死）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamFrame {
    /// 宿主铸的序号（从 1 开始，跨 `kind` 连续）。
    pub seq: u64,
    pub kind: StreamKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_json: Option<serde_json::Value>,
    /// 二进制载荷出口：不经 base64 夹带 JSON（§4.8 R6）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_raw: Option<Vec<u8>>,
}

impl StreamFrame {
    /// 帧是否终结了流。
    pub fn is_terminal(&self) -> bool {
        self.kind.is_terminal()
    }
}

/// 帧载体（传输无关）。Tauri 侧实现是 `Channel<StreamFrame>`，测试里是记录器。
pub trait StreamSink: Send + Sync {
    fn send(&self, frame: &StreamFrame) -> HostResult<()>;
}

/// 无载体：帧只进表、不派发（非 Tauri 宿主与纯逻辑测试用）。
///
/// 显式提供一个「什么都不做」的实现，是为了让 `None` 与 `NullSink` 语义可区分：
/// **没挂载体** = `stream_open` 直接失败（不会静默丢帧），挂了 `NullSink` = 调用方
/// 明确表示「我不要帧，只要 seq/终帧语义」。
pub struct NullSink;

impl StreamSink for NullSink {
    fn send(&self, _frame: &StreamFrame) -> HostResult<()> {
        Ok(())
    }
}

/// 一次调用的帧载体登记项。
struct CallBinding {
    subscriber: String,
    sink: Arc<dyn StreamSink>,
}

struct StreamHandle {
    call_id: String,
    subscriber: String,
    seq: u64,
    closed: bool,
    sink: Arc<dyn StreamSink>,
}

/// 流句柄表 + 调用载体表（同属一个 `Mutex`，见模块文档不变量 1）。
#[derive(Default)]
pub struct StreamRegistry {
    handles: HashMap<String, StreamHandle>,
    call_bindings: HashMap<String, CallBinding>,
    opened: u64,
}

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一次调用的帧载体（`host_plugin_call` 带 `channel` 时调用）。
    ///
    /// 重复登记**覆盖**旧载体：一次调用只有一个接收方，后到的载体是更准确的
    /// （例如前端重连时换了一个 Channel）。
    pub fn bind(&mut self, call_id: &str, subscriber: &str, sink: Arc<dyn StreamSink>) {
        self.call_bindings.insert(
            call_id.to_string(),
            CallBinding {
                subscriber: subscriber.to_string(),
                sink,
            },
        );
    }

    /// 该调用是否挂了帧载体。
    pub fn is_bound(&self, call_id: &str) -> bool {
        self.call_bindings.contains_key(call_id)
    }

    /// 为一次调用的载体开流。句柄 id 是 UUID v4（不可猜），返回前会校验订阅者。
    pub fn open(&mut self, call_id: &str, subscriber: &str) -> HostResult<String> {
        let binding = self.call_bindings.get(call_id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("调用 `{call_id}` 没有帧载体：请带 channel 发起该调用"),
            )
        })?;
        if binding.subscriber != subscriber {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "调用 `{call_id}` 属于插件 `{}`，`{subscriber}` 不得开流",
                    binding.subscriber
                ),
            ));
        }
        let sink = binding.sink.clone();
        self.opened += 1;
        let id = format!("st-{}", uuid::Uuid::new_v4());
        self.handles.insert(
            id.clone(),
            StreamHandle {
                call_id: call_id.to_string(),
                subscriber: subscriber.to_string(),
                seq: 0,
                closed: false,
                sink,
            },
        );
        Ok(id)
    }

    /// 写一帧 `data`（`seq` 由宿主铸）。
    pub fn write(
        &mut self,
        stream_id: &str,
        subscriber: &str,
        args_json: Option<serde_json::Value>,
        args_raw: Option<Vec<u8>>,
    ) -> HostResult<StreamFrame> {
        self.push(stream_id, subscriber, StreamKind::Data, args_json, args_raw)
    }

    /// 发终帧并**使句柄失效**。`kind` 必须是 `end`/`error`。
    pub fn close(
        &mut self,
        stream_id: &str,
        subscriber: &str,
        kind: StreamKind,
    ) -> HostResult<StreamFrame> {
        if !kind.is_terminal() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("关流只接受终帧种类（end/error），收到 `{}`", kind.as_str()),
            ));
        }
        self.push(stream_id, subscriber, kind, None, None)
    }

    fn push(
        &mut self,
        stream_id: &str,
        subscriber: &str,
        kind: StreamKind,
        args_json: Option<serde_json::Value>,
        args_raw: Option<Vec<u8>>,
    ) -> HostResult<StreamFrame> {
        let handle = self.handles.get_mut(stream_id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("流 `{stream_id}` 不存在或已终结"),
            )
        })?;
        if handle.subscriber != subscriber {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("流 `{stream_id}` 属于插件 `{}`", handle.subscriber),
            ));
        }
        if handle.closed {
            return Err(HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("流 `{stream_id}` 已终结（终帧后句柄失效）"),
            ));
        }
        // seq 先占位再派发：载体失败不回滚（见模块文档）。
        handle.seq += 1;
        let frame = StreamFrame {
            seq: handle.seq,
            kind,
            args_json,
            args_raw,
        };
        if frame.is_terminal() {
            handle.closed = true;
        }
        let sink = handle.sink.clone();
        sink.send(&frame)?;
        Ok(frame)
    }

    /// 句柄是否仍可写（终帧后为 `false`）。
    pub fn is_open(&self, stream_id: &str) -> bool {
        self.handles.get(stream_id).is_some_and(|h| !h.closed)
    }

    /// 活跃句柄数（含已终结但未回收的，仅测试/诊断用）。
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// 调用结束时整组回收：给每个未终结的流补发终帧，返回回收条数。
    ///
    /// **这是「handler 忘了关流」的唯一兜底**——不补终帧的话接收方会永远等下去。
    /// `reason` 会挂在终帧的 `argsJson` 上（前端据此区分「正常结束」「调用被取消」
    /// 「调用超时」「窗口关闭」，而不是笼统看到一个 error）。
    pub fn close_for_call(
        &mut self,
        call_id: &str,
        kind: StreamKind,
        reason: Option<serde_json::Value>,
    ) -> usize {
        let kind = if kind.is_terminal() {
            kind
        } else {
            StreamKind::End
        };
        let ids: Vec<String> = self
            .handles
            .iter()
            .filter(|(_, h)| h.call_id == call_id && !h.closed)
            .map(|(id, _)| id.clone())
            .collect();
        let mut closed = 0;
        for id in ids {
            let subscriber = self
                .handles
                .get(&id)
                .map(|h| h.subscriber.clone())
                .unwrap_or_default();
            if self
                .push(&id, &subscriber, kind, reason.clone(), None)
                .is_ok()
            {
                closed += 1;
            }
        }
        self.handles.retain(|_, h| h.call_id != call_id || !h.closed);
        self.call_bindings.remove(call_id);
        closed
    }

    /// 窗口关闭清理：回收该订阅者的全部句柄与调用载体，返回回收条数。
    pub fn close_for_subscriber(&mut self, subscriber: &str) -> usize {
        let ids: Vec<String> = self
            .handles
            .iter()
            .filter(|(_, h)| h.subscriber == subscriber && !h.closed)
            .map(|(id, _)| id.clone())
            .collect();
        let mut closed = 0;
        for id in ids {
            if self
                .push(
                    &id,
                    subscriber,
                    StreamKind::Error,
                    Some(serde_json::json!({ "reason": "subscriber_closed" })),
                    None,
                )
                .is_ok()
            {
                closed += 1;
            }
        }
        self.handles.retain(|_, h| h.subscriber != subscriber);
        self.call_bindings.retain(|_, b| b.subscriber != subscriber);
        closed
    }

    /// 已开过的流总数（诊断）。
    pub fn opened_total(&self) -> u64 {
        self.opened
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 记录型载体：把派发出去的帧原样留下，供断言。
    #[derive(Default)]
    struct RecordingSink {
        frames: Mutex<Vec<StreamFrame>>,
    }

    impl RecordingSink {
        fn frames(&self) -> Vec<StreamFrame> {
            self.frames.lock().unwrap().clone()
        }
    }

    impl StreamSink for RecordingSink {
        fn send(&self, frame: &StreamFrame) -> HostResult<()> {
            self.frames.lock().unwrap().push(frame.clone());
            Ok(())
        }
    }

    /// 会失败的载体：验证「载体报错不回滚 seq」。只失败第一次，之后正常。
    struct FlakySink {
        calls: std::sync::atomic::AtomicUsize,
        frames: Mutex<Vec<StreamFrame>>,
    }

    impl StreamSink for FlakySink {
        fn send(&self, frame: &StreamFrame) -> HostResult<()> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err(HostError::new(ErrorCode::E_HOST_PANIC, "载体挂了"));
            }
            self.frames.lock().unwrap().push(frame.clone());
            Ok(())
        }
    }

    fn registry_with_sink(sink: Arc<dyn StreamSink>) -> (StreamRegistry, String) {
        let mut reg = StreamRegistry::new();
        reg.bind("c-1", "p1", sink);
        let id = reg.open("c-1", "p1").expect("开流");
        (reg, id)
    }

    #[test]
    fn roundtrip_three_data_frames_then_end() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink.clone());

        for i in 1..=3 {
            let f = reg
                .write(&id, "p1", Some(serde_json::json!({ "i": i })), None)
                .expect("写帧");
            assert_eq!(f.seq, i as u64, "seq 从 1 起连续");
            assert_eq!(f.kind, StreamKind::Data);
        }
        let end = reg.close(&id, "p1", StreamKind::End).expect("关流");
        assert_eq!(end.seq, 4, "终帧也占一个 seq（接收方可据此断言前 3 帧都到过）");
        assert!(end.is_terminal());

        let frames = sink.frames();
        assert_eq!(frames.len(), 4, "3 数据帧 + 1 终帧全部派发");
        assert_eq!(frames.last().unwrap().kind, StreamKind::End);
        assert!(!reg.is_open(&id), "终帧后句柄失效");
    }

    #[test]
    fn seq_is_continuous_across_kinds() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink);
        assert_eq!(reg.write(&id, "p1", None, None).unwrap().seq, 1);
        assert_eq!(
            reg.write(&id, "p1", None, Some(vec![1, 2, 3])).unwrap().seq,
            2
        );
        let err = reg.close(&id, "p1", StreamKind::Error).unwrap();
        assert_eq!(err.seq, 3, "跨 kind 连续：data→data→error = 1,2,3");
        assert_eq!(err.kind, StreamKind::Error);
    }

    #[test]
    fn handle_is_invalid_after_terminal_frame() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink);
        reg.close(&id, "p1", StreamKind::End).unwrap();

        let err = reg.write(&id, "p1", None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND, "写失效句柄必须报错");
        let err = reg.close(&id, "p1", StreamKind::End).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND, "重复关流必须报错");
    }

    #[test]
    fn unknown_handle_and_unbound_call_are_rejected() {
        let mut reg = StreamRegistry::new();
        // 未登记载体的调用 → 开流失败（而不是开一条「帧进虚空」的流）。
        let err = reg.open("c-404", "p1").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
        assert!(err.message.contains("没有帧载体"));

        reg.bind("c-1", "p1", Arc::new(NullSink));
        let err = reg.write("st-nope", "p1", None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn cross_plugin_write_is_denied() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink.clone());
        let err = reg.write(&id, "p2", None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "跨插件写必须拒绝");
        let err = reg.open("c-1", "p2").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "跨插件开流必须拒绝");
        assert!(sink.frames().is_empty(), "被拒的写不得留下帧");
    }

    #[test]
    fn close_rejects_non_terminal_kind() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink);
        let err = reg.close(&id, "p1", StreamKind::Data).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(reg.is_open(&id), "非法关流不得终结句柄");
    }

    #[test]
    fn call_end_sweeps_streams_with_terminal_frame() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink.clone());
        reg.write(&id, "p1", Some(serde_json::json!(1)), None)
            .unwrap();
        // handler 忘了关流：调用结束时必须补终帧，否则接收方永远等下去。
        assert_eq!(reg.close_for_call("c-1", StreamKind::End, None), 1);
        let frames = sink.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].kind, StreamKind::End, "兜底终帧必须真的派发");
        assert_eq!(frames[1].seq, 2);
        assert!(!reg.is_bound("c-1"), "载体登记随调用回收");
        // 幂等：重复回收不再计数。
        assert_eq!(reg.close_for_call("c-1", StreamKind::End, None), 0);
    }

    #[test]
    fn close_for_call_carries_reason_for_error_frames() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink.clone());
        reg.write(&id, "p1", None, None).unwrap();
        assert_eq!(
            reg.close_for_call(
                "c-1",
                StreamKind::Error,
                Some(serde_json::json!({ "reason": "canceled" }))
            ),
            1
        );
        let last = sink.frames().last().cloned().unwrap();
        assert_eq!(last.kind, StreamKind::Error);
        assert_eq!(last.args_json.unwrap()["reason"], "canceled");
    }

    #[test]
    fn subscriber_cleanup_sends_error_frame_and_drops_registry() {
        let sink = Arc::new(RecordingSink::default());
        let (mut reg, id) = registry_with_sink(sink.clone());
        reg.write(&id, "p1", None, None).unwrap();
        assert_eq!(reg.close_for_subscriber("p1"), 1);
        let frames = sink.frames();
        assert_eq!(frames.last().unwrap().kind, StreamKind::Error);
        assert!(
            frames.last().unwrap().args_json.is_some(),
            "清理型终帧必须带原因，便于前端区分「正常结束」与「宿主回收」"
        );
        assert_eq!(reg.len(), 0, "句柄整组回收");
        assert_eq!(reg.close_for_subscriber("p1"), 0);
    }

    #[test]
    fn sink_failure_does_not_rollback_seq() {
        let sink = Arc::new(FlakySink {
            calls: std::sync::atomic::AtomicUsize::new(0),
            frames: Mutex::new(Vec::new()),
        });
        let mut reg = StreamRegistry::new();
        reg.bind("c-1", "p1", sink.clone());
        let id = reg.open("c-1", "p1").unwrap();

        assert!(reg.write(&id, "p1", None, None).is_err(), "首次派发失败");
        // seq 已占位：下一帧拿 2 而**不是**重试 1（重发会让同 seq 出现两次，
        // 接收方的去重假设就废了）。句柄仍可写——载体故障不是流的终态。
        let second = reg.write(&id, "p1", None, None).expect("载体恢复后可继续");
        assert_eq!(second.seq, 2, "失败的那一帧不得被重发");
        assert!(reg.is_open(&id));
        assert_eq!(sink.frames.lock().unwrap().len(), 1, "只成功派发了一帧");
    }

    #[test]
    fn kind_vocabulary_is_closed_and_case_sensitive() {
        assert_eq!(StreamKind::parse("data"), Some(StreamKind::Data));
        assert_eq!(StreamKind::parse("end"), Some(StreamKind::End));
        assert_eq!(StreamKind::parse("error"), Some(StreamKind::Error));
        assert_eq!(StreamKind::parse("Data"), None, "大小写敏感");
        assert_eq!(StreamKind::parse(""), None);
        assert_eq!(
            StreamKind::parse("done"),
            None,
            "拼错不得被当成终帧（否则一次 typo 会静默结束流）"
        );
        for k in [StreamKind::Data, StreamKind::End, StreamKind::Error] {
            assert_eq!(StreamKind::parse(k.as_str()), Some(k));
        }
    }
}
