//! Non-Tauri Local Host reference transport helpers (V4 A106).
//!
//! The canonical broker/authentication rules live in `local_host`. This module supplies the
//! first real transport consumer: Linux Unix Domain Sockets protected by owner-only filesystem
//! permissions plus kernel-derived `SO_PEERCRED` evidence. The wire payload is the same
//! `WireFrame<json-v1>` used by every other transport.
//!
//! Honest support boundary:
//! - Linux: reference endpoint + peer credential derivation are implemented.
//! - Other OSes: the transport-specific reference endpoint is not implemented here yet.
//! - Remote transport is out of scope; A108 remote chaos must remain open until a real Remote
//!   Host transport exists.

use serde_json::Value;
use thiserror::Error;

use crate::{
    decode_wire_json, encode_wire_json, LocalHostBrokerError, WireError, WireFrame,
    DEFAULT_MAX_WIRE_BYTES,
};

/// Control-plane packet ceiling (challenge/proof/ack), separate from the Universal Wire ceiling.
pub const REFERENCE_CONTROL_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum LocalHostReferenceError {
    #[error("local host I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("local host broker rejected operation: {0}")]
    Broker(#[from] LocalHostBrokerError),
    #[error("local host wire rejected frame: {0}")]
    Wire(#[from] WireError),
    #[error("local host protocol error: {0}")]
    Protocol(String),
    #[error("local host reference transport is unsupported on this platform")]
    UnsupportedPlatform,
}

/// Transport-neutral operation used by the reference server after peer authentication.
///
/// This is intentionally the canonical codec itself, not a parallel JSON interpretation.
pub fn reference_wire_roundtrip(bytes: &[u8]) -> Result<Vec<u8>, LocalHostReferenceError> {
    let frame: WireFrame<Value> = decode_wire_json(bytes, DEFAULT_MAX_WIRE_BYTES)?;
    Ok(encode_wire_json(&frame, DEFAULT_MAX_WIRE_BYTES)?)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::c_void;
    use std::fs;
    use std::io::{Read, Write};
    use std::mem::{size_of, MaybeUninit};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    use crate::{
        peer_proof, LocalHostBroker, LocalHostBrokerError, PeerChallenge, PeerCredentialEvidence,
        DEFAULT_MAX_WIRE_BYTES,
    };

    use super::{reference_wire_roundtrip, LocalHostReferenceError, REFERENCE_CONTROL_MAX_BYTES};

    fn write_packet(
        stream: &mut UnixStream,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<(), LocalHostReferenceError> {
        if bytes.len() > max_bytes || bytes.len() > u32::MAX as usize {
            return Err(LocalHostReferenceError::Protocol(format!(
                "packet too large: {} > {}",
                bytes.len(),
                max_bytes
            )));
        }
        stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
        stream.write_all(bytes)?;
        stream.flush()?;
        Ok(())
    }

    fn read_packet(
        stream: &mut UnixStream,
        max_bytes: usize,
    ) -> Result<Vec<u8>, LocalHostReferenceError> {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if len > max_bytes {
            return Err(LocalHostReferenceError::Protocol(format!(
                "packet length {len} exceeds limit {max_bytes}"
            )));
        }
        let mut bytes = vec![0u8; len];
        stream.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// Bind a protected Linux UDS endpoint.
    ///
    /// Parent directory is forced to 0700 and the socket node to 0600. Existing socket paths are
    /// never silently unlinked: stale endpoint takeover must first be proven through LocalHostBroker.
    pub fn bind_endpoint(path: &Path) -> Result<UnixListener, LocalHostReferenceError> {
        let parent = path.parent().ok_or_else(|| {
            LocalHostReferenceError::Protocol("socket path has no parent directory".into())
        })?;
        fs::create_dir_all(parent)?;
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(LocalHostReferenceError::Protocol(
                "local host parent must be a real directory, not a symlink".into(),
            ));
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        if path.exists() {
            return Err(LocalHostReferenceError::Protocol(
                "local host endpoint already exists; stale takeover must be explicit".into(),
            ));
        }
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Derive peer identity from the kernel, never from a client payload.
    pub fn peer_evidence(
        stream: &UnixStream,
    ) -> Result<PeerCredentialEvidence, LocalHostReferenceError> {
        let mut cred = MaybeUninit::<libc::ucred>::uninit();
        let mut len = size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: cred points to writable ucred storage and len describes that storage.
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                cred.as_mut_ptr().cast::<c_void>(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if len as usize != size_of::<libc::ucred>() {
            return Err(LocalHostReferenceError::Protocol(format!(
                "SO_PEERCRED returned unexpected size {len}"
            )));
        }
        // SAFETY: getsockopt succeeded and wrote the complete ucred struct.
        let cred = unsafe { cred.assume_init() };
        // SAFETY: geteuid has no preconditions.
        let server_uid = unsafe { libc::geteuid() };
        Ok(PeerCredentialEvidence {
            platform_subject: format!("linux:uid={}:pid={}", cred.uid, cred.pid),
            endpoint_owner_verified: cred.uid == server_uid,
        })
    }

    /// Subject used by a Linux client when calculating its HMAC proof.
    ///
    /// This value is not trusted by the server; the server independently derives the exact subject
    /// from SO_PEERCRED and Broker::verify compares it with the challenge record.
    pub fn current_client_subject() -> String {
        // SAFETY: getuid/getpid have no preconditions.
        let uid = unsafe { libc::getuid() };
        let pid = unsafe { libc::getpid() };
        format!("linux:uid={uid}:pid={pid}")
    }

    /// Serve one authenticated request/response exchange on an already protected listener.
    pub fn serve_one(
        listener: &UnixListener,
        broker: &mut LocalHostBroker,
    ) -> Result<(), LocalHostReferenceError> {
        let (mut stream, _) = listener.accept()?;
        let evidence = peer_evidence(&stream)?;
        let challenge = broker.challenge(&evidence)?;
        let challenge_json = serde_json::to_vec(&challenge)
            .map_err(|e| LocalHostReferenceError::Protocol(e.to_string()))?;
        write_packet(&mut stream, &challenge_json, REFERENCE_CONTROL_MAX_BYTES)?;

        let proof = read_packet(&mut stream, REFERENCE_CONTROL_MAX_BYTES)?;
        let peer = broker.verify(&evidence, &challenge.challenge_id, &proof)?;
        let active_generation =
            broker.active_lease().ok_or(LocalHostBrokerError::NoOwner)?.generation;
        if peer.owner_generation != active_generation {
            return Err(LocalHostReferenceError::Protocol(
                "authenticated peer generation became stale".into(),
            ));
        }
        write_packet(&mut stream, b"ok", REFERENCE_CONTROL_MAX_BYTES)?;

        let request = read_packet(&mut stream, DEFAULT_MAX_WIRE_BYTES)?;
        let response = reference_wire_roundtrip(&request)?;
        write_packet(&mut stream, &response, DEFAULT_MAX_WIRE_BYTES)?;
        Ok(())
    }

    /// Reference client for conformance/E2E. Production clients may be any language/transport
    /// binding that implements the same challenge + Universal Wire contract.
    pub fn client_roundtrip(
        path: &Path,
        bootstrap_secret: &[u8],
        request: &[u8],
    ) -> Result<Vec<u8>, LocalHostReferenceError> {
        let mut stream = UnixStream::connect(path)?;
        let challenge_json = read_packet(&mut stream, REFERENCE_CONTROL_MAX_BYTES)?;
        let challenge: PeerChallenge = serde_json::from_slice(&challenge_json)
            .map_err(|e| LocalHostReferenceError::Protocol(e.to_string()))?;
        let subject = current_client_subject();
        let proof =
            peer_proof(bootstrap_secret, &subject, &challenge.nonce, challenge.owner_generation);
        write_packet(&mut stream, &proof, REFERENCE_CONTROL_MAX_BYTES)?;
        let ack = read_packet(&mut stream, REFERENCE_CONTROL_MAX_BYTES)?;
        if ack != b"ok" {
            return Err(LocalHostReferenceError::Protocol(
                "authentication was not acknowledged".into(),
            ));
        }

        write_packet(&mut stream, request, DEFAULT_MAX_WIRE_BYTES)?;
        read_packet(&mut stream, DEFAULT_MAX_WIRE_BYTES)
    }

    #[cfg(test)]
    pub(super) fn endpoint_permissions(path: &Path) -> Result<(u32, u32), LocalHostReferenceError> {
        let parent = path.parent().ok_or_else(|| {
            LocalHostReferenceError::Protocol("socket path has no parent directory".into())
        })?;
        Ok((
            fs::metadata(parent)?.permissions().mode() & 0o777,
            fs::metadata(path)?.permissions().mode() & 0o777,
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{decode_wire_json, encode_wire_json, WireFrame};
        use std::thread;

        #[test]
        fn real_uds_peer_auth_and_wire_round_trip() {
            let dir = tempfile::tempdir().unwrap();
            let parent = dir.path().join("protected");
            let socket = parent.join("tauron.sock");
            let listener = bind_endpoint(&socket).unwrap();
            assert_eq!(endpoint_permissions(&socket).unwrap(), (0o700, 0o600));

            let secret = b"local-reference-secret".to_vec();
            let server_secret = secret.clone();
            let server = thread::spawn(move || {
                let mut broker = LocalHostBroker::new(server_secret);
                broker.acquire("reference-host", "linux-uds").unwrap();
                serve_one(&listener, &mut broker)
            });

            let request = encode_wire_json(
                &WireFrame::new("reference.ping/1", 1, serde_json::json!({"ping":"pong"})),
                DEFAULT_MAX_WIRE_BYTES,
            )
            .unwrap();
            let response = client_roundtrip(&socket, &secret, &request).unwrap();
            let frame: WireFrame<serde_json::Value> =
                decode_wire_json(&response, DEFAULT_MAX_WIRE_BYTES).unwrap();
            assert_eq!(frame.header.schema, "reference.ping/1");
            assert_eq!(frame.payload["ping"], "pong");
            server.join().unwrap().unwrap();
        }

        #[test]
        fn wrong_secret_is_rejected_without_a_wire_exchange() {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("tauron.sock");
            let listener = bind_endpoint(&socket).unwrap();
            let server = thread::spawn(move || {
                let mut broker = LocalHostBroker::new(b"correct-secret");
                broker.acquire("reference-host", "linux-uds").unwrap();
                serve_one(&listener, &mut broker)
            });

            let request = br#"{}"#;
            assert!(client_roundtrip(&socket, b"wrong-secret", request).is_err());
            assert!(matches!(
                server.join().unwrap(),
                Err(LocalHostReferenceError::Broker(LocalHostBrokerError::InvalidProof))
            ));
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{
    bind_endpoint, client_roundtrip, current_client_subject, peer_evidence, serve_one,
};

#[cfg(not(target_os = "linux"))]
pub fn current_client_subject() -> String {
    "unsupported-platform".to_string()
}
