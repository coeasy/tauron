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
    Duplicate {
        sender: String,
        receiver: String,
        sequence: u64,
    },
    #[error("sequence gap for {sender}->{receiver}: expected {expected}, got {actual}")]
    Gap {
        sender: String,
        receiver: String,
        expected: u64,
        actual: u64,
    },
    #[error("state revision regressed: previous={previous}, actual={actual}")]
    RevisionRegression { previous: u64, actual: u64 },
}

#[derive(Debug, Default)]
pub struct OrderingTracker {
    next: HashMap<(String, String), u64>,
    latest_revision: Option<u64>,
}

impl OrderingTracker {
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
            if let Some(previous) = self.latest_revision {
                if revision < previous {
                    return Err(OrderingError::RevisionRegression {
                        previous,
                        actual: revision,
                    });
                }
            }
            self.latest_revision = Some(revision);
        }
        self.next.insert(key, expected.saturating_add(1));
        Ok(())
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
        assert!(matches!(
            tracker.observe(&event(3)),
            Err(OrderingError::Gap { .. })
        ));
        assert!(matches!(
            tracker.observe(&event(1)),
            Err(OrderingError::Duplicate { .. })
        ));
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
            Err(OrderingError::RevisionRegression {
                previous: 4,
                actual: 3
            })
        );
    }
}
