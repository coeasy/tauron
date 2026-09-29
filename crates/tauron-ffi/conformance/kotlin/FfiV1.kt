@file:OptIn(kotlinx.cinterop.ExperimentalForeignApi::class)

import kotlinx.cinterop.*
import tauron_ffi.*

private const val EXPECTED_ABI_VERSION: UInt = 1u

// V4 A107 golden fixture for Kotlin/Native cinterop. The C header owns the ABI/lifetime rules.
fun tauronFfiV1GoldenFixture() = memScoped {
    check(tauron_ffi_abi_version() == EXPECTED_ABI_VERSION)
    val host = tauron_host_new()
    check(host != null)
    val retained = tauron_host_retain(host)
    tauron_host_shutdown(host)

    val result = tauron_wire_roundtrip_json(host, null, 0u)
    val resultVar = alloc<tauron_result_t>()
    resultVar.value = result
    tauron_buffer_free(resultVar.ptr.reinterpret())
    tauron_error_free(resultVar.ptr.reinterpret())
    tauron_result_free(resultVar.ptr)

    tauron_host_release(retained)
    tauron_host_release(host)

    // Canonical symbol inventory used by the drift gate:
    ::tauron_ffi_abi_version
    ::tauron_host_new
    ::tauron_host_retain
    ::tauron_host_release
    ::tauron_host_shutdown
    ::tauron_wire_roundtrip_json
    ::tauron_buffer_free
    ::tauron_error_free
    ::tauron_result_free
}
