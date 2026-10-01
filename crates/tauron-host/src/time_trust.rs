//! V4 trusted-time state and provider SPI (A100).

use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimeTrustState {
    Trusted,
    Suspicious,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedTime {
    pub now: SystemTime,
    pub state: TimeTrustState,
}

pub trait TrustedTimeProvider: Send + Sync + std::fmt::Debug {
    fn trusted_time(&self) -> TrustedTime;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TimeTrustError {
    #[error("supply-chain decision blocked because time is {0:?}")]
    Untrusted(TimeTrustState),
    #[error("artifact or metadata has expired")]
    Expired,
}

#[derive(Debug, Clone)]
pub struct SystemTimeProvider {
    state: TimeTrustState,
}

impl SystemTimeProvider {
    pub const fn new(state: TimeTrustState) -> Self {
        Self { state }
    }
}

impl TrustedTimeProvider for SystemTimeProvider {
    fn trusted_time(&self) -> TrustedTime {
        TrustedTime { now: SystemTime::now(), state: self.state }
    }
}

/// Expiry checks fail closed when time is not trusted. V4 explicitly forbids silently skipping
/// expiry because the local clock looks suspicious.
pub fn require_unexpired(
    provider: &dyn TrustedTimeProvider,
    expires_at: SystemTime,
) -> Result<(), TimeTrustError> {
    let trusted = provider.trusted_time();
    if trusted.state != TimeTrustState::Trusted {
        return Err(TimeTrustError::Untrusted(trusted.state));
    }
    if trusted.now > expires_at {
        return Err(TimeTrustError::Expired);
    }
    Ok(())
}

pub fn suspicious_if_skew_exceeds(
    local: SystemTime,
    reference: SystemTime,
    max_skew: Duration,
) -> TimeTrustState {
    let skew = if local >= reference {
        local.duration_since(reference).unwrap_or_default()
    } else {
        reference.duration_since(local).unwrap_or_default()
    };
    if skew > max_skew {
        TimeTrustState::Suspicious
    } else {
        TimeTrustState::Trusted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Fixed(TrustedTime);

    impl TrustedTimeProvider for Fixed {
        fn trusted_time(&self) -> TrustedTime {
            self.0
        }
    }

    #[test]
    fn suspicious_clock_does_not_bypass_expiry() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let provider = Fixed(TrustedTime { now, state: TimeTrustState::Suspicious });
        assert_eq!(
            require_unexpired(&provider, now + Duration::from_secs(60)),
            Err(TimeTrustError::Untrusted(TimeTrustState::Suspicious))
        );
    }

    #[test]
    fn trusted_clock_enforces_expiry() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let provider = Fixed(TrustedTime { now, state: TimeTrustState::Trusted });
        assert_eq!(
            require_unexpired(&provider, now - Duration::from_secs(1)),
            Err(TimeTrustError::Expired)
        );
    }
}
