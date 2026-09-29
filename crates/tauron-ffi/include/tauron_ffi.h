#ifndef TAURON_FFI_H
#define TAURON_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TAURON_FFI_ABI_VERSION 1u

typedef struct tauron_host tauron_host_t;

typedef enum tauron_status {
  TAURON_STATUS_OK = 0,
  TAURON_STATUS_INVALID_ARGUMENT = 1,
  TAURON_STATUS_SHUTDOWN = 2,
  TAURON_STATUS_WIRE_ERROR = 3,
  TAURON_STATUS_PANIC = 4
} tauron_status_t;

/*
 * Rust owns data/owner. Consumers may read data[0..len] but must never mutate or free data/owner
 * directly. Use tauron_buffer_free() or tauron_result_free().
 */
typedef struct tauron_buffer {
  const uint8_t *data;
  size_t len;
  void *owner;
} tauron_buffer_t;

typedef struct tauron_error {
  int32_t code;
  tauron_buffer_t message;
} tauron_error_t;

typedef struct tauron_result {
  int32_t status;
  tauron_buffer_t buffer;
  tauron_error_t error;
} tauron_result_t;

uint32_t tauron_ffi_abi_version(void);

tauron_host_t *tauron_host_new(void);
tauron_host_t *tauron_host_retain(tauron_host_t *host);
void tauron_host_release(tauron_host_t *host);
int32_t tauron_host_shutdown(tauron_host_t *host);

tauron_result_t tauron_wire_roundtrip_json(
    tauron_host_t *host,
    const uint8_t *input,
    size_t input_len);

void tauron_buffer_free(tauron_buffer_t *buffer);
void tauron_error_free(tauron_error_t *error);
void tauron_result_free(tauron_result_t *result);

#ifdef __cplusplus
}
#endif

#endif /* TAURON_FFI_H */
