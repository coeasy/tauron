//! V4 event ordering contract (A102).
//!
//! Tauron does not promise a global total order. Ordering metadata is explicit so every
//! transport can enforce the same minimum guarantees and detect gaps/duplicates.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderedEventMeta {
    pub event_id: String,
    pub sender: String,
    pub receiver: String,
    pub sequence: u64,
    pub state_revision: Option<u64>,
    pub causation_id: Option<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OrderingError {
    #[error("duplicate sequence {sequence} for {sender}->{receiver}")]
    Duplicate { sender: String, receiver: String, sequence: u64 },
    #[error("sequence gap for {sender}->{receiver}: expected {expected}, got {actual}")]
    Gap { sender: String, receiver: String, expected: u64, actual: u64 },
    #[error("state revision regressed: previous={previous}, actual={actual}")]
    RevisionRegression { previous: u64, actual: u64 },
}

#[derive(Debug, Default)]
pub struct OrderingTracker {
    next: HashMap<(String, String), u64>,
    latest_revision: HashMap<(String, String), u64>,
}

impl OrderingTracker {
    /// Allocate the next host-owned sequence for one sender→receiver stream.
    ///
    /// The sequence is committed immediately. Reliable transports must therefore
    /// **pre-validate** their queue budget before calling this (see
    /// `EventBus::publish_with_causation`): a sequence issued for a frame that
    /// never became visible leaves a permanent gap, and rolling it back is unsound once
    /// another publisher has issued on the same stream.
    pub fn issue(
        &mut self,
        sender: &str,
        receiver: &str,
        event_id: &str,
        state_revision: Option<u64>,
        causation_id: Option<&str>,
    ) -> Result<OrderedEventMeta, OrderingError> {
        let key = (sender.to_string(), receiver.to_string());
        if let Some(revision) = state_revision {
            if let Some(previous) = self.latest_revision.get(&key).copied() {
                if revision < previous {
                    return Err(OrderingError::RevisionRegression { previous, actual: revision });
                }
            }
        }
        let sequence = self.next.get(&key).copied().unwrap_or(1);
        let meta = OrderedEventMeta {
            event_id: event_id.to_string(),
            sender: sender.to_string(),
            receiver: receiver.to_string(),
            sequence,
            state_revision,
            causation_id: causation_id.map(str::to_string),
        };
        self.next.insert(key.clone(), sequence.saturating_add(1));
        if let Some(revision) = state_revision {
            self.latest_revision.insert(key, revision);
        }
        Ok(meta)
    }

    /// 接收端判定一帧的顺序（A102）：`Ok(())` = 正常，否则给出违规种类。
    ///
    /// 这是**验收视图**：遇到 gap 后不重同步，因此该流之后的每一帧都继续报 gap，
    /// 异常一个都不掩掉。运行视图正相反——TS 侧 `EventOrderingWatcher`
    /// （`packages/tauron-host/src/events.ts`，由 `toHostRpc` 取件泵与插件 SDK 泵消费）
    /// 报告一次即把期望值抬到 `seq + 1`，否则一次丢帧会把整条流变成噪音。
    pub fn observe(&mut self, meta: &OrderedEventMeta) -> Result<(), OrderingError> {
        let key = (meta.sender.clone(), meta.receiver.clone());
        let expected = self.next.get(&key).copied().unwrap_or(1);
        if meta.sequence < expected {
            return Err(OrderingError::Duplicate {
                sender: meta.sender.clone(),
                receiver: meta.receiver.clone(),
                sequence: meta.sequence,
            });
        }
        if meta.sequence > expected {
            return Err(OrderingError::Gap {
                sender: meta.sender.clone(),
                receiver: meta.receiver.clone(),
                expected,
                actual: meta.sequence,
            });
        }

        if let Some(revision) = meta.state_revision {
            if let Some(previous) = self.latest_revision.get(&key).copied() {
                if revision < previous {
                    return Err(OrderingError::RevisionRegression { previous, actual: revision });
                }
            }
            self.latest_revision.insert(key.clone(), revision);
        }
        self.next.insert(key, expected.saturating_add(1));
        Ok(())
    }

    /// Bound memory by removing every ordering stream involving a destroyed principal.
    pub fn clear_principal(&mut self, principal: &str) -> usize {
        let before = self.next.len();
        self.next.retain(|(sender, receiver), _| sender != principal && receiver != principal);
        self.latest_revision
            .retain(|(sender, receiver), _| sender != principal && receiver != principal);
        before.saturating_sub(self.next.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64) -> OrderedEventMeta {
        OrderedEventMeta {
            event_id: format!("e-{sequence}"),
            sender: "a".into(),
            receiver: "b".into(),
            sequence,
            state_revision: None,
            causation_id: None,
        }
    }

    #[test]
    fn per_sender_receiver_sequence_detects_gap_and_duplicate() {
        let mut tracker = OrderingTracker::default();
        tracker.observe(&event(1)).unwrap();
        assert!(matches!(tracker.observe(&event(3)), Err(OrderingError::Gap { .. })));
        assert!(matches!(tracker.observe(&event(1)), Err(OrderingError::Duplicate { .. })));
    }

    #[test]
    fn issued_sequence_is_per_sender_receiver_stream() {
        let mut tracker = OrderingTracker::default();
        let a1 = tracker.issue("a", "b", "e1", None, None).unwrap();
        let c1 = tracker.issue("a", "c", "e1", None, None).unwrap();
        let a2 = tracker.issue("a", "b", "e2", None, None).unwrap();
        assert_eq!((a1.sequence, c1.sequence, a2.sequence), (1, 1, 2));
    }

    #[test]
    fn clear_principal_releases_ordering_stream_state() {
        let mut tracker = OrderingTracker::default();
        tracker.issue("a", "b", "e1", None, None).unwrap();
        tracker.issue("c", "d", "e2", None, None).unwrap();
        assert_eq!(tracker.clear_principal("a"), 1);
        assert_eq!(tracker.issue("a", "b", "e3", None, None).unwrap().sequence, 1);
        assert_eq!(tracker.issue("c", "d", "e4", None, None).unwrap().sequence, 2);
    }

    #[test]
    fn state_revision_is_monotonic() {
        let mut tracker = OrderingTracker::default();
        let mut first = event(1);
        first.state_revision = Some(4);
        tracker.observe(&first).unwrap();

        let mut second = event(2);
        second.state_revision = Some(3);
        assert_eq!(
            tracker.observe(&second),
            Err(OrderingError::RevisionRegression { previous: 4, actual: 3 })
        );
    }
}
