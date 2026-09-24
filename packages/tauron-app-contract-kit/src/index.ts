// ──────────────────────────────────────────────────────────────────────────
// @tauron/app-contract-kit — 契约测试套件 + 插件测试工具（§4.28）。
//
// 导出：
// - MockRegistry：插件注册表模拟
// - PluginTestRunner：插件测试运行器
// - expect：断言工具
// ──────────────────────────────────────────────────────────────────────────

export {
  MockRegistry,
  createMockRegistry,
  createPopulatedRegistry,
} from './mock-registry.js';
export type {
  MockPluginManifest,
  PluginState,
  RegistryEntry,
  RegistryOpResult,
  OperationRecord,
  RegistrySnapshot,
} from './mock-registry.js';

export {
  PluginTestRunner,
  createTestRunner,
  createMockHostApi,
  expect,
  Expectation,
} from './test-runner.js';
export type {
  TestResult,
  SuiteResult,
  RunResult,
  Assertion,
  MockHostApi,
  TestContext,
  TestSuite,
} from './test-runner.js';
