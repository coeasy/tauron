//! V4 Remote Host security/session contract.
//!
//! This module is transport-neutral and owns the bounded security/session authority. A concrete
//! network adapter may provide RemoteTransportEvidence only after it has actually established
//! authenticated TLS. The official TLS network reference adapter remains a separate implementation.
//!
//! The authority owns short-lived one-time credentials, audience/origin binding, bounded sessions,
//! resume, replay protection, rate/inflight quotas, expiry and canonical Universal Wire handling.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{decode_wire_json, encode_wire_json, WireError, WireFrame, DEFAULT_MAX_WIRE_BYTES};

const MAX_NONCE_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteHostConfig {
    pub audience: String,
    pub allowed_origins: Vec<String>,
    pub credential_ttl_ms: u64,
    pub idle_timeout_ms: u64,
    pub absolute_session_age_ms: u64,
    pub resume_window_ms: u64,
    pub rate_window_ms: u64,
    pub max_requests_per_window: u32,
    pub max_inflight_per_session: u32,
    pub nonce_window: usize,
    pub max_credentials: usize,
    pub max_sessions: usize,
    pub max_sessions_per_subject: usize,
}

impl Default for RemoteHostConfig {
    fn default() -> Self {
        Self {
            audience: "tauron-remote".into(),
            allowed_origins: vec!["https://localhost".into()],
            credential_ttl_ms: 60_000,
            idle_timeout_ms: 30_000,
            absolute_session_age_ms: 600_000,
            resume_window_ms: 15_000,
            rate_window_ms: 1_000,
            max_requests_per_window: 100,
            max_inflight_per_session: 32,
            nonce_window: 256,
            max_credentials: 1_024,
            max_sessions: 1_024,
            max_sessions_per_subject: 8,
        }
    }
}

impl RemoteHostConfig {
    fn validate(&self) -> Result<(), RemoteHostError> {
        if self.audience.trim().is_empty()
            || self.allowed_origins.is_empty()
            || self.allowed_origins.iter().any(|origin| origin.trim().is_empty())
        {
            return Err(RemoteHostError::InvalidConfig(
                "audience and allowed_origins must be non-empty".into(),
            ));
        }
        if self.credential_ttl_ms == 0
            || self.idle_timeout_ms == 0
            || self.absolute_session_age_ms == 0
            || self.resume_window_ms == 0
            || self.rate_window_ms == 0
            || self.max_requests_per_window == 0
            || self.max_inflight_per_session == 0
            || self.nonce_window == 0
            || self.max_credentials == 0
            || self.max_sessions == 0
            || self.max_sessions_per_subject == 0
        {
            return Err(RemoteHostError::InvalidConfig(
                "remote bounds/timeouts must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTransportEvidence {
    pub transport_id: String,
    pub encrypted: bool,
    pub server_authenticated: bool,
    pub tls_protocol: String,
}

impl RemoteTransportEvidence {
    pub fn tls13(transport_id: impl Into<String>) -> Self {
        Self {
            transport_id: transport_id.into(),
            encrypted: true,
            server_authenticated: true,
            tls_protocol: "TLS1.3".into(),
        }
    }

    fn validate(&self) -> Result<(), RemoteHostError> {
        if self.transport_id.trim().is_empty() {
            return Err(RemoteHostError::TransportRejected("transport_id is empty".into()));
        }
        if !self.encrypted || !self.server_authenticated || self.tls_protocol != "TLS1.3" {
            return Err(RemoteHostError::TransportRejected(
                "remote session requires authenticated TLS 1.3 transport evidence".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAuthnCredential {
    pub token: String,
    pub expires_at_ms: u64,
    pub audience: String,
    pub origin: String,
}

#[derive(Debug, Clone)]
struct CredentialRecord {
    subject_id: String,
    expires_at_ms: u64,
    audience: String,
    origin: String,
    authn_level: String,
    claims: BTreeMap<String, String>,
    consumed: bool,
    epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RemoteSessionState {
    Active,
    Suspended,
    Closed,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePrincipal {
    pub subject_id: String,
    pub subject_kind: String,
    pub host_instance_id: String,
    pub session_id: String,
    pub transport_id: String,
    pub authn_level: String,
    pub claims: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteSessionSnapshot {
    pub session_id: String,
    pub state: RemoteSessionState,
    pub subject_id: String,
    pub transport_id: String,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub suspended_at_ms: Option<u64>,
    pub last_sequence: u64,
    pub inflight: u32,
    pub resume_token: String,
}

#[derive(Debug, Clone)]
struct RemoteSession {
    snapshot: RemoteSessionSnapshot,
    principal: RemotePrincipal,
    rate_window_started_at_ms: u64,
    requests_in_window: u32,
    recent_nonces: VecDeque<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RemoteHostError {
    #[error("invalid remote host config: {0}")]
    InvalidConfig(String),
    #[error("remote transport rejected: {0}")]
    TransportRejected(String),
    #[error("remote credential capacity exceeded")]
    CredentialCapacity,
    #[error("remote session capacity exceeded")]
    SessionCapacity,
    #[error("remote per-subject session quota exceeded")]
    SubjectSessionQuota,
    #[error("remote credential not found or already consumed")]
    CredentialUnavailable,
    #[error("remote credential expired")]
    CredentialExpired,
    #[error("remote credential audience mismatch")]
    AudienceMismatch,
    #[error("remote credential origin mismatch")]
    OriginMismatch,
    #[error("remote session not found")]
    SessionNotFound,
    #[error("remote session is not active")]
    SessionNotActive,
    #[error("remote session expired")]
    SessionExpired,
    #[error("remote resume token mismatch")]
    ResumeTokenMismatch,
    #[error("remote request sequence mismatch: expected {expected}, got {actual}")]
    SequenceMismatch { expected: u64, actual: u64 },
    #[error("remote request nonce is invalid or was already used")]
    NonceRejected,
    #[error("remote session rate limit exceeded")]
    RateLimited,
    #[error("remote session inflight quota exceeded")]
    InflightQuota,
    #[error("universal wire rejected remote frame: {0}")]
    Wire(String),
}

impl From<WireError> for RemoteHostError {
    fn from(value: WireError) -> Self {
        Self::Wire(value.to_string())
    }
}

pub struct RemoteHostSecurity {
    config: RemoteHostConfig,
    host_instance_id: String,
    credential_epoch: u64,
    credentials: HashMap<String, CredentialRecord>,
    sessions: HashMap<String, RemoteSession>,
}

impl RemoteHostSecurity {
    pub fn new(
        host_instance_id: impl Into<String>,
        config: RemoteHostConfig,
    ) -> Result<Self, RemoteHostError> {
        config.validate()?;
        let host_instance_id = host_instance_id.into();
        if host_instance_id.trim().is_empty() {
            return Err(RemoteHostError::InvalidConfig(
                "host_instance_id must be non-empty".into(),
            ));
        }
        Ok(Self {
            config,
            host_instance_id,
            credential_epoch: 1,
            credentials: HashMap::new(),
            sessions: HashMap::new(),
        })
    }

    pub fn config(&self) -> &RemoteHostConfig {
        &self.config
    }

    pub fn issue_credential(
        &mut self,
        subject_id: &str,
        origin: &str,
        authn_level: &str,
        claims: BTreeMap<String, String>,
        now_ms: u64,
    ) -> Result<RemoteAuthnCredential, RemoteHostError> {
        self.cleanup(now_ms);
        if subject_id.trim().is_empty() || authn_level.trim().is_empty() {
            return Err(RemoteHostError::InvalidConfig(
                "subject_id and authn_level must be non-empty".into(),
            ));
        }
        if !self.config.allowed_origins.iter().any(|allowed| allowed == origin) {
            return Err(RemoteHostError::OriginMismatch);
        }
        if self.credentials.len() >= self.config.max_credentials {
            return Err(RemoteHostError::CredentialCapacity);
        }

        let token = Uuid::new_v4().to_string();
        let expires_at_ms = now_ms.saturating_add(self.config.credential_ttl_ms);
        self.credentials.insert(
            token.clone(),
            CredentialRecord {
                subject_id: subject_id.to_string(),
                expires_at_ms,
                audience: self.config.audience.clone(),
                origin: origin.to_string(),
                authn_level: authn_level.to_string(),
                claims,
                consumed: false,
                epoch: self.credential_epoch,
            },
        );
        Ok(RemoteAuthnCredential {
            token,
            expires_at_ms,
            audience: self.config.audience.clone(),
            origin: origin.to_string(),
        })
    }

    pub fn rotate_credentials(&mut self) {
        self.credential_epoch = self.credential_epoch.saturating_add(1);
        self.credentials.clear();
    }

    pub fn close_all_sessions(&mut self) {
        for session in self.sessions.values_mut() {
            session.snapshot.state = RemoteSessionState::Closed;
            session.snapshot.inflight = 0;
        }
    }

    pub fn open_session(
        &mut self,
        credential_token: &str,
        audience: &str,
        origin: &str,
        transport: RemoteTransportEvidence,
        now_ms: u64,
    ) -> Result<RemoteSessionSnapshot, RemoteHostError> {
        transport.validate()?;
        self.cleanup(now_ms);

        if self
            .sessions
            .values()
            .filter(|session| {
                matches!(
                    session.snapshot.state,
                    RemoteSessionState::Active | RemoteSessionState::Suspended
                )
            })
            .count()
            >= self.config.max_sessions
        {
            return Err(RemoteHostError::SessionCapacity);
        }

        let record = self
            .credentials
            .get_mut(credential_token)
            .ok_or(RemoteHostError::CredentialUnavailable)?;
        if record.consumed || record.epoch != self.credential_epoch {
            return Err(RemoteHostError::CredentialUnavailable);
        }
        if now_ms >= record.expires_at_ms {
            return Err(RemoteHostError::CredentialExpired);
        }
        if audience != record.audience || audience != self.config.audience {
            return Err(RemoteHostError::AudienceMismatch);
        }
        if origin != record.origin
            || !self.config.allowed_origins.iter().any(|allowed| allowed == origin)
        {
            return Err(RemoteHostError::OriginMismatch);
        }

        let active_for_subject = self
            .sessions
            .values()
            .filter(|session| {
                session.principal.subject_id == record.subject_id
                    && matches!(
                        session.snapshot.state,
                        RemoteSessionState::Active | RemoteSessionState::Suspended
                    )
            })
            .count();
        if active_for_subject >= self.config.max_sessions_per_subject {
            return Err(RemoteHostError::SubjectSessionQuota);
        }

        record.consumed = true;
        let session_id = Uuid::new_v4().to_string();
        let principal = RemotePrincipal {
            subject_id: record.subject_id.clone(),
            subject_kind: "remote-user-session".into(),
            host_instance_id: self.host_instance_id.clone(),
            session_id: session_id.clone(),
            transport_id: transport.transport_id,
            authn_level: record.authn_level.clone(),
            claims: record.claims.clone(),
        };
        let snapshot = RemoteSessionSnapshot {
            session_id: session_id.clone(),
            state: RemoteSessionState::Active,
            subject_id: principal.subject_id.clone(),
            transport_id: principal.transport_id.clone(),
            created_at_ms: now_ms,
            last_activity_ms: now_ms,
            suspended_at_ms: None,
            last_sequence: 0,
            inflight: 0,
            resume_token: Uuid::new_v4().to_string(),
        };
        self.sessions.insert(
            session_id,
            RemoteSession {
                snapshot: snapshot.clone(),
                principal,
                rate_window_started_at_ms: now_ms,
                requests_in_window: 0,
                recent_nonces: VecDeque::new(),
            },
        );
        Ok(snapshot)
    }

    pub fn suspend(&mut self, session_id: &str, now_ms: u64) -> Result<(), RemoteHostError> {
        let session = self.session_mut_checked(session_id, now_ms)?;
        if session.snapshot.state != RemoteSessionState::Active {
            return Err(RemoteHostError::SessionNotActive);
        }
        session.snapshot.state = RemoteSessionState::Suspended;
        session.snapshot.suspended_at_ms = Some(now_ms);
        session.snapshot.inflight = 0;
        Ok(())
    }

    pub fn resume(
        &mut self,
        session_id: &str,
        resume_token: &str,
        transport: RemoteTransportEvidence,
        now_ms: u64,
    ) -> Result<RemoteSessionSnapshot, RemoteHostError> {
        transport.validate()?;
        self.expire_session_if_needed(session_id, now_ms)?;
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RemoteHostError::SessionNotFound)?;
        if session.snapshot.state != RemoteSessionState::Suspended {
            return Err(RemoteHostError::SessionNotActive);
        }
        if session.snapshot.resume_token != resume_token {
            return Err(RemoteHostError::ResumeTokenMismatch);
        }

        session.snapshot.state = RemoteSessionState::Active;
        session.snapshot.suspended_at_ms = None;
        session.snapshot.last_activity_ms = now_ms;
        session.snapshot.transport_id = transport.transport_id.clone();
        session.snapshot.resume_token = Uuid::new_v4().to_string();
        session.principal.transport_id = transport.transport_id;
        Ok(session.snapshot.clone())
    }

    pub fn close(&mut self, session_id: &str) -> Result<(), RemoteHostError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RemoteHostError::SessionNotFound)?;
        session.snapshot.state = RemoteSessionState::Closed;
        session.snapshot.inflight = 0;
        Ok(())
    }

    pub fn snapshot(&self, session_id: &str) -> Option<RemoteSessionSnapshot> {
        self.sessions.get(session_id).map(|session| session.snapshot.clone())
    }

    pub fn begin_request(
        &mut self,
        session_id: &str,
        sequence: u64,
        nonce: &str,
        now_ms: u64,
    ) -> Result<RemotePrincipal, RemoteHostError> {
        self.expire_session_if_needed(session_id, now_ms)?;
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RemoteHostError::SessionNotFound)?;
        if session.snapshot.state != RemoteSessionState::Active {
            return Err(RemoteHostError::SessionNotActive);
        }

        let expected = session.snapshot.last_sequence.saturating_add(1);
        if sequence != expected {
            return Err(RemoteHostError::SequenceMismatch {
                expected,
                actual: sequence,
            });
        }
        if nonce.is_empty()
            || nonce.len() > MAX_NONCE_BYTES
            || session.recent_nonces.iter().any(|existing| existing == nonce)
        {
            return Err(RemoteHostError::NonceRejected);
        }
        if now_ms.saturating_sub(session.rate_window_started_at_ms) >= self.config.rate_window_ms {
            session.rate_window_started_at_ms = now_ms;
            session.requests_in_window = 0;
        }
        if session.requests_in_window >= self.config.max_requests_per_window {
            return Err(RemoteHostError::RateLimited);
        }
        if session.snapshot.inflight >= self.config.max_inflight_per_session {
            return Err(RemoteHostError::InflightQuota);
        }

        session.snapshot.last_sequence = sequence;
        session.snapshot.last_activity_ms = now_ms;
        session.snapshot.inflight += 1;
        session.requests_in_window += 1;
        session.recent_nonces.push_back(nonce.to_string());
        while session.recent_nonces.len() > self.config.nonce_window {
            session.recent_nonces.pop_front();
        }
        Ok(session.principal.clone())
    }

    pub fn finish_request(&mut self, session_id: &str, now_ms: u64) -> Result<(), RemoteHostError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RemoteHostError::SessionNotFound)?;
        session.snapshot.inflight = session.snapshot.inflight.saturating_sub(1);
        session.snapshot.last_activity_ms = now_ms;
        Ok(())
    }

    pub fn handle_wire(
        &mut self,
        session_id: &str,
        sequence: u64,
        nonce: &str,
        now_ms: u64,
        bytes: &[u8],
    ) -> Result<Vec<u8>, RemoteHostError> {
        self.begin_request(session_id, sequence, nonce, now_ms)?;
        let result = (|| {
            let frame: WireFrame<serde_json::Value> =
                decode_wire_json(bytes, DEFAULT_MAX_WIRE_BYTES)?;
            Ok::<_, RemoteHostError>(encode_wire_json(&frame, DEFAULT_MAX_WIRE_BYTES)?)
        })();
        let finish = self.finish_request(session_id, now_ms);
        match (result, finish) {
            (Ok(bytes), Ok(())) => Ok(bytes),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    pub fn cleanup(&mut self, now_ms: u64) {
        self.credentials
            .retain(|_, credential| !credential.consumed && now_ms < credential.expires_at_ms);
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            let _ = self.expire_session_if_needed(&id, now_ms);
        }
        self.sessions.retain(|_, session| {
            !matches!(
                session.snapshot.state,
                RemoteSessionState::Closed | RemoteSessionState::Expired
            )
        });
    }

    pub fn credential_count(&self) -> usize {
        self.credentials.len()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    fn session_mut_checked(
        &mut self,
        session_id: &str,
        now_ms: u64,
    ) -> Result<&mut RemoteSession, RemoteHostError> {
        self.expire_session_if_needed(session_id, now_ms)?;
        self.sessions
            .get_mut(session_id)
            .ok_or(RemoteHostError::SessionNotFound)
    }

    fn expire_session_if_needed(
        &mut self,
        session_id: &str,
        now_ms: u64,
    ) -> Result<(), RemoteHostError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RemoteHostError::SessionNotFound)?;
        if matches!(
            session.snapshot.state,
            RemoteSessionState::Closed | RemoteSessionState::Expired
        ) {
            return Err(RemoteHostError::SessionExpired);
        }

        let absolute_expired = now_ms.saturating_sub(session.snapshot.created_at_ms)
            >= self.config.absolute_session_age_ms;
        let active_idle_expired = session.snapshot.state == RemoteSessionState::Active
            && now_ms.saturating_sub(session.snapshot.last_activity_ms)
                >= self.config.idle_timeout_ms;
        let resume_expired = session.snapshot.state == RemoteSessionState::Suspended
            && session.snapshot.suspended_at_ms.is_some_and(|at| {
                now_ms.saturating_sub(at) > self.config.resume_window_ms
            });

        if absolute_expired || active_idle_expired || resume_expired {
            session.snapshot.state = RemoteSessionState::Expired;
            session.snapshot.inflight = 0;
            return Err(RemoteHostError::SessionExpired);
        }
        Ok(())
    }
}
