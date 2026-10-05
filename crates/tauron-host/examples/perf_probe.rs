//! Release-stable performance/size probe for V4 evidence.
//!
//! This is a bounded smoke baseline rather than a benchmark leaderboard. Release CI needs a
//! deterministic regression ceiling for the canonical Wire hot path. Peak-RSS evidence
//! moved to the real install stream (`tauron-adapter --example install_stream_probe`, 轮 44):
//! reading VmHWM here only measured the wire bench, not the memory-heavy path.

use std::hint::black_box;
use std::time::Instant;

use tauron_host::{decode_wire_json, encode_wire_json, WireFrame, DEFAULT_MAX_WIRE_BYTES};

fn main() {
    const ITERATIONS: u64 = 100_000;
    let frame = WireFrame::new(
        "performance.wire/1",
        42,
        serde_json::json!({
            "callId": "perf-call",
            "method": "probe",
            "payload": {"value": 42, "text": "tauron"}
        }),
    );
    let mut bytes = encode_wire_json(&frame, DEFAULT_MAX_WIRE_BYTES).expect("encode seed frame");

    let started = Instant::now();
    for _ in 0..ITERATIONS {
        let decoded: WireFrame<serde_json::Value> =
            decode_wire_json(&bytes, DEFAULT_MAX_WIRE_BYTES).expect("decode probe frame");
        bytes = encode_wire_json(&decoded, DEFAULT_MAX_WIRE_BYTES).expect("encode probe frame");
        black_box(&bytes);
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    println!(
        "{}",
        serde_json::json!({
            "schemaVersion": 1,
            "wireRoundTripIterations": ITERATIONS,
            "wireRoundTripElapsedMs": elapsed_ms,
            "finalFrameBytes": bytes.len()
        })
    );
}
