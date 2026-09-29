import TauronFFI

// V4 A107 golden fixture. The generated/imported C header remains the ownership source of truth.
func tauronFfiV1GoldenFixture() {
    precondition(tauron_ffi_abi_version() == TAURON_FFI_ABI_VERSION)
    let host = tauron_host_new()
    precondition(host != nil)
    let retained = tauron_host_retain(host)
    _ = tauron_host_shutdown(host)
    var result = tauron_wire_roundtrip_json(host, nil, 0)
    tauron_buffer_free(&result.buffer)
    tauron_error_free(&result.error)
    tauron_result_free(&result)
    tauron_host_release(retained)
    tauron_host_release(host)

    // Canonical symbols kept explicit for drift gates:
    _ = tauron_ffi_abi_version
    _ = tauron_host_new
    _ = tauron_host_retain
    _ = tauron_host_release
    _ = tauron_host_shutdown
    _ = tauron_wire_roundtrip_json
    _ = tauron_buffer_free
    _ = tauron_error_free
    _ = tauron_result_free
}
