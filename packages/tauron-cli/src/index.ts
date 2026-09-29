/**
 * @tauron/cli — tauron CLI 工具
 *
 * 设计文档引用：
 * - §7 CLI 工具
 * - §7.1 create-tauron-app
 * - §7.2 plugin new/dev/test/pack/sign/publish
 * - §7.3 doctor 诊断
 */

// ---- CLI 入口 ----
export { runCli, type CliResult } from './cli.js';

// ---- Doctor ----
export { runDoctor, formatDoctorReport, type DoctorReport, type DoctorCheck } from './doctor.js';

// ---- V4 Host Conformance ----
export {
  HOST_CONFORMANCE_SCENARIOS,
  formatHostConformanceReport,
  loadHostConformanceAdapter,
  runHostConformance,
} from './conformance.js';
export type {
  ConformanceCheck,
  HostCapabilitySnapshot,
  HostConformanceAdapter,
  HostConformanceMetadata,
  HostConformanceReport,
  HostConformanceScenario,
  HostResourceSnapshot,
  HostScenarioEvidence,
} from './conformance.js';

// ---- Scaffold ----
export { createApp, type AppScaffoldResult } from './scaffold.js';

// ---- Plugin ----
export { pluginNew, generatePluginManifest, type PluginCreateResult } from './plugin.js';

// ---- Types ----
export type { PluginConfig, AppConfig, PackageManifest, CliOptions, PluginType } from './types.js';
