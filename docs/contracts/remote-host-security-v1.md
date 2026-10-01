# Remote Host Security Contract v1

The canonical implementation is `tauron-host::remote_host`.

Implemented now:

- authenticated transport evidence must report TLS 1.3 and server authentication;
- short-lived one-time credentials;
- audience and origin binding;
- Principal minted from server-side credential facts, never request payload identity;
- bounded total sessions and per-subject session quota;
- ACTIVE/SUSPENDED/CLOSED/EXPIRED lifecycle;
- bounded resume window with rotating resume token;
- exact request sequence plus bounded nonce replay window;
- per-session rate limit and inflight quota;
- idle timeout and absolute session age;
- credential rotation/revocation;
- canonical Universal Wire handling after security checks;
- malformed wire still consumes sequence and releases inflight accounting.

Concrete reference transport:

- `tauron-host::remote_host_reference` is the official TCP/rustls reference adapter;
- both server and client configurations are TLS1.3-only;
- the client verifies the server certificate chain and server name through rustls;
- the server mints `RemoteTransportEvidence::tls13` only after the real TLS handshake completes;
- one-time credential, audience/origin binding, replay/rate/inflight checks and Universal Wire are
  still enforced by `RemoteHostSecurity`;
- `examples/remote_host_tls_reference.rs` is executed in the native target matrix.

`RemoteTransportEvidence` remains a trusted adapter-only value. No JSON field such as `tls=true`,
`userId` or `pluginId` may be converted directly into transport evidence or Principal identity.
