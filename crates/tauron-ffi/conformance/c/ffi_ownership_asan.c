#include "tauron_ffi.h"

#include <assert.h>
#include <string.h>

static const char *FRAME =
    "{\"header\":{\"wireVersion\":1,\"codec\":\"json-v1\","
    "\"schema\":\"ffi.asan/1\",\"generation\":5},"
    "\"payload\":{\"value\":\"ok\"}}";

int main(void) {
  assert(tauron_ffi_abi_version() == TAURON_FFI_ABI_VERSION);

  tauron_host_t *host = tauron_host_new();
  assert(host != NULL);
  tauron_host_t *retained = tauron_host_retain(host);
  assert(retained == host);

  tauron_result_t ok = tauron_wire_roundtrip_json(
      host, (const uint8_t *)FRAME, strlen(FRAME));
  assert(ok.status == TAURON_STATUS_OK);
  assert(ok.buffer.data != NULL);
  assert(ok.buffer.len > 0);
  tauron_result_free(&ok);
  tauron_result_free(&ok); /* same struct pointer is intentionally idempotent */

  const char *bad_json = "{";
  tauron_result_t bad = tauron_wire_roundtrip_json(
      host, (const uint8_t *)bad_json, strlen(bad_json));
  assert(bad.status == TAURON_STATUS_WIRE_ERROR);
  assert(bad.error.message.data != NULL);
  tauron_buffer_free(&bad.buffer);
  tauron_error_free(&bad.error);

  assert(tauron_host_shutdown(host) == TAURON_STATUS_OK);
  assert(tauron_host_shutdown(host) == TAURON_STATUS_OK);

  tauron_result_t closed = tauron_wire_roundtrip_json(
      retained, (const uint8_t *)FRAME, strlen(FRAME));
  assert(closed.status == TAURON_STATUS_SHUTDOWN);
  tauron_result_free(&closed);

  tauron_host_release(retained);
  tauron_host_release(host);

  /* Golden symbol inventory: tauron_ffi_abi_version tauron_host_new
   * tauron_host_retain tauron_host_release tauron_host_shutdown
   * tauron_wire_roundtrip_json tauron_buffer_free tauron_error_free tauron_result_free */
  return 0;
}
