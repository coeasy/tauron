//! V4 Host Conformance baseline.
//!
//! This is intentionally transport-neutral and GUI-free. Every official/ref host must preserve
//! these semantics before transport-specific conformance is layered on top.

use std::process::Command;

use serde::{Deserialize, Serialize};
use tauron_host::{
    decode_wire_json, encode_wire_json, peer_proof, production_doctor, Degradation, DeploymentMode,
    HealthReport, Liveness, LocalHostBroker, OrderedEventMeta, OrderingError, OrderingTracker,
    PeerCredentialEvidence, PersistentWriterLease, ProductionReadiness, Readiness, StorageNamespace,
    WireFrame, WriterLeaseError, DEFAULT_MAX_WIRE_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ping {
    value: String,
}

fn production_ready() -> ProductionReadiness {
    ProductionReadiness {
        caller_identity_policy_enabled: true,
        durable_recovery_available: true,
        recovery_explicitly_unsupported: false,
        install_feature_enabled: false,
        install_trust_configured: false,
        audit_for_admin_operations_available: true,
        writable_data_dir_available: true,
        process_runtime_enabled: false,
        hard_process_sandbox_available: false,
        mock_provider_enabled: false,
    }
}

#[test]
fn conform_wire_round_trip_and_unknown_field_rejection() {
    let frame = WireFrame::new("conformance.ping/1", 9, Ping { value: "pong".into() });
    let bytes = encode_wire_json(&frame, DEFAULT_MAX_WIRE_BYTES).unwrap();
    let decoded: WireFrame<Ping> = decode_wire_json(&bytes, DEFAULT_MAX_WIRE_BYTES).unwrap();
    assert_eq!(decoded, frame);

    let bad = br#"{"header":{"wireVersion":1,"codec":"json-v1","schema":"conformance.ping/1","generation":9},"payload":{"value":"pong"},"unexpected":true}"#;
    assert!(decode_wire_json::<Ping>(bad, DEFAULT_MAX_WIRE_BYTES).is_err());
}

#[test]
fn conform_local_host_peer_auth_is_one_time_and_owner_scoped() {
    let secret = b"conformance-bootstrap-secret";
    let mut broker = LocalHostBroker::new(secret.as_slice());
    let lease = broker.acquire("host-a", "endpoint-a").unwrap();
    let peer = PeerCredentialEvidence {
        platform_subject: "uid:conformance".into(),
        endpoint_owner_verified: true,
    };
    let challenge = broker.challenge(&peer).unwrap();
    let proof =
        peer_proof(secret, &peer.platform_subject, &challenge.nonce, challenge.owner_generation);
    let authenticated = broker.verify(&peer, &challenge.challenge_id, &proof).unwrap();
    assert_eq!(authenticated.owner_generation, lease.generation);
    assert!(broker.verify(&peer, &challenge.challenge_id, &proof).is_err());
    broker.release(&lease).unwrap();
}

#[test]
fn conform_production_ready_requires_all_security_facts() {
    let report = production_doctor(DeploymentMode::Production, &production_ready());
    assert!(report.production_safe);
    assert!(report.checks.iter().all(|check| check.pass));

    let mut unsafe_input = production_ready();
    unsafe_input.mock_provider_enabled = true;
    let report = production_doctor(DeploymentMode::Production, &unsafe_input);
    assert!(!report.production_safe);
    assert!(report.checks.iter().any(|check| check.id == "no-mock-provider" && !check.pass));
}

#[test]
fn conform_health_does_not_equate_alive_with_ready() {
    let report = HealthReport {
        liveness: Liveness::Alive,
        readiness: Readiness::NotReady,
        degradation: Degradation::Degraded,
        diagnostics: vec!["provider-reconcile-required".into()],
    };
    assert!(!report.can_accept_work());
}

#[test]
fn conform_ordering_detects_duplicate_gap_and_revision_regression() {
    let mut tracker = OrderingTracker::default();
    let first = OrderedEventMeta {
        event_id: "evt-1".into(),
        sender: "client".into(),
        receiver: "host".into(),
        sequence: 1,
        state_revision: Some(10),
        causation_id: None,
    };
    tracker.observe(&first).unwrap();

    let gap = OrderedEventMeta { sequence: 3, ..first.clone() };
    assert!(matches!(tracker.observe(&gap), Err(OrderingError::Gap { .. })));

    let duplicate = OrderedEventMeta { sequence: 1, ..first.clone() };
    assert!(matches!(tracker.observe(&duplicate), Err(OrderingError::Duplicate { .. })));

    let regressed = OrderedEventMeta {
        event_id: "evt-2".into(),
        sequence: 2,
        state_revision: Some(9),
        ..first
    };
    assert!(matches!(
        tracker.observe(&regressed),
        Err(OrderingError::RevisionRegression { previous: 10, actual: 9 })
    ));
}


fn storage_namespace() -> StorageNamespace {
    StorageNamespace {
        tenant: "default".into(),
        application: "conformance".into(),
        principal: "registry".into(),
    }
}

#[test]
fn storage_lease_child_probe() {
    let Ok(root) = std::env::var("TAURON_STORAGE_LEASE_PROBE_ROOT") else {
        return;
    };
    let expectation =
        std::env::var("TAURON_STORAGE_LEASE_PROBE_EXPECT").unwrap_or_else(|_| "busy".into());
    let result =
        PersistentWriterLease::acquire(std::path::Path::new(&root), storage_namespace(), "child");
    match expectation.as_str() {
        "busy" => assert!(
            matches!(result, Err(WriterLeaseError::Busy { .. })),
            "independent process must observe the active writer lock"
        ),
        "acquire" => assert!(
            result.is_ok(),
            "independent process must acquire after the prior owner exits: {result:?}"
        ),
        other => panic!("unknown storage probe expectation: {other}"),
    }
}

#[test]
fn conform_storage_single_writer_uses_cross_process_file_lock() {
    let root = tempfile::tempdir().unwrap();
    let first =
        PersistentWriterLease::acquire(root.path(), storage_namespace(), "host-a").unwrap();
    let first_epoch = first.lease().epoch;

    let current_exe = std::env::current_exe().unwrap();
    let blocked = Command::new(&current_exe)
        .arg("--exact")
        .arg("storage_lease_child_probe")
        .arg("--nocapture")
        .env("TAURON_STORAGE_LEASE_PROBE_ROOT", root.path())
        .env("TAURON_STORAGE_LEASE_PROBE_EXPECT", "busy")
        .status()
        .unwrap();
    assert!(blocked.success(), "child process did not prove the active lock");

    drop(first);

    let acquired = Command::new(&current_exe)
        .arg("--exact")
        .arg("storage_lease_child_probe")
        .arg("--nocapture")
        .env("TAURON_STORAGE_LEASE_PROBE_ROOT", root.path())
        .env("TAURON_STORAGE_LEASE_PROBE_EXPECT", "acquire")
        .status()
        .unwrap();
    assert!(acquired.success(), "child process could not acquire after owner drop");

    let second =
        PersistentWriterLease::acquire(root.path(), storage_namespace(), "host-b").unwrap();
    assert!(
        second.lease().epoch > first_epoch,
        "persistent fencing epoch must advance across process ownership changes"
    );
}
