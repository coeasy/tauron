//! V4 execution-domain and thread-affinity contract (A73).
//!
//! Providers declare where an operation is allowed to execute. The core does not own an
//! async runtime; adapters dispatch onto the proper executor and enter a scoped domain guard.

use std::cell::RefCell;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionDomain {
    MainThread,
    Io,
    Cpu,
    Blocking,
    Runtime(String),
}

impl fmt::Display for ExecutionDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MainThread => f.write_str("main-thread"),
            Self::Io => f.write_str("io"),
            Self::Cpu => f.write_str("cpu"),
            Self::Blocking => f.write_str("blocking"),
            Self::Runtime(name) => write!(f, "runtime:{name}"),
        }
    }
}

thread_local! {
    static CURRENT_DOMAIN: RefCell<Option<ExecutionDomain>> = const { RefCell::new(None) };
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExecutionError {
    #[error("operation requires execution domain {required}, current domain is {actual}")]
    WrongDomain {
        required: ExecutionDomain,
        actual: String,
    },
}

pub struct ExecutionDomainGuard {
    previous: Option<ExecutionDomain>,
}

impl ExecutionDomainGuard {
    pub fn enter(domain: ExecutionDomain) -> Self {
        let previous = CURRENT_DOMAIN.with(|slot| slot.replace(Some(domain)));
        Self { previous }
    }
}

impl Drop for ExecutionDomainGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT_DOMAIN.with(|slot| {
            slot.replace(previous);
        });
    }
}

pub fn current_domain() -> Option<ExecutionDomain> {
    CURRENT_DOMAIN.with(|slot| slot.borrow().clone())
}

pub fn require_domain(required: &ExecutionDomain) -> Result<(), ExecutionError> {
    let current = current_domain();
    if current.as_ref() == Some(required) {
        return Ok(());
    }
    Err(ExecutionError::WrongDomain {
        required: required.clone(),
        actual: current.map_or_else(|| "unbound".into(), |d| d.to_string()),
    })
}

pub fn in_domain<T>(domain: ExecutionDomain, f: impl FnOnce() -> T) -> T {
    let _guard = ExecutionDomainGuard::enter(domain);
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_enforces_and_restores_thread_affinity() {
        assert_eq!(current_domain(), None);
        in_domain(ExecutionDomain::MainThread, || {
            require_domain(&ExecutionDomain::MainThread).unwrap();
            assert!(matches!(
                require_domain(&ExecutionDomain::Io),
                Err(ExecutionError::WrongDomain { .. })
            ));
            in_domain(ExecutionDomain::Io, || {
                require_domain(&ExecutionDomain::Io).unwrap();
            });
            require_domain(&ExecutionDomain::MainThread).unwrap();
        });
        assert_eq!(current_domain(), None);
    }
}
