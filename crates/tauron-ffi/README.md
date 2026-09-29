# tauron-ffi

`tauron-ffi` is Tauron's stable C ABI ownership boundary for non-Rust consumers.

ABI v1 intentionally exposes only the host-neutral Universal Wire validation/round-trip surface.
It does **not** claim that Provider, Runtime, UI, filesystem, network, or plugin-management APIs are
available through FFI yet.

Public ownership rules are defined in `include/tauron_ffi.h` and
`docs/contracts/ffi-v1.md`.

Key rules:

- `tauron_host_t` is opaque.
- `tauron_host_new` / `tauron_host_retain` ownership is balanced by
  `tauron_host_release`.
- Rust-owned result buffers are released only by Tauron free functions.
- Rust panics are caught at every exported execution boundary.
- after `tauron_host_shutdown`, operational calls fail with
  `TAURON_STATUS_SHUTDOWN`; retain/release remain valid.
- ABI version 1 invokes no user callbacks, so callback thread/re-entry semantics are explicitly
  absent rather than implicit.
