//! V4 Local Host Broker core (A86).
//!
//! The platform adapter owns endpoint ACLs / Unix peer credentials / Windows SID evidence.
//! This module owns the cross-platform invariants: single active owner generation, bounded
//! one-time challenges, bootstrap-secret proof, replay rejection and stale-generation fencing.

use std::collections::{HashMap, VecDeque};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use uuid::Uuid;

const MAX_PENDING_CHALLENGES: usize = 128;
type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalHostLease {
    pub owner_id: String,
    pub endpoint_id: String,
    pub generation: u64,
    pub lease_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerCredentialEvidence {
    /// Platform-derived identity (UID/SID/process identity); never copied from client payload.
    pub platform_subject: String,
    /// Adapter proved the peer is allowed to use the protected per-user endpoint.
    pub endpoint_owner_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerChallenge {
    pub challenge_id: String,
    pub nonce: String,
    pub owner_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChallengeRecord {
    peer_subject: String,
    nonce: String,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedPeer {
    pub platform_subject: String,
    pub owner_generation: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LocalHostBrokerError {
    #[error("local host already owned by {owner_id}")]
    Busy { owner_id: String },
    #[error("local host has no active owner")]
    NoOwner,
    #[error("local host lease is stale")]
    StaleLease,
    #[error("platform peer credential verification failed")]
    PeerCredentialRejected,
    #[error("peer challenge is missing, expired, or already consumed")]
    ChallengeUnavailable,
    #[error("peer proof is invalid")]
    InvalidProof,
}

#[derive(Debug)]
pub struct LocalHostBroker {
    secret: Vec<u8>,
    active: Option<LocalHostLease>,
    next_generation: u64,
    challenges: HashMap<String, ChallengeRecord>,
    challenge_order: VecDeque<String>,
}

impl LocalHostBroker {
    pub fn new(bootstrap_secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: bootstrap_secret.into(),
            active: None,
            next_generation: 1,
            challenges: HashMap::new(),
            challenge_order: VecDeque::new(),
        }
    }

    pub fn active_lease(&self) -> Option<&LocalHostLease> {
        self.active.as_ref()
    }

    pub fn acquire(
        &mut self,
        owner_id: &str,
        endpoint_id: &str,
    ) -> Result<LocalHostLease, LocalHostBrokerError> {
        if let Some(active) = &self.active {
            return Err(LocalHostBrokerError::Busy { owner_id: active.owner_id.clone() });
        }
        let lease = LocalHostLease {
            owner_id: owner_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            generation: self.next_generation,
            lease_token: Uuid::new_v4().to_string(),
        };
        self.next_generation = self.next_generation.saturating_add(1);
        self.active = Some(lease.clone());
        Ok(lease)
    }

    pub fn release(&mut self, lease: &LocalHostLease) -> Result<(), LocalHostBrokerError> {
        match &self.active {
            Some(active) if active == lease => {
                self.active = None;
                self.challenges.clear();
                self.challenge_order.clear();
                Ok(())
            }
            Some(_) => Err(LocalHostBrokerError::StaleLease),
            None => Err(LocalHostBrokerError::NoOwner),
        }
    }

    /// Platform adapter may reclaim an endpoint only after proving the previous owner is gone.
    pub fn reclaim_stale(
        &mut self,
        observed: &LocalHostLease,
        previous_owner_alive: bool,
        new_owner_id: &str,
        endpoint_id: &str,
    ) -> Result<LocalHostLease, LocalHostBrokerError> {
        if previous_owner_alive {
            let owner_id = self
                .active
                .as_ref()
                .map(|x| x.owner_id.clone())
                .unwrap_or_else(|| observed.owner_id.clone());
            return Err(LocalHostBrokerError::Busy { owner_id });
        }
        if self.active.as_ref() != Some(observed) {
            return Err(LocalHostBrokerError::StaleLease);
        }
        self.active = None;
        self.challenges.clear();
        self.challenge_order.clear();
        self.acquire(new_owner_id, endpoint_id)
    }

    pub fn challenge(
        &mut self,
        evidence: &PeerCredentialEvidence,
    ) -> Result<PeerChallenge, LocalHostBrokerError> {
        if !evidence.endpoint_owner_verified || evidence.platform_subject.trim().is_empty() {
            return Err(LocalHostBrokerError::PeerCredentialRejected);
        }
        let generation = self.active.as_ref().ok_or(LocalHostBrokerError::NoOwner)?.generation;

        while self.challenge_order.len() >= MAX_PENDING_CHALLENGES {
            if let Some(oldest) = self.challenge_order.pop_front() {
                self.challenges.remove(&oldest);
            }
        }

        let challenge_id = Uuid::new_v4().to_string();
        let nonce = Uuid::new_v4().to_string();
        self.challenges.insert(
            challenge_id.clone(),
            ChallengeRecord {
                peer_subject: evidence.platform_subject.clone(),
                nonce: nonce.clone(),
                generation,
            },
        );
        self.challenge_order.push_back(challenge_id.clone());
        Ok(PeerChallenge { challenge_id, nonce, owner_generation: generation })
    }

    pub fn verify(
        &mut self,
        evidence: &PeerCredentialEvidence,
        challenge_id: &str,
        proof: &[u8],
    ) -> Result<AuthenticatedPeer, LocalHostBrokerError> {
        if !evidence.endpoint_owner_verified {
            return Err(LocalHostBrokerError::PeerCredentialRejected);
        }
        let record = self
            .challenges
            .remove(challenge_id)
            .ok_or(LocalHostBrokerError::ChallengeUnavailable)?;
        self.challenge_order.retain(|id| id != challenge_id);

        let active_generation =
            self.active.as_ref().ok_or(LocalHostBrokerError::NoOwner)?.generation;
        if record.generation != active_generation
            || record.peer_subject != evidence.platform_subject
        {
            return Err(LocalHostBrokerError::StaleLease);
        }

        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("HMAC accepts any key length");
        mac.update(&proof_message(&record.peer_subject, &record.nonce, record.generation));
        if mac.verify_slice(proof).is_err() {
            return Err(LocalHostBrokerError::InvalidProof);
        }

        Ok(AuthenticatedPeer {
            platform_subject: evidence.platform_subject.clone(),
            owner_generation: record.generation,
        })
    }
}

pub fn peer_proof(secret: &[u8], peer_subject: &str, nonce: &str, generation: u64) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&proof_message(peer_subject, nonce, generation));
    mac.finalize().into_bytes().to_vec()
}

fn proof_message(peer_subject: &str, nonce: &str, generation: u64) -> Vec<u8> {
    format!("tauron-local-host-v1\0{peer_subject}\0{nonce}\0{generation}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(subject: &str) -> PeerCredentialEvidence {
        PeerCredentialEvidence {
            platform_subject: subject.to_string(),
            endpoint_owner_verified: true,
        }
    }

    #[test]
    fn single_instance_lease_rejects_second_owner() {
        let mut broker = LocalHostBroker::new(b"secret");
        broker.acquire("host-1", "endpoint").unwrap();
        assert!(matches!(
            broker.acquire("host-2", "endpoint"),
            Err(LocalHostBrokerError::Busy { .. })
        ));
    }

    #[test]
    fn peer_auth_requires_platform_evidence_and_one_time_challenge() {
        let mut broker = LocalHostBroker::new(b"secret");
        broker.acquire("host-1", "endpoint").unwrap();
        let ev = evidence("uid:1000");
        let challenge = broker.challenge(&ev).unwrap();
        let proof = peer_proof(
            b"secret",
            &ev.platform_subject,
            &challenge.nonce,
            challenge.owner_generation,
        );
        let peer = broker.verify(&ev, &challenge.challenge_id, &proof).unwrap();
        assert_eq!(peer.platform_subject, "uid:1000");
        assert_eq!(
            broker.verify(&ev, &challenge.challenge_id, &proof),
            Err(LocalHostBrokerError::ChallengeUnavailable)
        );
    }

    #[test]
    fn wrong_peer_or_secret_cannot_authenticate() {
        let mut broker = LocalHostBroker::new(b"secret");
        broker.acquire("host-1", "endpoint").unwrap();
        let ev = evidence("sid:user-a");
        let challenge = broker.challenge(&ev).unwrap();
        let bad =
            peer_proof(b"other-secret", "sid:user-a", &challenge.nonce, challenge.owner_generation);
        assert_eq!(
            broker.verify(&ev, &challenge.challenge_id, &bad),
            Err(LocalHostBrokerError::InvalidProof)
        );
    }

    #[test]
    fn stale_takeover_requires_platform_proof_owner_is_gone_and_fences_generation() {
        let mut broker = LocalHostBroker::new(b"secret");
        let first = broker.acquire("host-1", "endpoint").unwrap();
        assert!(matches!(
            broker.reclaim_stale(&first, true, "host-2", "endpoint"),
            Err(LocalHostBrokerError::Busy { .. })
        ));
        let second = broker.reclaim_stale(&first, false, "host-2", "endpoint").unwrap();
        assert!(second.generation > first.generation);
        assert_eq!(broker.release(&first), Err(LocalHostBrokerError::StaleLease));
    }
}
