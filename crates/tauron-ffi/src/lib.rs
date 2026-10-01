//! Stable C ABI ownership boundary for Tauron Universal Wire consumers (V4 A72/A107).
//!
//! ABI v1 is deliberately small: it proves opaque-handle ownership, retain/release,
//! Rust-owned buffer/error lifetime, shutdown invalidation and panic containment while
//! exercising the canonical `tauron-host` wire codec. It does not pretend that provider or
//! runtime command surfaces are already bound through FFI.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauron_host::{decode_wire_json, encode_wire_json, WireFrame, DEFAULT_MAX_WIRE_BYTES};

/// Stable ABI version for the C ownership contract.
pub const TAURON_FFI_ABI_VERSION: u32 = 1;

/// Stable status values used by every ABI v1 result.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TauronStatus {
    Ok = 0,
    InvalidArgument = 1,
    Shutdown = 2,
    WireError = 3,
    Panic = 4,
}

impl TauronStatus {
    const fn code(self) -> i32 {
        self as i32
    }
}

/// Opaque C handle. The concrete Rust allocation is never exposed in the header.
#[repr(C)]
pub struct TauronHost {
    _private: [u8; 0],
}

struct HostInner {
    shutdown: AtomicBool,
}

struct BufferOwner {
    bytes: Vec<u8>,
}

/// Rust-owned immutable bytes.
///
/// The caller may read `data[0..len]` but must not mutate or free `data` directly.
/// Release with `tauron_buffer_free` or `tauron_result_free`.
#[repr(C)]
#[derive(Debug)]
pub struct TauronBuffer {
    pub data: *const u8,
    pub len: usize,
    /// Opaque Rust allocation token. Never dereference or alter it outside Tauron.
    pub owner: *mut c_void,
}

impl TauronBuffer {
    const fn empty() -> Self {
        Self { data: ptr::null(), len: 0, owner: ptr::null_mut() }
    }
}

/// Structured ABI error. `message` is Rust-owned and follows TauronBuffer ownership rules.
#[repr(C)]
#[derive(Debug)]
pub struct TauronError {
    pub code: i32,
    pub message: TauronBuffer,
}

impl TauronError {
    const fn empty() -> Self {
        Self { code: TauronStatus::Ok as i32, message: TauronBuffer::empty() }
    }
}

/// Common FFI result shape.
///
/// `status == TAURON_STATUS_OK` means `buffer` contains the operation result and `error` is
/// empty. Any non-zero status means `error` is populated and `buffer` is empty.
#[repr(C)]
#[derive(Debug)]
pub struct TauronResult {
    pub status: i32,
    pub buffer: TauronBuffer,
    pub error: TauronError,
}

type FfiFailure = (TauronStatus, String);

fn owned_buffer(bytes: Vec<u8>) -> TauronBuffer {
    if bytes.is_empty() {
        return TauronBuffer::empty();
    }
    let owner = Box::new(BufferOwner { bytes });
    let data = owner.bytes.as_ptr();
    let len = owner.bytes.len();
    TauronBuffer { data, len, owner: Box::into_raw(owner).cast::<c_void>() }
}

unsafe fn drop_owned_buffer(buffer: &mut TauronBuffer) {
    if !buffer.owner.is_null() {
        // SAFETY: owner is minted only by owned_buffer and cleared immediately after this drop.
        unsafe {
            drop(Box::from_raw(buffer.owner.cast::<BufferOwner>()));
        }
    }
    *buffer = TauronBuffer::empty();
}

fn failure(status: TauronStatus, message: impl Into<String>) -> TauronResult {
    TauronResult {
        status: status.code(),
        buffer: TauronBuffer::empty(),
        error: TauronError {
            code: status.code(),
            message: owned_buffer(message.into().into_bytes()),
        },
    }
}

fn success(bytes: Vec<u8>) -> TauronResult {
    TauronResult {
        status: TauronStatus::Ok.code(),
        buffer: owned_buffer(bytes),
        error: TauronError::empty(),
    }
}

fn guarded_result<F>(f: F) -> TauronResult
where
    F: FnOnce() -> Result<Vec<u8>, FfiFailure>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(bytes)) => success(bytes),
        Ok(Err((status, message))) => failure(status, message),
        Err(_) => failure(TauronStatus::Panic, "Rust panic caught at Tauron FFI boundary"),
    }
}

fn host_ptr(handle: *const TauronHost) -> Result<*const HostInner, FfiFailure> {
    if handle.is_null() {
        Err((TauronStatus::InvalidArgument, "host handle is null".into()))
    } else {
        Ok(handle.cast::<HostInner>())
    }
}

/// Return the stable C ABI version.
#[no_mangle]
pub extern "C" fn tauron_ffi_abi_version() -> u32 {
    TAURON_FFI_ABI_VERSION
}

/// Allocate one opaque FFI host handle.
///
/// Returns null only if allocation/setup panics. No Rust panic crosses the ABI.
#[no_mangle]
pub extern "C" fn tauron_host_new() -> *mut TauronHost {
    catch_unwind(AssertUnwindSafe(|| {
        Arc::into_raw(Arc::new(HostInner { shutdown: AtomicBool::new(false) })) as *mut TauronHost
    }))
    .unwrap_or(ptr::null_mut())
}

/// Retain one additional reference to the same opaque host.
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by `tauron_host_new`/
/// `tauron_host_retain` whose final release has not occurred.
#[no_mangle]
pub unsafe extern "C" fn tauron_host_retain(handle: *mut TauronHost) -> *mut TauronHost {
    if handle.is_null() {
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: guaranteed by the function contract above.
        unsafe {
            Arc::increment_strong_count(handle.cast::<HostInner>() as *const HostInner);
        }
        handle
    }))
    .unwrap_or(ptr::null_mut())
}

/// Release exactly one reference previously created by new/retain.
///
/// # Safety
///
/// `handle` must be null or a live retained handle. Each retained reference may be released
/// exactly once. After the final release the pointer is invalid and must not be reused.
#[no_mangle]
pub unsafe extern "C" fn tauron_host_release(handle: *mut TauronHost) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: guaranteed by the function contract above.
        unsafe {
            Arc::decrement_strong_count(handle.cast::<HostInner>() as *const HostInner);
        }
    }));
}

/// Mark the host closed. This operation is idempotent.
///
/// Operational calls after shutdown return `TAURON_STATUS_SHUTDOWN`; retain/release remain valid.
///
/// # Safety
///
/// `handle` must be null or a live retained handle.
#[no_mangle]
pub unsafe extern "C" fn tauron_host_shutdown(handle: *mut TauronHost) -> i32 {
    let Ok(inner) = host_ptr(handle) else {
        return TauronStatus::InvalidArgument.code();
    };
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: host_ptr validated non-null; caller guarantees the retained lifetime.
        unsafe { &*inner }.shutdown.store(true, Ordering::Release);
    })) {
        Ok(()) => TauronStatus::Ok.code(),
        Err(_) => TauronStatus::Panic.code(),
    }
}

/// Validate and re-encode one Universal Wire JSON frame through the canonical Rust codec.
///
/// The returned buffer is Rust-owned and must be freed with `tauron_result_free` (or
/// `tauron_buffer_free` + `tauron_error_free`).
///
/// # Safety
///
/// `handle` must be a live retained handle. If `input_len > 0`, `input` must point to at
/// least `input_len` readable bytes for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn tauron_wire_roundtrip_json(
    handle: *mut TauronHost,
    input: *const u8,
    input_len: usize,
) -> TauronResult {
    guarded_result(|| {
        let inner = host_ptr(handle)?;
        // SAFETY: caller owns a live retained handle for this call.
        if unsafe { &*inner }.shutdown.load(Ordering::Acquire) {
            return Err((TauronStatus::Shutdown, "host is shut down".into()));
        }
        if input.is_null() && input_len != 0 {
            return Err((
                TauronStatus::InvalidArgument,
                "input pointer is null while input_len is non-zero".into(),
            ));
        }
        let bytes: &[u8] = if input_len == 0 {
            &[]
        } else {
            // SAFETY: guaranteed by the function contract above.
            unsafe { slice::from_raw_parts(input, input_len) }
        };
        let frame: WireFrame<serde_json::Value> =
            decode_wire_json(bytes, DEFAULT_MAX_WIRE_BYTES)
                .map_err(|e| (TauronStatus::WireError, e.to_string()))?;
        encode_wire_json(&frame, DEFAULT_MAX_WIRE_BYTES)
            .map_err(|e| (TauronStatus::WireError, e.to_string()))
    })
}

/// Free a standalone TauronBuffer and clear the caller-owned struct.
///
/// Calling this repeatedly on the same struct pointer is safe because the first call clears it.
///
/// # Safety
///
/// `buffer` must be null or point to a TauronBuffer produced by this ABI and not copied into a
/// second independently freed struct.
#[no_mangle]
pub unsafe extern "C" fn tauron_buffer_free(buffer: *mut TauronBuffer) {
    if buffer.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: guaranteed by the function contract.
        unsafe { drop_owned_buffer(&mut *buffer) };
    }));
}

/// Free an error message and clear the caller-owned error struct.
///
/// # Safety
///
/// `error` must be null or point to a TauronError produced by this ABI and not independently
/// double-freed through a copied struct.
#[no_mangle]
pub unsafe extern "C" fn tauron_error_free(error: *mut TauronError) {
    if error.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: guaranteed by the function contract.
        let error = unsafe { &mut *error };
        // SAFETY: message ownership is part of the same valid TauronError.
        unsafe { drop_owned_buffer(&mut error.message) };
        error.code = TauronStatus::Ok.code();
    }));
}

/// Free every Rust-owned allocation in a TauronResult and clear the struct.
///
/// # Safety
///
/// `result` must be null or point to a TauronResult returned by this ABI. Do not separately free
/// copied buffer/error fields and then pass an unmodified copy of the original result here.
#[no_mangle]
pub unsafe extern "C" fn tauron_result_free(result: *mut TauronResult) {
    if result.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: guaranteed by the function contract.
        let result = unsafe { &mut *result };
        // SAFETY: both allocations, when present, were minted by this result.
        unsafe {
            drop_owned_buffer(&mut result.buffer);
            drop_owned_buffer(&mut result.error.message);
        }
        result.error.code = TauronStatus::Ok.code();
        result.status = TauronStatus::Ok.code();
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: &[u8] = br#"{"header":{"wireVersion":1,"codec":"json-v1","schema":"ffi.test/1","generation":3},"payload":{"value":"ok"}}"#;

    unsafe fn bytes(buffer: &TauronBuffer) -> &[u8] {
        if buffer.len == 0 {
            &[]
        } else {
            // SAFETY: tests only call this before freeing a Tauron-owned buffer.
            unsafe { slice::from_raw_parts(buffer.data, buffer.len) }
        }
    }

    #[test]
    fn abi_version_and_opaque_handle_lifecycle_are_stable() {
        assert_eq!(tauron_ffi_abi_version(), 1);
        let host = tauron_host_new();
        assert!(!host.is_null());
        // SAFETY: host is live and each retain is paired with one release.
        let retained = unsafe { tauron_host_retain(host) };
        assert_eq!(retained, host);

        // SAFETY: FRAME is readable and host is live.
        let mut result = unsafe { tauron_wire_roundtrip_json(host, FRAME.as_ptr(), FRAME.len()) };
        assert_eq!(result.status, TauronStatus::Ok.code());
        // SAFETY: result has not been freed yet.
        let roundtrip = unsafe { bytes(&result.buffer) };
        let decoded: WireFrame<serde_json::Value> =
            decode_wire_json(roundtrip, DEFAULT_MAX_WIRE_BYTES).unwrap();
        assert_eq!(decoded.header.schema, "ffi.test/1");

        // SAFETY: result came from this ABI; second call is idempotent because fields are cleared.
        unsafe {
            tauron_result_free(&mut result);
            tauron_result_free(&mut result);
        }
        assert!(result.buffer.data.is_null());
        assert!(result.error.message.data.is_null());

        // SAFETY: both pointers represent distinct retained references to the same Arc.
        unsafe {
            tauron_host_release(retained);
            tauron_host_release(host);
        }
    }

    #[test]
    fn shutdown_invalidates_operations_but_not_reference_management() {
        let host = tauron_host_new();
        assert!(!host.is_null());
        // SAFETY: host is live.
        assert_eq!(unsafe { tauron_host_shutdown(host) }, TauronStatus::Ok.code());
        assert_eq!(unsafe { tauron_host_shutdown(host) }, TauronStatus::Ok.code());

        // SAFETY: host remains retained; FRAME is readable.
        let mut result = unsafe { tauron_wire_roundtrip_json(host, FRAME.as_ptr(), FRAME.len()) };
        assert_eq!(result.status, TauronStatus::Shutdown.code());
        // SAFETY: error buffer is live until result_free.
        let message = String::from_utf8_lossy(unsafe { bytes(&result.error.message) });
        assert!(message.contains("shut down"));
        // SAFETY: result and host were created by this ABI.
        unsafe {
            tauron_result_free(&mut result);
            tauron_host_release(host);
        }
    }

    #[test]
    fn invalid_json_is_a_structured_owned_error() {
        let host = tauron_host_new();
        let invalid = b"{";
        // SAFETY: host is live and invalid points to one readable byte.
        let mut result =
            unsafe { tauron_wire_roundtrip_json(host, invalid.as_ptr(), invalid.len()) };
        assert_eq!(result.status, TauronStatus::WireError.code());
        assert_eq!(result.error.code, TauronStatus::WireError.code());
        assert!(!result.error.message.owner.is_null());
        // SAFETY: result and host were created by this ABI.
        unsafe {
            tauron_result_free(&mut result);
            tauron_host_release(host);
        }
    }

    #[test]
    fn panic_is_contained_inside_result_boundary() {
        let mut result = guarded_result(|| -> Result<Vec<u8>, FfiFailure> {
            panic!("ffi-test-panic");
        });
        assert_eq!(result.status, TauronStatus::Panic.code());
        // SAFETY: result was created by guarded_result using the same ownership path as externs.
        unsafe { tauron_result_free(&mut result) };
    }

    #[test]
    fn null_and_pointer_length_mismatch_fail_closed() {
        assert!(unsafe { tauron_host_retain(ptr::null_mut()) }.is_null());
        assert_eq!(
            unsafe { tauron_host_shutdown(ptr::null_mut()) },
            TauronStatus::InvalidArgument.code()
        );

        let host = tauron_host_new();
        // SAFETY: null input with non-zero length is intentionally tested and must be rejected
        // before any dereference.
        let mut result = unsafe { tauron_wire_roundtrip_json(host, ptr::null(), 1) };
        assert_eq!(result.status, TauronStatus::InvalidArgument.code());
        unsafe {
            tauron_result_free(&mut result);
            tauron_host_release(host);
        }
    }

    #[test]
    fn golden_language_fixtures_track_the_same_abi_symbols() {
        let golden: serde_json::Value =
            serde_json::from_str(include_str!("../conformance/abi-v1.json")).unwrap();
        assert_eq!(golden["abiVersion"], TAURON_FFI_ABI_VERSION);
        assert_eq!(golden["status"]["ok"], TauronStatus::Ok.code());
        assert_eq!(golden["status"]["shutdown"], TauronStatus::Shutdown.code());
        assert_eq!(golden["status"]["panic"], TauronStatus::Panic.code());

        let header = include_str!("../include/tauron_ffi.h");
        let fixtures = [
            include_str!("../conformance/c/ffi_ownership_asan.c"),
            include_str!("../conformance/csharp/FfiV1.cs"),
            include_str!("../conformance/swift/FfiV1.swift"),
            include_str!("../conformance/kotlin/FfiV1.kt"),
        ];
        for symbol in golden["requiredSymbols"].as_array().unwrap() {
            let symbol = symbol.as_str().unwrap();
            assert!(header.contains(symbol), "header missing {symbol}");
            for fixture in fixtures {
                assert!(fixture.contains(symbol), "golden binding fixture missing {symbol}");
            }
        }
    }
}
