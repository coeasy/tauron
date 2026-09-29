//! V4 A106 non-Tauri Local Host reference application.
//!
//! Linux/macOS execute owner-only UDS + kernel peer credentials. Windows executes an owner-only
//! Named Pipe + impersonation-token SID. All platforms share Broker HMAC + Universal Wire.

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;
    use std::thread;

    use tauron_host::local_host_reference::{bind_endpoint, client_roundtrip, serve_one};
    use tauron_host::{
        decode_wire_json, encode_wire_json, LocalHostBroker, WireFrame, DEFAULT_MAX_WIRE_BYTES,
    };

    let root =
        std::env::temp_dir().join(format!("tauron-local-host-reference-{}", std::process::id()));
    let socket = root.join("tauron.sock");
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    let listener = bind_endpoint(&socket)?;
    let secret = b"tauron-a106-reference-secret".to_vec();
    let server_secret = secret.clone();

    let server = thread::spawn(move || {
        let mut broker = LocalHostBroker::new(server_secret);
        broker.acquire("a106-reference-host", "unix-uds")?;
        serve_one(&listener, &mut broker)
    });

    let request = encode_wire_json(
        &WireFrame::new(
            "a106.reference.ping/1",
            1,
            serde_json::json!({"consumer":"non-tauri-local-host"}),
        ),
        DEFAULT_MAX_WIRE_BYTES,
    )?;
    let response = client_roundtrip(&socket, &secret, &request)?;
    let decoded: WireFrame<serde_json::Value> =
        decode_wire_json(&response, DEFAULT_MAX_WIRE_BYTES)?;
    if decoded.header.schema != "a106.reference.ping/1" {
        return Err("reference wire schema drift".into());
    }

    server.join().map_err(|_| "reference server thread panicked")??;
    fs::remove_file(&socket)?;
    fs::remove_dir(&root)?;
    println!(
        "A106 Local Host reference E2E OK: UDS + kernel peer credentials + peer proof + Universal Wire"
    );
    Ok(())
}


#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;
    use std::thread;

    use tauron_host::local_host_reference::{bind_endpoint, client_roundtrip, serve_one};
    use tauron_host::{
        decode_wire_json, encode_wire_json, LocalHostBroker, WireFrame, DEFAULT_MAX_WIRE_BYTES,
    };

    let pipe = PathBuf::from(format!(
        r"\\.\pipe\tauron-local-host-reference-{}",
        std::process::id()
    ));
    let listener = bind_endpoint(&pipe)?;
    let secret = b"tauron-a106-reference-secret".to_vec();
    let server_secret = secret.clone();

    let server = thread::spawn(move || {
        let mut broker = LocalHostBroker::new(server_secret);
        broker.acquire("a106-reference-host", "windows-named-pipe")?;
        serve_one(&listener, &mut broker)
    });

    let request = encode_wire_json(
        &WireFrame::new(
            "a106.reference.ping/1",
            1,
            serde_json::json!({"consumer":"non-tauri-local-host","transport":"named-pipe"}),
        ),
        DEFAULT_MAX_WIRE_BYTES,
    )?;
    let response = client_roundtrip(&pipe, &secret, &request)?;
    let decoded: WireFrame<serde_json::Value> =
        decode_wire_json(&response, DEFAULT_MAX_WIRE_BYTES)?;
    if decoded.header.schema != "a106.reference.ping/1" {
        return Err("reference wire schema drift".into());
    }

    server.join().map_err(|_| "reference server thread panicked")??;
    println!("A106 Local Host reference E2E OK: Named Pipe + SID + peer proof + Universal Wire");
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() {
    eprintln!(
        "A106 reference endpoint is not implemented on this platform;          it remains explicitly unsupported rather than falling back insecurely."
    );
}
