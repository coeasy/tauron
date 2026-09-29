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

Current boundary:

`RemoteTransportEvidence` is trusted evidence from a concrete transport adapter. The official
Remote TLS network reference adapter is still open. Until that adapter establishes TLS itself,
Tauron must not claim that the complete Remote Host transport is finished.

No JSON field such as `tls=true`, `userId` or `pluginId` may be converted directly into trusted
transport evidence or Principal identity.
