//! V4 A106/A108 official Remote TLS reference consumer.
//!
//! Uses a generated self-signed certificate only for the reference E2E. Production callers must
//! provision their own certificate chain and trust roots.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::thread;

use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::RootCertStore;
use tauron_host::{
    client_roundtrip_tls, decode_wire_json, encode_wire_json, serve_one_tls, tls13_client_config,
    tls13_server_config, RemoteHostConfig, RemoteHostSecurity, RemoteTlsRequest, WireFrame,
    DEFAULT_MAX_WIRE_BYTES,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string()])?;
    let cert_der = cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));

    let server_config = tls13_server_config(vec![cert_der.clone()], key)?;
    let mut roots = RootCertStore::empty();
    roots.add(cert_der)?;
    let client_config = tls13_client_config(roots)?;

    let config = RemoteHostConfig {
        audience: "tauron-remote-reference".into(),
        allowed_origins: vec!["https://reference.example".into()],
        ..RemoteHostConfig::default()
    };
    let mut security = RemoteHostSecurity::new("remote-reference-host", config)?;
    let credential = security.issue_credential(
        "reference-user",
        "https://reference.example",
        "one-time+tls13",
        BTreeMap::from([("fixture".into(), "remote-tls".into())]),
        1,
    )?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let server = thread::spawn(move || serve_one_tls(&listener, server_config, &mut security, 2));

    let wire = encode_wire_json(
        &WireFrame::new(
            "remote.reference.ping/1",
            1,
            serde_json::json!({"transport":"real-rustls-tls13"}),
        ),
        DEFAULT_MAX_WIRE_BYTES,
    )?;
    let request = RemoteTlsRequest {
        credential_token: credential.token,
        audience: "tauron-remote-reference".into(),
        origin: "https://reference.example".into(),
        sequence: 1,
        nonce: "remote-reference-nonce-1".into(),
        wire,
    };
    let response = client_roundtrip_tls(
        addr,
        client_config,
        ServerName::try_from("localhost")?,
        &request,
    )?;
    let frame: WireFrame<serde_json::Value> =
        decode_wire_json(&response, DEFAULT_MAX_WIRE_BYTES)?;
    if frame.header.schema != "remote.reference.ping/1"
        || frame.payload["transport"] != "real-rustls-tls13"
    {
        return Err("Remote TLS Universal Wire response drift".into());
    }

    let observation = server.join().map_err(|_| "Remote TLS server thread panicked")??;
    if observation.tls_protocol != "TLS1.3" || !observation.transport_id.starts_with("rustls-tls13:")
    {
        return Err("Remote TLS transport evidence drift".into());
    }

    println!(
        "A106/A108 Remote TLS reference E2E OK: {} / {}",
        observation.tls_protocol, observation.transport_id
    );
    Ok(())
}
