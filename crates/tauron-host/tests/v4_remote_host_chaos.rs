//! V4 A108 Remote Host contract chaos gate.
//!
//! This proves the bounded security/session authority and canonical Universal Wire consumer path.
//! TLS socket coverage remains a separate official reference-adapter gate.

use std::collections::BTreeMap;

use tauron_host::{
    encode_wire_json, RemoteHostConfig, RemoteHostError, RemoteHostSecurity, RemoteSessionState,
    RemoteTransportEvidence, WireFrame, DEFAULT_MAX_WIRE_BYTES,
};

fn config() -> RemoteHostConfig {
    RemoteHostConfig {
        audience: "tauron-chaos".into(),
        allowed_origins: vec!["https://app.example".into()],
        credential_ttl_ms: 100,
        idle_timeout_ms: 100,
        absolute_session_age_ms: 1_000,
        resume_window_ms: 40,
        rate_window_ms: 50,
        max_requests_per_window: 3,
        max_inflight_per_session: 2,
        nonce_window: 4,
        max_credentials: 8,
        max_sessions: 8,
        max_sessions_per_subject: 1,
    }
}

fn security() -> RemoteHostSecurity {
    RemoteHostSecurity::new("chaos-host", config()).unwrap()
}

fn open(security: &mut RemoteHostSecurity, now_ms: u64) -> tauron_host::RemoteSessionSnapshot {
    let credential = security
        .issue_credential(
            "user-1",
            "https://app.example",
            "oidc+tls",
            BTreeMap::from([("tenant".into(), "t-1".into())]),
            now_ms,
        )
        .unwrap();
    security
        .open_session(
            &credential.token,
            "tauron-chaos",
            "https://app.example",
            RemoteTransportEvidence::tls13("tls-conn-1"),
            now_ms,
        )
        .unwrap()
}

#[test]
fn credential_and_transport_checks_are_fail_closed() {
    let mut security = security();
    let credential = security
        .issue_credential(
            "user-1",
            "https://app.example",
            "oidc",
            BTreeMap::new(),
            0,
        )
        .unwrap();

    assert_eq!(
        security.open_session(
            &credential.token,
            "wrong-audience",
            "https://app.example",
            RemoteTransportEvidence::tls13("tls-1"),
            1,
        ),
        Err(RemoteHostError::AudienceMismatch)
    );

    let plain = RemoteTransportEvidence {
        transport_id: "tcp".into(),
        encrypted: false,
        server_authenticated: false,
        tls_protocol: "none".into(),
    };
    assert!(matches!(
        security.open_session(
            &credential.token,
            "tauron-chaos",
            "https://app.example",
            plain,
            1,
        ),
        Err(RemoteHostError::TransportRejected(_))
    ));

    assert_eq!(
        security.open_session(
            &credential.token,
            "tauron-chaos",
            "https://evil.example",
            RemoteTransportEvidence::tls13("tls-2"),
            1,
        ),
        Err(RemoteHostError::OriginMismatch)
    );

    assert_eq!(
        security.open_session(
            &credential.token,
            "tauron-chaos",
            "https://app.example",
            RemoteTransportEvidence::tls13("tls-3"),
            100,
        ),
        Err(RemoteHostError::CredentialExpired)
    );
}

#[test]
fn replay_gap_nonce_rate_and_inflight_are_bounded() {
    let mut security = security();
    let session = open(&mut security, 0);

    security.begin_request(&session.session_id, 1, "n1", 1).unwrap();
    security.begin_request(&session.session_id, 2, "n2", 2).unwrap();
    assert_eq!(
        security.begin_request(&session.session_id, 3, "n3", 3),
        Err(RemoteHostError::InflightQuota)
    );
    security.finish_request(&session.session_id, 3).unwrap();

    assert_eq!(
        security.begin_request(&session.session_id, 2, "replay", 4),
        Err(RemoteHostError::SequenceMismatch {
            expected: 3,
            actual: 2,
        })
    );
    assert_eq!(
        security.begin_request(&session.session_id, 4, "gap", 4),
        Err(RemoteHostError::SequenceMismatch {
            expected: 3,
            actual: 4,
        })
    );
    assert_eq!(
        security.begin_request(&session.session_id, 3, "n1", 4),
        Err(RemoteHostError::NonceRejected)
    );

    security.begin_request(&session.session_id, 3, "n3", 4).unwrap();
    security.finish_request(&session.session_id, 4).unwrap();
    assert_eq!(
        security.begin_request(&session.session_id, 4, "n4", 5),
        Err(RemoteHostError::RateLimited)
    );

    security.begin_request(&session.session_id, 4, "n4", 51).unwrap();
    security.finish_request(&session.session_id, 51).unwrap();
}

#[test]
fn suspend_resume_rotates_token_and_expires() {
    let mut security = security();
    let session = open(&mut security, 0);
    security.suspend(&session.session_id, 10).unwrap();

    assert_eq!(
        security.resume(
            &session.session_id,
            "wrong",
            RemoteTransportEvidence::tls13("tls-2"),
            20,
        ),
        Err(RemoteHostError::ResumeTokenMismatch)
    );

    let resumed = security
        .resume(
            &session.session_id,
            &session.resume_token,
            RemoteTransportEvidence::tls13("tls-3"),
            20,
        )
        .unwrap();
    assert_eq!(resumed.state, RemoteSessionState::Active);
    assert_ne!(resumed.resume_token, session.resume_token);

    security.suspend(&session.session_id, 25).unwrap();
    assert_eq!(
        security.resume(
            &session.session_id,
            &resumed.resume_token,
            RemoteTransportEvidence::tls13("tls-4"),
            66,
        ),
        Err(RemoteHostError::SessionExpired)
    );
}

#[test]
fn rotation_quota_and_at_most_once_wire_are_enforced() {
    let mut security = security();
    let session = open(&mut security, 0);

    let second = security
        .issue_credential(
            "user-1",
            "https://app.example",
            "oidc",
            BTreeMap::new(),
            1,
        )
        .unwrap();
    assert_eq!(
        security.open_session(
            &second.token,
            "tauron-chaos",
            "https://app.example",
            RemoteTransportEvidence::tls13("tls-2"),
            2,
        ),
        Err(RemoteHostError::SubjectSessionQuota)
    );

    assert!(matches!(
        security.handle_wire(&session.session_id, 1, "wire-1", 3, b"{"),
        Err(RemoteHostError::Wire(_))
    ));
    assert_eq!(security.snapshot(&session.session_id).unwrap().inflight, 0);

    let frame = WireFrame::new("remote.echo/1", 1, serde_json::json!({"ok":true}));
    let bytes = encode_wire_json(&frame, DEFAULT_MAX_WIRE_BYTES).unwrap();
    assert!(matches!(
        security.handle_wire(&session.session_id, 1, "wire-2", 4, &bytes),
        Err(RemoteHostError::SequenceMismatch { .. })
    ));
    assert!(!security
        .handle_wire(&session.session_id, 2, "wire-2", 4, &bytes)
        .unwrap()
        .is_empty());

    security.close(&session.session_id).unwrap();
    security.cleanup(5);
    let credential = security
        .issue_credential(
            "user-2",
            "https://app.example",
            "oidc",
            BTreeMap::new(),
            5,
        )
        .unwrap();
    security.rotate_credentials();
    assert_eq!(
        security.open_session(
            &credential.token,
            "tauron-chaos",
            "https://app.example",
            RemoteTransportEvidence::tls13("tls-3"),
            6,
        ),
        Err(RemoteHostError::CredentialUnavailable)
    );
}
