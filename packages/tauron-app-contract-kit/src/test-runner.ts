// ──────────────────────────────────────────────────────────────────────────
// PluginTestRunner：插件测试运行器（§4.28 插件测试工具）。
//
// 用途：
// - 运行插件的单元测试
// - 模拟宿主 API 调用
// - 记录测试执行结果
// - 支持异步测试
//
// 与真实测试框架的区别：
// - 轻量级（无外部依赖）
// - 专注于插件测试场景
// - 提供模拟宿主 API
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型定义
// ──────────────────────────────────────────────────────────────────────────

/** 测试用例结果。 */
export interface TestResult {
  name: string;
  suite: string;
  success: boolean;
  durationMs: number;
  error?: Error;
  assertionCount: number;
}

/** 测试套件结果。 */
export interface SuiteResult {
  suite: string;
  tests: TestResult[];
  success: boolean;
  durationMs: number;
  assertionCount: number;
  error?: Error;
}

/** 测试运行器结果。 */
export interface RunResult {
  suites: SuiteResult[];
  totalTests: number;
  passed: number;
  failed: number;
  durationMs: number;
  success: boolean;
}

/** 测试断言。 */
export interface Assertion {
  name: string;
  success: boolean;
  expected?: unknown;
  actual?: unknown;
  error?: string;
}

/** 模拟宿主 API。 */
export interface MockHostApi {
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  getSettings: (namespace: string, key: string) => unknown;
  setSettings: (namespace: string, key: string, value: unknown) => void;
  getPluginData: (pluginId: string, key: string) => unknown;
  setPluginData: (pluginId: string, key: string, value: unknown) => void;
  requestPermission: (permission: string) => Promise<boolean>;
  log: (level: 'debug' | 'info' | 'warn' | 'error', message: string, data?: unknown) => void;
}

/** 测试上下文。 */
export interface TestContext {
  pluginId: string;
  mockHost: MockHostApi;
  beforeEach: (fn: () => void | Promise<void>) => void;
  afterEach: (fn: () => void | Promise<void>) => void;
  setTimeout: (ms: number) => void;
  skip: (name: string, fn: () => void | Promise<void>) => void;
  only: (name: string, fn: () => void | Promise<void>) => void;
  fail: (message: string) => void;
  pass: (message?: string) => void;
}

// ──────────────────────────────────────────────────────────────────────────
// PluginTestRunner 实现
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件测试运行器。
 *
 * 提供轻量级的测试执行环境，支持插件单元测试。
 */
export class PluginTestRunner {
  private suites: Map<string, { beforeEach: Array<() => void | Promise<void>>; afterEach: Array<() => void | Promise<void>>; tests: Array<{ name: string; fn: () => void | Promise<void>; skip: boolean; only: boolean; timeout: number }> }>;
  private results: RunResult | null;
  private assertions: Assertion[];
  private startedAt: number;

  constructor() {
    this.suites = new Map();
    this.results = null;
    this.assertions = [];
    this.startedAt = 0;
  }

  /**
   * 创建测试套件。
   */
  describe(suiteName: string, fn: (suite: TestSuite) => void): void {
    const suite: { beforeEach: Array<() => void | Promise<void>>; afterEach: Array<() => void | Promise<void>>; tests: Array<{ name: string; fn: () => void | Promise<void>; skip: boolean; only: boolean; timeout: number }> } = {
      beforeEach: [],
      afterEach: [],
      tests: [],
    };

    this.suites.set(suiteName, suite);

    const testSuite: TestSuite = {
      beforeEach: (callback) => suite.beforeEach.push(callback),
      afterEach: (callback) => suite.afterEach.push(callback),
      it: (name, fn, timeout = 5000) => {
        suite.tests.push({ name, fn, skip: false, only: false, timeout });
      },
      skip: (name, fn) => {
        suite.tests.push({ name, fn, skip: true, only: false, timeout: 5000 });
      },
      only: (name, fn) => {
        suite.tests.push({ name, fn, skip: false, only: true, timeout: 5000 });
      },
    };

    fn(testSuite);
  }

  /**
   * 运行所有测试。
   */
  async run(): Promise<RunResult> {
    this.startedAt = Date.now();
    this.assertions = [];

    const suites: SuiteResult[] = [];
    let totalTests = 0;
    let passed = 0;
    let failed = 0;

    for (const [suiteName, suite] of this.suites) {
      const suiteResult: SuiteResult = {
        suite: suiteName,
        tests: [],
        success: true,
        durationMs: 0,
        assertionCount: 0,
      };

      const suiteStart = Date.now();

      try {
        // 执行 beforeEach
        for (const before of suite.beforeEach) {
          await before();
        }

        // 执行测试
        for (const test of suite.tests) {
          if (test.skip) {
            continue;
          }

          totalTests++;
          const testStart = Date.now();

          try {
            await Promise.race([
              test.fn(),
              new Promise<never>((_, reject) =>
                setTimeout(() => reject(new Error(`测试超时（${test.timeout}ms）`)), test.timeout)
              ),
            ]);

            const testResult: TestResult = {
              name: test.name,
              suite: suiteName,
              success: true,
              durationMs: Date.now() - testStart,
              assertionCount: 0,
            };

            suiteResult.tests.push(testResult);
            suiteResult.assertionCount += testResult.assertionCount;
            passed++;
          } catch (error) {
            const testResult: TestResult = {
              name: test.name,
              suite: suiteName,
              success: false,
              durationMs: Date.now() - testStart,
              error: error as Error,
              assertionCount: 0,
            };

            suiteResult.tests.push(testResult);
            suiteResult.assertionCount += testResult.assertionCount;
            failed++;
          }
        }

        // 执行 afterEach
        for (const after of suite.afterEach) {
          await after();
        }
      } catch (error) {
        suiteResult.success = false;
        suiteResult.error = error as Error;
      }

      suiteResult.durationMs = Date.now() - suiteStart;
      suites.push(suiteResult);
    }

    this.results = {
      suites,
      totalTests,
      passed,
      failed,
      durationMs: Date.now() - this.startedAt,
      success: failed === 0,
    };

    return this.results;
  }

  /**
   * 获取测试结果。
   */
  getResult(): RunResult | null {
    return this.results;
  }

  /**
   * 获取断言记录。
   */
  getAssertions(): Assertion[] {
    return [...this.assertions];
  }

  /**
   * 创建测试上下文。
   */
  createContext(pluginId: string): TestContext {
    const mockHost = createMockHostApi(pluginId);

    const context: TestContext = {
      pluginId,
      mockHost,
      beforeEach: () => {},
      afterEach: () => {},
      setTimeout: () => {},
      skip: () => {},
      only: () => {},
      fail: (message) => {
        throw new Error(`测试失败：${message}`);
      },
      pass: () => {},
    };

    return context;
  }

  /**
   * 重置运行器。
   */
  reset(): void {
    this.suites.clear();
    this.results = null;
    this.assertions = [];
    this.startedAt = 0;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试套件接口
// ──────────────────────────────────────────────────────────────────────────

export interface TestSuite {
  beforeEach: (fn: () => void | Promise<void>) => void;
  afterEach: (fn: () => void | Promise<void>) => void;
  it: (name: string, fn: () => void | Promise<void>, timeout?: number) => void;
  skip: (name: string, fn: () => void | Promise<void>) => void;
  only: (name: string, fn: () => void | Promise<void>) => void;
}

// ──────────────────────────────────────────────────────────────────────────
// 模拟宿主 API
// ──────────────────────────────────────────────────────────────────────────

/**
 * 创建模拟宿主 API。
 *
 * `pluginId` 是该 mock 宿主会话所代表的插件身份：模拟命令调用时回显它，
 * 便于断言"哪个插件发起了调用"。
 */
export function createMockHostApi(pluginId: string): MockHostApi {
  const settings = new Map<string, unknown>();
  const pluginData = new Map<string, unknown>();
  const permissions = new Set<string>();
  const logs: Array<{ level: string; message: string; data?: unknown; timestamp: string }> = [];

  return {
    async invoke(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
      // 模拟命令调用
      return {
        command,
        args,
        pluginId,
        success: true,
        timestamp: new Date().toISOString(),
      };
    },

    getSettings(namespace: string, key: string): unknown {
      const fullKey = `${namespace}.${key}`;
      return settings.get(fullKey);
    },

    setSettings(namespace: string, key: string, value: unknown): void {
      const fullKey = `${namespace}.${key}`;
      settings.set(fullKey, value);
    },

    getPluginData(_pluginId: string, key: string): unknown {
      return pluginData.get(key);
    },

    setPluginData(_pluginId: string, key: string, value: unknown): void {
      pluginData.set(key, value);
    },

    async requestPermission(permission: string): Promise<boolean> {
      permissions.add(permission);
      return true;
    },

    log(level: 'debug' | 'info' | 'warn' | 'error', message: string, data?: unknown): void {
      logs.push({
        level,
        message,
        data,
        timestamp: new Date().toISOString(),
      });
    },
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 工厂函数
// ──────────────────────────────────────────────────────────────────────────

/**
 * 创建 PluginTestRunner 实例。
 */
export function createTestRunner(): PluginTestRunner {
  return new PluginTestRunner();
}

// ──────────────────────────────────────────────────────────────────────────
// 辅助断言函数
// ──────────────────────────────────────────────────────────────────────────

/**
 * 断言相等。
 */
export function expect(actual: unknown): Expectation {
  return new Expectation(actual);
}

/**
 * 断言对象。
 */
export class Expectation {
  private actual: unknown;

  constructor(actual: unknown) {
    this.actual = actual;
  }

  /**
   * 断言等于。
   */
  toEqual(expected: unknown): void {
    if (this.actual !== expected) {
      throw new Error(`期望 ${JSON.stringify(expected)}，实际 ${JSON.stringify(this.actual)}`);
    }
  }

  /**
   * 断言不等于。
   */
  notToEqual(expected: unknown): void {
    if (this.actual === expected) {
      throw new Error(`期望不等于 ${JSON.stringify(expected)}，但实际相等`);
    }
  }

  /**
   * 断言为真。
   */
  toBeTruthy(): void {
    if (!this.actual) {
      throw new Error(`期望为真，实际为 ${JSON.stringify(this.actual)}`);
    }
  }

  /**
   * 断言为假。
   */
  toBeFalsy(): void {
    if (this.actual) {
      throw new Error(`期望为假，实际为 ${JSON.stringify(this.actual)}`);
    }
  }

  /**
   * 断言为 undefined。
   */
  toBeUndefined(): void {
    if (this.actual !== undefined) {
      throw new Error(`期望为 undefined，实际为 ${JSON.stringify(this.actual)}`);
    }
  }

  /**
   * 断言为 null。
   */
  toBeNull(): void {
    if (this.actual !== null) {
      throw new Error(`期望为 null，实际为 ${JSON.stringify(this.actual)}`);
    }
  }

  /**
   * 断言匹配正则。
   */
  toMatch(pattern: RegExp): void {
    if (typeof this.actual !== 'string' || !pattern.test(this.actual)) {
      throw new Error(`期望匹配 ${pattern}，实际为 ${JSON.stringify(this.actual)}`);
    }
  }

  /**
   * 断言包含。
   */
  toContain(expected: unknown): void {
    if (Array.isArray(this.actual)) {
      if (!this.actual.includes(expected)) {
        throw new Error(`期望包含 ${JSON.stringify(expected)}，实际为 ${JSON.stringify(this.actual)}`);
      }
    } else if (typeof this.actual === 'string') {
      if (!this.actual.includes(expected as string)) {
        throw new Error(`期望包含 ${expected}，实际为 ${this.actual}`);
      }
    } else {
      throw new Error(`期望包含 ${JSON.stringify(expected)}，但实际不是数组或字符串`);
    }
  }

  /**
   * 断言抛出错误。
   */
  toThrow(expected?: string): void {
    if (typeof this.actual !== 'function') {
      throw new Error('期望是函数');
    }

    let threw = false;
    let error: Error | null = null;

    try {
      (this.actual as () => void)();
    } catch (e) {
      threw = true;
      error = e as Error;
    }

    if (!threw) {
      throw new Error('期望抛出错误，但实际没有');
    }

    if (expected && error && !error.message.includes(expected)) {
      throw new Error(`期望错误包含 "${expected}"，实际为 "${error.message}"`);
    }
  }

  /**
   * 断言类型。
   */
  toBeType(type: string): void {
    const actualType = typeof this.actual;
    if (actualType !== type) {
      throw new Error(`期望类型 ${type}，实际为 ${actualType}`);
    }
  }
}
