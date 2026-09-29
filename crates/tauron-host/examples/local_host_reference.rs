//! Non-Tauri Local Host reference application (V4 A106).
//!
//! This deliberately depends only on tauron-host. Platform-specific endpoint creation and
//! peer-credential collection live in adapters; the universal broker/authentication contract
//! remains reusable by Electron/Qt/.NET/headless service bridges.

use tauron_host::{peer_proof, LocalHostBroker, PeerCredentialEvidence};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // In a real adapter this secret is created once and exposed only through the protected
    // per-user endpoint bootstrap channel, never through an untrusted client payload.
    let secret = b"reference-only-bootstrap-secret";
    let mut broker = LocalHostBroker::new(secret.as_slice());

    let lease = broker.acquire("reference-host", "local-reference-endpoint")?;
    let peer = PeerCredentialEvidence {
        platform_subject: "reference-user".to_string(),
        endpoint_owner_verified: true,
    };

    let challenge = broker.challenge(&peer)?;
    let proof =
        peer_proof(secret, &peer.platform_subject, &challenge.nonce, challenge.owner_generation);
    let authenticated = broker.verify(&peer, &challenge.challenge_id, &proof)?;

    assert_eq!(authenticated.owner_generation, lease.generation);
    assert_eq!(authenticated.platform_subject, peer.platform_subject);
    broker.release(&lease)?;

    println!("local-host-reference: authenticated and cleanly released");
    Ok(())
}
