//! Non-Tauri Local Host reference transport helpers (V4 A106).
//!
//! The canonical broker/authentication rules live in `local_host`. This module supplies the
//! first real transport consumer: owner-only Unix Domain Sockets on Linux/macOS. Peer identity is
//! derived from the kernel (Linux `SO_PEERCRED`; macOS `getpeereid`) before the shared Broker
//! challenge/HMAC check. The wire payload is the same `WireFrame<json-v1>` used by every other
//! transport.
//!
//! Honest support boundary:
//! - Linux: UDS + SO_PEERCRED is implemented and executed in required CI.
//! - macOS: UDS + getpeereid is implemented and executed on arm64 + Intel required CI.
//! - Windows: owner-only Named Pipe DACL + kernel-derived impersonation token SID is implemented.
//! - Remote transport is out of scope; A108 remote chaos remains open until a real Remote Host
//!   transport exists.

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

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix {
    #[cfg(target_os = "linux")]
    use std::ffi::c_void;
    use std::fs;
    use std::io::{Read, Write};
    #[cfg(target_os = "linux")]
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

    /// Bind a protected Linux/macOS UDS endpoint.
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
    #[cfg(target_os = "linux")]
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

    /// macOS peer identity is derived with getpeereid(3), which asks the kernel for the effective
    /// UID/GID of the process at the other end of the connected Unix socket.
    #[cfg(target_os = "macos")]
    pub fn peer_evidence(
        stream: &UnixStream,
    ) -> Result<PeerCredentialEvidence, LocalHostReferenceError> {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: uid/gid point to writable storage and stream owns a live connected descriptor.
        let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: geteuid has no preconditions.
        let server_uid = unsafe { libc::geteuid() };
        Ok(PeerCredentialEvidence {
            platform_subject: format!("macos:uid={uid}:gid={gid}"),
            endpoint_owner_verified: uid == server_uid,
        })
    }

    /// Subject used by a Linux client when calculating its HMAC proof.
    ///
    /// This value is not trusted by the server; the server independently derives the exact subject
    /// from SO_PEERCRED and Broker::verify compares it with the challenge record.
    #[cfg(target_os = "linux")]
    pub fn current_client_subject() -> String {
        // SAFETY: getuid/getpid have no preconditions.
        let uid = unsafe { libc::getuid() };
        let pid = unsafe { libc::getpid() };
        format!("linux:uid={uid}:pid={pid}")
    }

    /// Subject used by the macOS reference client. The server derives the same effective
    /// UID/GID independently through getpeereid; this payload is not itself trusted.
    #[cfg(target_os = "macos")]
    pub fn current_client_subject() -> String {
        // SAFETY: geteuid/getegid have no preconditions.
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        format!("macos:uid={uid}:gid={gid}")
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
                broker.acquire("reference-host", "unix-uds").unwrap();
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
                broker.acquire("reference-host", "unix-uds").unwrap();
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

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE,
        HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, RevertToSelf, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
        OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient,
        WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
    };

    use crate::{
        peer_proof, LocalHostBroker, LocalHostBrokerError, PeerChallenge, PeerCredentialEvidence,
        DEFAULT_MAX_WIRE_BYTES,
    };

    use super::{reference_wire_roundtrip, LocalHostReferenceError, REFERENCE_CONTROL_MAX_BYTES};

    const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
    const PIPE_CONNECT_TIMEOUT_MS: u32 = 5_000;
    // Windows named-pipe impersonation is defined against the security context of the last
    // message read by the server. This fixed pre-auth marker exists only to establish that
    // kernel context; it carries no identity and is bounded before any allocation-heavy decode.
    const WINDOWS_CLIENT_HELLO: &[u8] = b"TAURON_LOCAL_HELLO_V1";
    const WINDOWS_CLIENT_DONE: &[u8] = b"TAURON_LOCAL_DONE_V1";
    const WINDOWS_CLIENT_HELLO_MAX_BYTES: usize = 64;

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide_str(value: &str) -> Vec<u16> {
        wide(std::ffi::OsStr::new(value))
    }

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn new(handle: HANDLE) -> Result<Self, LocalHostReferenceError> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                Err(std::io::Error::last_os_error().into())
            } else {
                Ok(Self(handle))
            }
        }

        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: this wrapper uniquely owns one valid Win32 handle.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct LocalAllocation(*mut c_void);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: SDDL/SID conversion APIs allocate these buffers for LocalFree.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    struct RevertImpersonation;

    impl Drop for RevertImpersonation {
        fn drop(&mut self) {
            // SAFETY: RevertToSelf has no pointer preconditions and is idempotent for our use.
            unsafe {
                RevertToSelf();
            }
        }
    }

    fn token_sid_string(token: HANDLE) -> Result<String, LocalHostReferenceError> {
        let mut needed = 0u32;
        // SAFETY: first call deliberately supplies no output buffer to obtain required length.
        unsafe {
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: buffer has the size reported by GetTokenInformation and lives through parsing.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast::<c_void>(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        // SAFETY: successful TokenUser output begins with a TOKEN_USER whose SID points into
        // the same live buffer.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut text = null_mut();
        // SAFETY: User.Sid is valid while buffer is alive; Windows allocates text for LocalFree.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let _text_guard = LocalAllocation(text.cast::<c_void>());
        let mut len = 0usize;
        // SAFETY: ConvertSidToStringSidW returned a NUL-terminated UTF-16 string.
        unsafe {
            while *text.add(len) != 0 {
                len += 1;
            }
        }
        // SAFETY: text points to len initialized UTF-16 code units.
        let slice = unsafe { std::slice::from_raw_parts(text, len) };
        Ok(String::from_utf16_lossy(slice))
    }

    fn current_process_sid() -> Result<String, LocalHostReferenceError> {
        let mut token = null_mut();
        // SAFETY: pseudo process handle is valid and token receives an owned kernel handle.
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        token_sid_string(OwnedHandle::new(token)?.raw())
    }

    fn peer_sid(pipe: HANDLE) -> Result<String, LocalHostReferenceError> {
        // SAFETY: pipe is a connected server-side named-pipe handle.
        if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let _revert = RevertImpersonation;
        let mut token = null_mut();
        // OpenAsSelf=TRUE keeps the token being opened as the client's impersonation token,
        // while performing the access check with the server process security context. Using
        // FALSE can make the impersonated client unable to open its own executive token object.
        // SAFETY: current thread is impersonating the connected client.
        let ok = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        token_sid_string(OwnedHandle::new(token)?.raw())
    }

    fn security_descriptor_for_current_user(
    ) -> Result<(SECURITY_ATTRIBUTES, LocalAllocation), LocalHostReferenceError> {
        let sid = current_process_sid()?;
        // Protected DACL with one ACE: Generic-All for this exact user SID only.
        let sddl = wide_str(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: sddl is a valid NUL-terminated SDDL string; descriptor is an out pointer.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let allocation = LocalAllocation(descriptor.cast::<c_void>());
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        Ok((attrs, allocation))
    }

    fn read_exact(handle: HANDLE, mut out: &mut [u8]) -> Result<(), LocalHostReferenceError> {
        while !out.is_empty() {
            let mut read = 0u32;
            let chunk = out.len().min(u32::MAX as usize) as u32;
            // SAFETY: out points to chunk writable bytes; synchronous I/O uses null OVERLAPPED.
            let ok = unsafe { ReadFile(handle, out.as_mut_ptr(), chunk, &mut read, null_mut()) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if read == 0 {
                return Err(LocalHostReferenceError::Protocol(
                    "named pipe closed before packet completed".into(),
                ));
            }
            out = &mut out[read as usize..];
        }
        Ok(())
    }

    fn write_all(handle: HANDLE, mut bytes: &[u8]) -> Result<(), LocalHostReferenceError> {
        while !bytes.is_empty() {
            let mut written = 0u32;
            let chunk = bytes.len().min(u32::MAX as usize) as u32;
            // SAFETY: bytes points to chunk readable bytes; synchronous I/O uses null OVERLAPPED.
            let ok = unsafe { WriteFile(handle, bytes.as_ptr(), chunk, &mut written, null_mut()) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if written == 0 {
                return Err(LocalHostReferenceError::Protocol(
                    "named pipe accepted zero bytes".into(),
                ));
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    fn write_packet(
        handle: HANDLE,
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
        write_all(handle, &(bytes.len() as u32).to_be_bytes())?;
        write_all(handle, bytes)
    }

    fn read_packet(handle: HANDLE, max_bytes: usize) -> Result<Vec<u8>, LocalHostReferenceError> {
        let mut len = [0u8; 4];
        read_exact(handle, &mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if len > max_bytes {
            return Err(LocalHostReferenceError::Protocol(format!(
                "packet length {len} exceeds limit {max_bytes}"
            )));
        }
        let mut bytes = vec![0u8; len];
        read_exact(handle, &mut bytes)?;
        Ok(bytes)
    }

    pub struct WindowsPipeListener {
        handle: OwnedHandle,
    }

    // HANDLE ownership may move to the one server thread; access remains single-threaded.
    unsafe impl Send for WindowsPipeListener {}

    /// Create one first-instance named pipe with a protected DACL for the current user.
    pub fn bind_endpoint(path: &Path) -> Result<WindowsPipeListener, LocalHostReferenceError> {
        let name = wide(path.as_os_str());
        let (mut attrs, _descriptor) = security_descriptor_for_current_user()?;
        // SAFETY: name/SECURITY_ATTRIBUTES remain alive for this synchronous create call.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                PIPE_CONNECT_TIMEOUT_MS,
                &mut attrs,
            )
        };
        Ok(WindowsPipeListener { handle: OwnedHandle::new(handle)? })
    }

    pub fn current_client_subject() -> String {
        current_process_sid()
            .map(|sid| format!("windows:sid={sid}"))
            .unwrap_or_else(|_| "windows:sid=unavailable".to_string())
    }

    fn peer_evidence(handle: HANDLE) -> Result<PeerCredentialEvidence, LocalHostReferenceError> {
        let peer = peer_sid(handle)?;
        let owner = current_process_sid()?;
        Ok(PeerCredentialEvidence {
            platform_subject: format!("windows:sid={peer}"),
            endpoint_owner_verified: peer.eq_ignore_ascii_case(&owner),
        })
    }

    pub fn serve_one(
        listener: &WindowsPipeListener,
        broker: &mut LocalHostBroker,
    ) -> Result<(), LocalHostReferenceError> {
        // SAFETY: listener owns a valid named-pipe server handle.
        let connected = unsafe { ConnectNamedPipe(listener.handle.raw(), null_mut()) };
        if connected == 0 {
            // A client can connect between CreateNamedPipe and ConnectNamedPipe.
            if unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
                return Err(std::io::Error::last_os_error().into());
            }
        }

        let handle = listener.handle.raw();
        let result = (|| {
            // ImpersonateNamedPipeClient uses the security context of the last message read.
            // Read one fixed, tiny marker first; never trust this payload as identity.
            let hello = read_packet(handle, WINDOWS_CLIENT_HELLO_MAX_BYTES)?;
            if hello != WINDOWS_CLIENT_HELLO {
                return Err(LocalHostReferenceError::Protocol(
                    "invalid Windows Local Host pre-auth marker".into(),
                ));
            }

            let evidence = peer_evidence(handle)?;
            let challenge = broker.challenge(&evidence)?;
            let challenge_json = serde_json::to_vec(&challenge)
                .map_err(|e| LocalHostReferenceError::Protocol(e.to_string()))?;
            write_packet(handle, &challenge_json, REFERENCE_CONTROL_MAX_BYTES)?;

            let proof = read_packet(handle, REFERENCE_CONTROL_MAX_BYTES)?;
            let peer = broker.verify(&evidence, &challenge.challenge_id, &proof)?;
            let active_generation =
                broker.active_lease().ok_or(LocalHostBrokerError::NoOwner)?.generation;
            if peer.owner_generation != active_generation {
                return Err(LocalHostReferenceError::Protocol(
                    "authenticated peer generation became stale".into(),
                ));
            }
            write_packet(handle, b"ok", REFERENCE_CONTROL_MAX_BYTES)?;

            let request = read_packet(handle, DEFAULT_MAX_WIRE_BYTES)?;
            let response = reference_wire_roundtrip(&request)?;
            write_packet(handle, &response, DEFAULT_MAX_WIRE_BYTES)?;

            // Do not disconnect immediately after WriteFile. Named-pipe disconnect can race a
            // client that has not consumed the response yet. Require one fixed completion ACK so
            // the server only tears down after the client proved the full response was read.
            let done = read_packet(handle, WINDOWS_CLIENT_HELLO_MAX_BYTES)?;
            if done != WINDOWS_CLIENT_DONE {
                return Err(LocalHostReferenceError::Protocol(
                    "invalid Windows Local Host completion marker".into(),
                ));
            }
            Ok(())
        })();

        // SAFETY: handle is the connected server pipe; disconnection does not close the handle.
        unsafe {
            DisconnectNamedPipe(handle);
        }
        result
    }

    pub fn client_roundtrip(
        path: &Path,
        bootstrap_secret: &[u8],
        request: &[u8],
    ) -> Result<Vec<u8>, LocalHostReferenceError> {
        let name = wide(path.as_os_str());
        // SAFETY: name is a valid NUL-terminated named-pipe path.
        if unsafe { WaitNamedPipeW(name.as_ptr(), PIPE_CONNECT_TIMEOUT_MS) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: arguments describe synchronous read/write access to the existing named pipe.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        let handle = OwnedHandle::new(handle)?;

        // This marker is deliberately unauthenticated and contains no caller-supplied identity.
        // The server reads it only so Windows can bind subsequent impersonation to this client.
        write_packet(handle.raw(), WINDOWS_CLIENT_HELLO, WINDOWS_CLIENT_HELLO_MAX_BYTES)?;

        let challenge_json = read_packet(handle.raw(), REFERENCE_CONTROL_MAX_BYTES)?;
        let challenge: PeerChallenge = serde_json::from_slice(&challenge_json)
            .map_err(|e| LocalHostReferenceError::Protocol(e.to_string()))?;
        // Authentication must never fall back to a synthetic subject if SID lookup fails.
        let subject = format!("windows:sid={}", current_process_sid()?);
        let proof =
            peer_proof(bootstrap_secret, &subject, &challenge.nonce, challenge.owner_generation);
        write_packet(handle.raw(), &proof, REFERENCE_CONTROL_MAX_BYTES)?;
        let ack = read_packet(handle.raw(), REFERENCE_CONTROL_MAX_BYTES)?;
        if ack != b"ok" {
            return Err(LocalHostReferenceError::Protocol(
                "authentication was not acknowledged".into(),
            ));
        }

        write_packet(handle.raw(), request, DEFAULT_MAX_WIRE_BYTES)?;
        let response = read_packet(handle.raw(), DEFAULT_MAX_WIRE_BYTES)?;
        write_packet(
            handle.raw(),
            WINDOWS_CLIENT_DONE,
            WINDOWS_CLIENT_HELLO_MAX_BYTES,
        )?;
        Ok(response)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use unix::{bind_endpoint, client_roundtrip, current_client_subject, peer_evidence, serve_one};

#[cfg(target_os = "windows")]
pub use windows::{
    bind_endpoint, client_roundtrip, current_client_subject, serve_one, WindowsPipeListener,
};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn current_client_subject() -> String {
    "unsupported-platform".to_string()
}
