//! V4 call-graph and event-causation guards (A77/A78).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_MAX_CALL_HOPS: u16 = 16;
pub const DEFAULT_MAX_CAUSATION_DEPTH: u16 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReentrancyPolicy {
    DenySamePlugin,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveCall {
    caller: String,
    callee: String,
    parent: Option<String>,
    depth: u16,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CallGraphError {
    #[error("call parent {0} does not exist")]
    ParentMissing(String),
    #[error("call graph hop limit exceeded: {depth} > {limit}")]
    HopLimitExceeded { depth: u16, limit: u16 },
    #[error("call cycle detected through plugin {0}")]
    CycleDetected(String),
    #[error("re-entrant call into plugin {0} is forbidden")]
    ReentrantCall(String),
    #[error("call id {0} already exists")]
    DuplicateCall(String),
}

#[derive(Debug)]
pub struct CallGraph {
    active: HashMap<String, ActiveCall>,
    max_hops: u16,
    reentrancy: ReentrancyPolicy,
}

impl Default for CallGraph {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CALL_HOPS, ReentrancyPolicy::DenySamePlugin)
    }
}

impl CallGraph {
    pub fn new(max_hops: u16, reentrancy: ReentrancyPolicy) -> Self {
        Self { active: HashMap::new(), max_hops: max_hops.max(1), reentrancy }
    }

    pub fn begin(
        &mut self,
        call_id: impl Into<String>,
        parent: Option<&str>,
        caller: impl Into<String>,
        callee: impl Into<String>,
    ) -> Result<u16, CallGraphError> {
        let call_id = call_id.into();
        if self.active.contains_key(&call_id) {
            return Err(CallGraphError::DuplicateCall(call_id));
        }
        let caller = caller.into();
        let callee = callee.into();

        let depth = if let Some(parent_id) = parent {
            let parent_call = self
                .active
                .get(parent_id)
                .ok_or_else(|| CallGraphError::ParentMissing(parent_id.to_string()))?;
            parent_call.depth.saturating_add(1)
        } else {
            1
        };
        if parent.is_some()
            && self.reentrancy == ReentrancyPolicy::DenySamePlugin
            && caller == callee
        {
            return Err(CallGraphError::ReentrantCall(caller));
        }

        // Classify graph-safety violations before the generic hop budget so callers get the
        // actionable terminal cause when a request is both cyclic and too deep.
        let mut cursor = parent.map(str::to_string);
        while let Some(id) = cursor {
            let ancestor =
                self.active.get(&id).ok_or_else(|| CallGraphError::ParentMissing(id.clone()))?;
            if ancestor.caller == callee || ancestor.callee == callee {
                return Err(CallGraphError::CycleDetected(callee));
            }
            cursor = ancestor.parent.clone();
        }

        if depth > self.max_hops {
            return Err(CallGraphError::HopLimitExceeded { depth, limit: self.max_hops });
        }

        self.active.insert(
            call_id,
            ActiveCall { caller, callee, parent: parent.map(str::to_string), depth },
        );
        Ok(depth)
    }

    pub fn end(&mut self, call_id: &str) -> bool {
        self.active.remove(call_id).is_some()
    }

    pub fn active_len(&self) -> usize {
        self.active.len()
    }

    pub fn clear_principal(&mut self, principal: &str) -> usize {
        let before = self.active.len();
        self.active.retain(|_, call| call.caller != principal && call.callee != principal);
        before - self.active.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventCausation {
    pub root_id: String,
    pub parent_id: Option<String>,
    pub depth: u16,
    pub budget: u16,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CausationError {
    #[error("event causation budget exhausted at depth {depth} / {budget}")]
    BudgetExhausted { depth: u16, budget: u16 },
}

impl EventCausation {
    pub fn root(root_id: impl Into<String>, budget: u16) -> Self {
        Self { root_id: root_id.into(), parent_id: None, depth: 0, budget: budget.max(1) }
    }

    pub fn child(&self, event_id: impl Into<String>) -> Result<Self, CausationError> {
        let next = self.depth.saturating_add(1);
        if next > self.budget {
            return Err(CausationError::BudgetExhausted { depth: next, budget: self.budget });
        }
        Ok(Self {
            root_id: self.root_id.clone(),
            parent_id: Some(event_id.into()),
            depth: next,
            budget: self.budget,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_graph_rejects_cycle_and_hop_overflow() {
        let mut graph = CallGraph::new(2, ReentrancyPolicy::DenySamePlugin);
        assert_eq!(graph.begin("c1", None, "host", "a").unwrap(), 1);
        assert_eq!(graph.begin("c2", Some("c1"), "a", "b").unwrap(), 2);
        assert!(matches!(
            graph.begin("c3", Some("c2"), "b", "c"),
            Err(CallGraphError::HopLimitExceeded { .. })
        ));
        assert!(matches!(
            graph.begin("cycle", Some("c2"), "b", "a"),
            Err(CallGraphError::CycleDetected(_))
        ));
    }

    #[test]
    fn direct_self_reentry_is_rejected_without_blocking_normal_delegation() {
        let mut graph = CallGraph::default();
        graph.begin("root-self", None, "a", "a").unwrap();
        graph.end("root-self");
        graph.begin("c1", None, "host", "a").unwrap();
        graph.begin("c2", Some("c1"), "a", "b").unwrap();
        assert!(matches!(
            graph.begin("self", Some("c2"), "b", "b"),
            Err(CallGraphError::ReentrantCall(plugin)) if plugin == "b"
        ));
    }

    #[test]
    fn cleanup_removes_calls_for_dead_principal() {
        let mut graph = CallGraph::default();
        graph.begin("c1", None, "host", "a").unwrap();
        graph.begin("c2", None, "host", "b").unwrap();
        assert_eq!(graph.clear_principal("a"), 1);
        assert_eq!(graph.active_len(), 1);
    }

    #[test]
    fn causation_depth_is_bounded() {
        let root = EventCausation::root("evt-root", 2);
        let one = root.child("evt-1").unwrap();
        let two = one.child("evt-2").unwrap();
        assert!(matches!(two.child("evt-3"), Err(CausationError::BudgetExhausted { .. })));
    }
}
