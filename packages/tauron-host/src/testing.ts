// ──────────────────────────────────────────────────────────────────────────
// 契约测试专用出口（`@tauron/host/testing`）。
//
// 单独成出口是为了让打包器能把 mock 代码排除在交付产物之外
// （§4.9 / ADR-16：Backend 的浏览器实现仅供契约测试，不作为 Web 交付）。
// ──────────────────────────────────────────────────────────────────────────

export { MockBackend } from './backend.js';
export type { MockBackendOptions, MockInvokeCase } from './backend.js';

export { CAPABILITIES, capabilityOf, isAvailable, capabilityMatrix } from './capabilities.js';

export { HostClient, AdminClient, FrameSink } from './host.js';
export type { HostClientOptions, PluginCallRequest } from './host.js';

export { HOST_ERROR_CODES, RETRYABLE_HOST_ERROR_CODES, normalizeError, HostException } from './errors.js';
export type { HostErrorCode, HostErrorShape } from './errors.js';
