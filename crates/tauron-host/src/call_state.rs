//! Atomic call terminal-state arbitration.
//!
//! Completion, cancellation and timeout may race. Only the first transition out of PENDING wins.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CallTerminalState {
    Completed = 1,
    Cancelled = 2,
    TimedOut = 3,
    Failed = 4,
}

#[derive(Debug)]
pub struct AtomicCallState {
    state: AtomicU8,
}

impl Default for AtomicCallState {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicCallState {
    const PENDING: u8 = 0;

    pub const fn new() -> Self {
        Self { state: AtomicU8::new(Self::PENDING) }
    }

    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == Self::PENDING
    }

    pub fn terminal(&self) -> Option<CallTerminalState> {
        match self.state.load(Ordering::Acquire) {
            1 => Some(CallTerminalState::Completed),
            2 => Some(CallTerminalState::Cancelled),
            3 => Some(CallTerminalState::TimedOut),
            4 => Some(CallTerminalState::Failed),
            _ => None,
        }
    }

    /// Attempt the single legal terminal transition. Losing racers perform cleanup only.
    pub fn try_finish(&self, terminal: CallTerminalState) -> bool {
        self.state
            .compare_exchange(
                Self::PENDING,
                terminal as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn only_first_terminal_state_wins() {
        let state = AtomicCallState::new();
        assert!(state.try_finish(CallTerminalState::Completed));
        assert!(!state.try_finish(CallTerminalState::Cancelled));
        assert_eq!(state.terminal(), Some(CallTerminalState::Completed));
    }

    #[test]
    fn concurrent_terminal_race_has_exactly_one_winner() {
        for _ in 0..256 {
            let state = Arc::new(AtomicCallState::new());
            let candidates = [
                CallTerminalState::Completed,
                CallTerminalState::Cancelled,
                CallTerminalState::TimedOut,
                CallTerminalState::Failed,
            ];
            let mut joins = Vec::new();
            for candidate in candidates {
                let state = Arc::clone(&state);
                joins.push(thread::spawn(move || state.try_finish(candidate)));
            }
            let winners = joins.into_iter().map(|j| j.join().unwrap()).filter(|won| *won).count();
            assert_eq!(winners, 1);
            assert!(state.terminal().is_some());
        }
    }
}
