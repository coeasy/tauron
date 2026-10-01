# Tauron FFI v1 Ownership Contract

This document is the normative ownership supplement for V4 A72/A107. The C header in
`crates/tauron-ffi/include/tauron_ffi.h` is the binary-facing contract.

## Scope

FFI v1 exposes the **Universal Wire ownership boundary only**. It validates and round-trips the
same `WireFrame<json-v1>` implemented by `tauron-host`. It does not claim that OS Providers,
plugin installation, process/WASM runtimes, UI capabilities, or the full Host command surface are
already bound through C.

## ABI and layout

- ABI version is `1` and is returned by `tauron_ffi_abi_version()`.
- Rust structs/enums are never exposed by layout.
- `tauron_host_t` is opaque.
- Only explicit `repr(C)` wire ownership records are represented in the C header:
  `tauron_buffer_t`, `tauron_error_t`, and `tauron_result_t`.
- Unknown future ABI versions must be rejected by the binding before normal use.

## Host ownership

`tauron_host_new()` creates one retained reference.

`tauron_host_retain(host)` creates one additional retained reference and may return the same
opaque pointer value.

Every successful new/retain reference must be balanced by exactly one
`tauron_host_release(host)`. The pointer becomes invalid after its final release. Calling release
again after final release is caller misuse and is outside the ABI contract.

`tauron_host_shutdown(host)` is idempotent. Shutdown invalidates operational calls but does not
invalidate retained references; callers must still release every reference.

## Buffer and error ownership

Tauron allocates result payloads and error messages with the Rust allocator. Foreign code must
never call `free()`, `delete`, language GC deallocation, or mutate the opaque `owner` token.

Use one of:

- `tauron_buffer_free()`
- `tauron_error_free()`
- `tauron_result_free()`

The free functions clear the caller-owned record, so repeating the same free function on the same
record pointer is safe. Copying a record and freeing both copies is not allowed because both copies
contain the same ownership token.

## Panic and error contract

No Rust panic may cross the C ABI. Exported execution paths use a catch-unwind boundary and return
`TAURON_STATUS_PANIC` on panic.

Stable v1 statuses:

- 0: OK
- 1: INVALID_ARGUMENT
- 2: SHUTDOWN
- 3: WIRE_ERROR
- 4: PANIC

Messages are diagnostic only; machine logic uses the numeric status.

## Threads, callbacks and re-entry

Host retain/release and shutdown state use thread-safe atomics/Arc ownership.

ABI v1 invokes **no foreign callbacks**. Therefore callback thread selection and callback re-entry
are explicitly **not present** in v1 rather than being left undefined. Any future callback API must
declare execution domain, re-entry policy, lifetime and cancellation before it can enter the stable
ABI.

## Conformance

The crate carries golden fixtures for C, C#, Swift and Kotlin/Native. They share the same symbol
inventory and ABI version.

CI additionally builds the real `cdylib`, links the C ownership fixture against it, and runs that
fixture under AddressSanitizer with leak detection enabled. This is the V4
`FFI Ownership / Sanitizer` gate.
