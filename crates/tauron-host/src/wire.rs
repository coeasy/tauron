//! V4 universal wire framing and schema evolution contract (A67/A68).
//!
//! The wire layer is host-neutral: Tauri, local IPC, FFI and remote transports must all
//! carry the same versioned envelope. JSON v1 is mandatory; transports may add framing,
//! but may not silently reinterpret payloads.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// First stable Tauron wire protocol version.
pub const WIRE_VERSION_V1: u16 = 1;
/// Mandatory codec for V4. Binary codecs require a future negotiated wire version.
pub const JSON_V1_CODEC: &str = "json-v1";
/// Hard frame ceiling before any payload deserialization.
pub const DEFAULT_MAX_WIRE_BYTES: usize = 8 * 1024 * 1024;

/// Versioned wire metadata shared by every transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireHeader {
    pub wire_version: u16,
    pub codec: String,
    /// Stable schema identifier, e.g. `host.call.request/1`.
    pub schema: String,
    /// Generation protects consumers from stale handles after activation/upgrade.
    pub generation: u64,
}

impl WireHeader {
    pub fn json_v1(schema: impl Into<String>, generation: u64) -> Self {
        Self {
            wire_version: WIRE_VERSION_V1,
            codec: JSON_V1_CODEC.to_string(),
            schema: schema.into(),
            generation,
        }
    }

    pub fn validate(&self) -> Result<(), WireError> {
        if self.wire_version != WIRE_VERSION_V1 {
            return Err(WireError::UnsupportedVersion(self.wire_version));
        }
        if self.codec != JSON_V1_CODEC {
            return Err(WireError::UnsupportedCodec(self.codec.clone()));
        }
        if self.schema.trim().is_empty() {
            return Err(WireError::MissingSchema);
        }
        Ok(())
    }
}

/// Explicit extension bag. Unknown top-level fields are rejected; forward-compatible data
/// must live here so old readers can preserve it without accidentally treating it as trusted.
pub type WireExtensions = BTreeMap<String, serde_json::Value>;

/// Canonical V4 envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireFrame<T> {
    pub header: WireHeader,
    pub payload: T,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: WireExtensions,
}

impl<T> WireFrame<T> {
    pub fn new(schema: impl Into<String>, generation: u64, payload: T) -> Self {
        Self {
            header: WireHeader::json_v1(schema, generation),
            payload,
            extensions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WireError {
    #[error("wire frame exceeds limit: {actual} > {limit}")]
    FrameTooLarge { actual: usize, limit: usize },
    #[error("unsupported wire version {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported wire codec {0}")]
    UnsupportedCodec(String),
    #[error("wire schema identifier is empty")]
    MissingSchema,
    #[error("wire JSON is invalid: {0}")]
    InvalidJson(String),
}

pub fn encode_json<T: Serialize>(
    frame: &WireFrame<T>,
    max_bytes: usize,
) -> Result<Vec<u8>, WireError> {
    frame.header.validate()?;
    let bytes = serde_json::to_vec(frame).map_err(|e| WireError::InvalidJson(e.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(WireError::FrameTooLarge { actual: bytes.len(), limit: max_bytes });
    }
    Ok(bytes)
}

pub fn decode_json<T: DeserializeOwned>(
    bytes: &[u8],
    max_bytes: usize,
) -> Result<WireFrame<T>, WireError> {
    if bytes.len() > max_bytes {
        return Err(WireError::FrameTooLarge { actual: bytes.len(), limit: max_bytes });
    }
    let frame: WireFrame<T> =
        serde_json::from_slice(bytes).map_err(|e| WireError::InvalidJson(e.to_string()))?;
    frame.header.validate()?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Payload {
        call_id: String,
    }

    #[test]
    fn json_v1_round_trip_is_transport_neutral() {
        let mut frame = WireFrame::new("host.call.request/1", 7, Payload { call_id: "c-1".into() });
        frame.extensions.insert("traceparent".into(), serde_json::json!("00-ab"));
        let bytes = encode_json(&frame, DEFAULT_MAX_WIRE_BYTES).unwrap();
        let decoded: WireFrame<Payload> = decode_json(&bytes, DEFAULT_MAX_WIRE_BYTES).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(decoded.header.codec, JSON_V1_CODEC);
    }

    #[test]
    fn unsupported_version_and_codec_fail_closed() {
        let mut frame = WireFrame::new("x/1", 0, serde_json::json!({}));
        frame.header.wire_version = 99;
        assert!(matches!(
            encode_json(&frame, DEFAULT_MAX_WIRE_BYTES),
            Err(WireError::UnsupportedVersion(99))
        ));

        frame.header.wire_version = WIRE_VERSION_V1;
        frame.header.codec = "msgpack".into();
        assert!(matches!(
            encode_json(&frame, DEFAULT_MAX_WIRE_BYTES),
            Err(WireError::UnsupportedCodec(_))
        ));
    }

    #[test]
    fn unknown_top_level_field_is_rejected_but_extensions_are_allowed() {
        let bad = br#"{"header":{"wireVersion":1,"codec":"json-v1","schema":"x/1","generation":0},"payload":{"callId":"c"},"mystery":1}"#;
        assert!(decode_json::<Payload>(bad, DEFAULT_MAX_WIRE_BYTES).is_err());

        let good = br#"{"header":{"wireVersion":1,"codec":"json-v1","schema":"x/1","generation":0},"payload":{"callId":"c"},"extensions":{"mystery":1}}"#;
        assert!(decode_json::<Payload>(good, DEFAULT_MAX_WIRE_BYTES).is_ok());
    }

    #[test]
    fn size_limit_is_checked_before_deserialization() {
        let bytes = vec![b'x'; 17];
        assert_eq!(
            decode_json::<serde_json::Value>(&bytes, 16),
            Err(WireError::FrameTooLarge { actual: 17, limit: 16 })
        );
    }
}
