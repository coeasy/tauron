//! V4 fault boundary and subsystem quarantine (A91).
//!
//! Catching a panic protects the process boundary, but it does not prove subsystem state is
//! still consistent. A faulted boundary therefore rejects new work until an explicit reconcile
//! step marks it ready again.

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultState {
    Ready,
    Faulted,
    Reconciling,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FaultRecord {
    pub boundary: String,
    pub operation: String,
    pub message: String,
    pub generation: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FaultError {
    #[error("fault boundary {boundary} is not ready: {state:?}")]
    NotReady { boundary: String, state: FaultState },
    #[error("fault boundary {boundary} panicked during {operation}: {message}")]
    Panicked { boundary: String, operation: String, message: String },
}

#[derive(Debug)]
pub struct FaultBoundary {
    name: String,
    state: FaultState,
    generation: u64,
    last_fault: Option<FaultRecord>,
}

impl FaultBoundary {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), state: FaultState::Ready, generation: 1, last_fault: None }
    }

    pub fn state(&self) -> FaultState {
        self.state
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn last_fault(&self) -> Option<&FaultRecord> {
        self.last_fault.as_ref()
    }

    pub fn ensure_ready(&self) -> Result<(), FaultError> {
        if self.state == FaultState::Ready {
            Ok(())
        } else {
            Err(FaultError::NotReady { boundary: self.name.clone(), state: self.state })
        }
    }

    pub fn run<T>(&mut self, operation: &str, f: impl FnOnce() -> T) -> Result<T, FaultError> {
        self.ensure_ready()?;
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(value) => Ok(value),
            Err(payload) => Err(self.record_panic(operation, payload)),
        }
    }

    /// 登记一次 panic：边界切 `Faulted`、推进 generation、留下记录，返回对外错误。
    ///
    /// 从 `run` 里拆出来只有一个理由：被守护的闭包可能包含**阻塞 I/O**，而 `run` 要求
    /// 调用方在整段闭包期间持有边界锁（`&mut self`）。锁跨磁盘写下去，故障闸门就成了
    /// 事实上的串行化点——读命令被排在别人的写盘后面，真正该管事务的写租约反而成了装饰。
    /// 需要「判就绪 → 松开锁跑闭包 → 事后登记」的调用方（宿主 settings 事务）走这两段。
    pub fn record_panic(
        &mut self,
        operation: &str,
        payload: Box<dyn std::any::Any + Send>,
    ) -> FaultError {
        let message = panic_message(payload);
        self.state = FaultState::Faulted;
        self.generation = self.generation.saturating_add(1);
        self.last_fault = Some(FaultRecord {
            boundary: self.name.clone(),
            operation: operation.to_string(),
            message: message.clone(),
            generation: self.generation,
        });
        FaultError::Panicked {
            boundary: self.name.clone(),
            operation: operation.to_string(),
            message,
        }
    }

    pub fn begin_reconcile(&mut self) -> Result<(), FaultError> {
        if self.state != FaultState::Faulted && self.state != FaultState::Quarantined {
            return self.ensure_ready();
        }
        self.state = FaultState::Reconciling;
        Ok(())
    }

    pub fn reconcile_succeeded(&mut self) {
        self.state = FaultState::Ready;
        self.generation = self.generation.saturating_add(1);
    }

    pub fn reconcile_failed(&mut self) {
        self.state = FaultState::Quarantined;
        self.generation = self.generation.saturating_add(1);
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_faults_boundary_and_blocks_new_work_until_reconcile() {
        let mut boundary = FaultBoundary::new("settings");
        assert!(matches!(
            boundary.run("commit", || panic!("boom")),
            Err(FaultError::Panicked { .. })
        ));
        assert_eq!(boundary.state(), FaultState::Faulted);
        assert!(matches!(boundary.run("get", || 1_u8), Err(FaultError::NotReady { .. })));
        boundary.begin_reconcile().unwrap();
        boundary.reconcile_succeeded();
        assert_eq!(boundary.run("get", || 7_u8).unwrap(), 7);
    }

    #[test]
    fn failed_reconcile_quarantines_boundary() {
        let mut boundary = FaultBoundary::new("provider:http");
        let _ = boundary.run("request", || panic!("bad state"));
        boundary.begin_reconcile().unwrap();
        boundary.reconcile_failed();
        assert_eq!(boundary.state(), FaultState::Quarantined);
    }
}
