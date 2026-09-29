//! Official Remote Host TLS reference transport (V4 A106/A108).
//!
//! Security authority stays in `remote_host`; this module proves a concrete network adapter that
//! creates `RemoteTransportEvidence` only after a real rustls TLS 1.3 handshake completes.
//! Client-provided JSON can never mint trusted TLS evidence.
//!
//! The reference transport deliberately handles one authenticated request per TLS connection.
//! Long-lived multiplexing belongs to a higher transport layer; session/replay/rate/inflight
//! authority remains in `RemoteHostSecurity`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::ClientConnection;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::ServerConnection;
use rustls::{
    ClientConfig, ProtocolVersion, RootCertStore, ServerConfig, StreamOwned,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{RemoteHostError, RemoteHostSecurity, RemoteTransportEvidence, DEFAULT_MAX_WIRE_BYTES};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_OVERHEAD_BYTES: usize = 256 * 1024;

/// Hard ceiling for one JSON request envelope. The Universal Wire body keeps its own independent
/// `DEFAULT_MAX_WIRE_BYTES` validation inside `RemoteHostSecurity::handle_wire`.
pub const REMOTE_TLS_MAX_MESSAGE_BYTES: usize = DEFAULT_MAX_WIRE_BYTES + CONTROL_OVERHEAD_BYTES;

#[derive(Debug, Error)]
pub enum RemoteTlsReferenceError {
    #[error("Remote TLS I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Remote TLS setup/handshake failed: {0}")]
    Tls(#[from] rustls::Error),
    #[error("Remote TLS JSON protocol failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Remote Host security rejected request: {0}")]
    Remote(#[from] RemoteHostError),
    #[error("Remote TLS protocol rejected request: {0}")]
    Protocol(String),
}

/// One authenticated request carried over the reference TLS connection.
///
/// Identity is intentionally absent. The server-side Principal is minted only from the
/// one-time credential record owned by `RemoteHostSecurity`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTlsRequest {
    pub credential_token: String,
    pub audience: String,
    pub origin: String,
    pub sequence: u64,
    pub nonce: String,
    pub wire: Vec<u8>,
}

/// Evidence emitted by the reference server after a successful TLS+RemoteHost exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTlsServerObservation {
    pub peer_addr: String,
    pub transport_id: String,
    pub session_id: String,
    pub tls_protocol: String,
    pub server_name: Option<String>,
}

fn ring_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Build a TLS-1.3-only server configuration.
///
/// The caller owns certificate/key provisioning. The reference transport never generates or
/// silently trusts production certificates.
pub fn tls13_server_config(
    cert_chain: Vec<CertificateDer<'static>>,
    private_key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, RemoteTlsReferenceError> {
    let config = ServerConfig::builder_with_provider(ring_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(cert_chain, private_key)?;
    Ok(Arc::new(config))
}

/// Build a TLS-1.3-only client configuration with explicit trust roots.
///
/// rustls performs certificate chain and server-name verification; there is no insecure
/// "accept any certificate" path in this reference adapter.
pub fn tls13_client_config(
    roots: RootCertStore,
) -> Result<Arc<ClientConfig>, RemoteTlsReferenceError> {
    if roots.is_empty() {
        return Err(RemoteTlsReferenceError::Protocol(
            "client root store must not be empty".into(),
        ));
    }
    let config = ClientConfig::builder_with_provider(ring_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn configure_tcp(stream: &TcpStream) -> Result<(), RemoteTlsReferenceError> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    Ok(())
}

fn write_packet<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<(), RemoteTlsReferenceError> {
    if bytes.len() > max_bytes || bytes.len() > u32::MAX as usize {
        return Err(RemoteTlsReferenceError::Protocol(format!(
            "Remote TLS packet too large: {} > {}",
            bytes.len(),
            max_bytes
        )));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}

fn read_packet<R: Read>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Vec<u8>, RemoteTlsReferenceError> {
    let mut len = [0u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max_bytes {
        return Err(RemoteTlsReferenceError::Protocol(format!(
            "Remote TLS packet length {len} exceeds limit {max_bytes}"
        )));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn require_tls13(version: Option<ProtocolVersion>) -> Result<(), RemoteTlsReferenceError> {
    if version == Some(ProtocolVersion::TLSv1_3) {
        Ok(())
    } else {
        Err(RemoteTlsReferenceError::Protocol(format!(
            "Remote Host requires negotiated TLS1.3; got {version:?}"
        )))
    }
}

/// Serve one real TCP/TLS request and bind it to the canonical Remote Host authority.
///
/// Trusted transport evidence is minted only after `ServerConnection::complete_io` has completed
/// a TLS1.3 handshake. Request JSON has no field that can override this evidence.
pub fn serve_one_tls(
    listener: &TcpListener,
    server_config: Arc<ServerConfig>,
    security: &mut RemoteHostSecurity,
    now_ms: u64,
) -> Result<RemoteTlsServerObservation, RemoteTlsReferenceError> {
    let (mut tcp, peer_addr) = listener.accept()?;
    configure_tcp(&tcp)?;

    let mut connection = ServerConnection::new(server_config)?;
    connection.complete_io(&mut tcp)?;
    require_tls13(connection.protocol_version())?;

    let server_name = connection.server_name().map(str::to_string);
    let transport_id = format!("rustls-tls13:{peer_addr}:{}", Uuid::new_v4());
    let mut tls = StreamOwned::new(connection, tcp);

    let request_json = read_packet(&mut tls, REMOTE_TLS_MAX_MESSAGE_BYTES)?;
    let request: RemoteTlsRequest = serde_json::from_slice(&request_json)?;

    let session = security.open_session(
        &request.credential_token,
        &request.audience,
        &request.origin,
        RemoteTransportEvidence::tls13(transport_id.clone()),
        now_ms,
    )?;

    let response = security.handle_wire(
        &session.session_id,
        request.sequence,
        &request.nonce,
        now_ms,
        &request.wire,
    );
    let close = security.close(&session.session_id);

    let response = match (response, close) {
        (Ok(bytes), Ok(())) => bytes,
        (Err(error), _) => return Err(error.into()),
        (Ok(_), Err(error)) => return Err(error.into()),
    };
    write_packet(&mut tls, &response, DEFAULT_MAX_WIRE_BYTES)?;

    Ok(RemoteTlsServerObservation {
        peer_addr: peer_addr.to_string(),
        transport_id,
        session_id: session.session_id,
        tls_protocol: "TLS1.3".into(),
        server_name,
    })
}

/// Reference client that proves certificate/server-name verification before sending credentials.
pub fn client_roundtrip_tls(
    addr: SocketAddr,
    client_config: Arc<ClientConfig>,
    server_name: ServerName<'static>,
    request: &RemoteTlsRequest,
) -> Result<Vec<u8>, RemoteTlsReferenceError> {
    let mut tcp = TcpStream::connect(addr)?;
    configure_tcp(&tcp)?;

    let mut connection = ClientConnection::new(client_config, server_name)?;
    connection.complete_io(&mut tcp)?;
    require_tls13(connection.protocol_version())?;
    if connection.peer_certificates().is_none_or(|certs| certs.is_empty()) {
        return Err(RemoteTlsReferenceError::Protocol(
            "TLS completed without an authenticated server certificate".into(),
        ));
    }

    let mut tls = StreamOwned::new(connection, tcp);
    let encoded = serde_json::to_vec(request)?;
    write_packet(&mut tls, &encoded, REMOTE_TLS_MAX_MESSAGE_BYTES)?;
    read_packet(&mut tls, DEFAULT_MAX_WIRE_BYTES)
}
