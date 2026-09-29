//! V4 A108 local chaos / peer-auth gate.
//!
//! This suite is intentionally limited to the Local Host implementation that exists today.
//! It does not claim Remote chaos coverage before a real Remote transport is present.

use tauron_host::{
    peer_proof, LocalHostBroker, LocalHostBrokerError, PeerCredentialEvidence,
};

fn evidence(subject: &str) -> PeerCredentialEvidence {
    PeerCredentialEvidence {
        platform_subject: subject.to_string(),
        endpoint_owner_verified: true,
    }
}

#[test]
fn challenge_flood_is_bounded_and_oldest_tokens_expire() {
    let mut broker = LocalHostBroker::new(b"chaos-secret");
    broker.acquire("host-a", "endpoint").unwrap();
    let ev = evidence("uid:chaos");
    let first = broker.challenge(&ev).unwrap();

    for _ in 0..192 {
        broker.challenge(&ev).unwrap();
    }
    let proof = peer_proof(
        b"chaos-secret",
        &ev.platform_subject,
        &first.nonce,
        first.owner_generation,
    );
    assert_eq!(
        broker.verify(&ev, &first.challenge_id, &proof),
        Err(LocalHostBrokerError::ChallengeUnavailable)
    );
}

#[test]
fn replay_and_wrong_secret_never_authenticate() {
    let mut broker = LocalHostBroker::new(b"chaos-secret");
    broker.acquire("host-a", "endpoint").unwrap();
    let ev = evidence("sid:chaos");

    let wrong = broker.challenge(&ev).unwrap();
    let bad = peer_proof(
        b"wrong-secret",
        &ev.platform_subject,
        &wrong.nonce,
        wrong.owner_generation,
    );
    assert_eq!(
        broker.verify(&ev, &wrong.challenge_id, &bad),
        Err(LocalHostBrokerError::InvalidProof)
    );
    assert_eq!(
        broker.verify(&ev, &wrong.challenge_id, &bad),
        Err(LocalHostBrokerError::ChallengeUnavailable)
    );

    let good = broker.challenge(&ev).unwrap();
    let proof = peer_proof(
        b"chaos-secret",
        &ev.platform_subject,
        &good.nonce,
        good.owner_generation,
    );
    broker.verify(&ev, &good.challenge_id, &proof).unwrap();
    assert_eq!(
        broker.verify(&ev, &good.challenge_id, &proof),
        Err(LocalHostBrokerError::ChallengeUnavailable)
    );
}

#[test]
fn takeover_fences_all_pre_takeover_auth_material() {
    let mut broker = LocalHostBroker::new(b"chaos-secret");
    let old_lease = broker.acquire("host-a", "endpoint").unwrap();
    let ev = evidence("uid:chaos");
    let old_challenge = broker.challenge(&ev).unwrap();
    let old_proof = peer_proof(
        b"chaos-secret",
        &ev.platform_subject,
        &old_challenge.nonce,
        old_challenge.owner_generation,
    );

    let new_lease =
        broker.reclaim_stale(&old_lease, false, "host-b", "endpoint").unwrap();
    assert!(new_lease.generation > old_lease.generation);
    assert_eq!(
        broker.verify(&ev, &old_challenge.challenge_id, &old_proof),
        Err(LocalHostBrokerError::ChallengeUnavailable)
    );
    assert_eq!(broker.release(&old_lease), Err(LocalHostBrokerError::StaleLease));
}

#[test]
fn repeated_owner_crash_takeover_cycles_keep_generation_monotonic() {
    let mut broker = LocalHostBroker::new(b"chaos-secret");
    let mut lease = broker.acquire("host-0", "endpoint").unwrap();
    let mut previous_generation = lease.generation;

    for i in 1..=256 {
        lease = broker
            .reclaim_stale(&lease, false, &format!("host-{i}"), "endpoint")
            .unwrap();
        assert!(lease.generation > previous_generation);
        previous_generation = lease.generation;

        let ev = evidence(&format!("uid:{i}"));
        let challenge = broker.challenge(&ev).unwrap();
        let proof = peer_proof(
            b"chaos-secret",
            &ev.platform_subject,
            &challenge.nonce,
            challenge.owner_generation,
        );
        let peer = broker.verify(&ev, &challenge.challenge_id, &proof).unwrap();
        assert_eq!(peer.owner_generation, lease.generation);
    }
}
