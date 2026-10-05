/**
 * Shell 管理器（设计文档 §7.2）
 *
 * 管理 4 种 shell 形态的插件加载与生命周期。
 *
 * ⚠️ **诚实边界（V7-P1-03 之后仍然成立）**：下面四条 `start*` 路径至今都是
 * **10ms 延迟模拟**——不读本地文件、不起本地 HTTP 服务器、不建子 webview、
 * 不请求远程 URL。因此 `status: 'ready'` 只说明「模拟流程走完了」，
 * **不说明资源已装载可用**；对外判据是恒为 `true` 的 `simulated` 字段。
 * 上游（安装/更新/可用性判定）必须继续把 simulated shell 挡在「可用」路径之外，
 * 不许因为 `ready` 就放行。
 *
 * 本轮改的是**时序与错误传播**（不是把模拟接成真实装载）：
 * - 每个实例一个单调代际；`start`/`stop`/`destroy` 都会抬高它。
 * - 状态只能经 `commit()`/`writeStatus()` 写入，且必须过代际闸门 + 迁移表：
 *   陈旧启动再也无法把已 `stop`/`destroy` 的实例写成 `ready`（resurrection）。
 * - 启动失败**一定 reject**（不再 resolve 却留 `status: 'error'` 的假成功）；
 *   被作废的启动 reject 为 `ShellStartAbandonedError`。
 */

import type {
  LocalServerShellConfig,
  LocalShellConfig,
  RemoteUrlShellConfig,
  ShellConfig,
  ShellInstance,
  ShellManager,
  ShellManagerOptions,
  ShellStartProvider,
  ShellStatus,
  SubWebviewShellConfig,
} from './types.js';

/**
 * 状态迁移表（V7-P1-03）：只有表里出现的迁移允许写入。
 *
 * 这张表是「状态即对外可用性判据」的护栏——除了 start/stop 的正常路径，
 * 不该有任何一条路径能改写状态（尤其不能从 `unloaded`/`error` 直接跳 `ready`）。
 */
const SHELL_STATUS_TRANSITIONS: Record<ShellStatus, readonly ShellStatus[]> = {
  // 启动中：可落 ready/error，可被 stop 打断，可被再次 start 重启。
  loading: ['loading', 'ready', 'error', 'unloaded'],
  // 已 ready：只能重启或 stop。
  ready: ['loading', 'unloaded'],
  // 失败：只能重启或 stop。
  error: ['loading', 'unloaded'],
  // 已停：只能重启或重复 stop（幂等）。
  unloaded: ['loading', 'unloaded'],
};

/**
 * 启动被新一代作废时抛出的错误。
 *
 * ⚠️ 本包自有码（**不是** wire 码，不进 `contracts/error/error-codes.json`）：
 * `SHELL_START_ABANDONED` = 「这次启动的结果已作废，别当成成功」；
 * 装载失败走的是另一条分支（reject 原始 cause + `status: 'error'`）。
 */
export class ShellStartAbandonedError extends Error {
  readonly code = 'SHELL_START_ABANDONED';
  readonly pluginId: string;
  readonly generation: number;

  constructor(pluginId: string, generation: number, cause?: Error) {
    super(
      `Shell start abandoned for plugin '${pluginId}': generation ${generation} 已被新的代际作废，结果已丢弃`,
      cause === undefined ? undefined : { cause },
    );
    this.name = 'ShellStartAbandonedError';
    this.pluginId = pluginId;
    this.generation = generation;
  }
}

/** 实例需要的管理器侧能力（计数与装载实现都从这里进来）。 */
interface ShellRuntime {
  start(config: ShellConfig): Promise<void>;
  recordAbandonedStart(): void;
  recordIllegalTransition(): void;
  recordGeneration(): void;
}

/** 模拟延迟：真实装载实现接入后由真实等待取代。 */
function simulateDelay(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 10));
}

async function startLocalShell(_config: LocalShellConfig): Promise<void> {
  // Simulate loading local file —— 只等待，不读 `_config.entryPath`。
  await simulateDelay();
}

async function startLocalServerShell(_config: LocalServerShellConfig): Promise<void> {
  // Simulate starting local server —— 只等待，不起 `_config.host:_config.port`。
  await simulateDelay();
}

async function startRemoteUrlShell(_config: RemoteUrlShellConfig): Promise<void> {
  // Simulate loading remote URL —— 只等待，不发请求、不校验 `_config.url` 的 CSP/域名。
  await simulateDelay();
}

async function startSubWebviewShell(_config: SubWebviewShellConfig): Promise<void> {
  // Simulate creating sub-webview —— 只等待，不创建任何 webview。
  await simulateDelay();
}

/** 内置装载实现：按形态分发到 4 条模拟路径（全部无真实装载，见文件头）。 */
function simulateShellStart(config: ShellConfig): Promise<void> {
  switch (config.form) {
    case 'local':
      return startLocalShell(config as LocalShellConfig);
    case 'local-server':
      return startLocalServerShell(config as LocalServerShellConfig);
    case 'remote-url':
      return startRemoteUrlShell(config as RemoteUrlShellConfig);
    case 'sub-webview':
      return startSubWebviewShell(config as SubWebviewShellConfig);
  }
}

/**
 * 创建 Shell 实例。
 *
 * `status` 仍是对外的普通字段（读方便、与 `ShellInstance` 契约一致），
 * 但**本文件内**的所有写入都必须走 `writeStatus`/`commit` 两个闸门。
 */
function createShell(config: ShellConfig, runtime: ShellRuntime): ShellInstance {
  /** 本实例的当前代际：create 起算 0，`start`/`stop`/`destroy`(经 `stop`) 各 +1。 */
  let generation = 0;
  /** 最近一次在途的启动任务；`stop()` 必须等它结束，才能断言「异步启动已死」。 */
  let inFlight: Promise<void> | undefined;

  const isCurrent = (token: number): boolean => token === generation;

  /** 迁移表闸门：表外写入一律拒绝并计数（状态机不许说谎）。 */
  function writeStatus(next: ShellStatus): boolean {
    const from = instance.status;
    if (!SHELL_STATUS_TRANSITIONS[from].includes(next)) {
      runtime.recordIllegalTransition();
      return false;
    }
    instance.status = next;
    return true;
  }

  /** 代际闸门 + 迁移表闸门。返回 false 表示这次写入被丢弃（陈旧启动的完成）。 */
  function commit(token: number, next: ShellStatus): boolean {
    if (!isCurrent(token)) {
      runtime.recordAbandonedStart();
      return false;
    }
    return writeStatus(next);
  }

  const instance: ShellInstance = {
    config,
    status: 'loading',
    // 诚实标记：本包 `start()` 只做模拟加载（见文件头注释），恒为 true。
    simulated: true,

    async start(): Promise<void> {
      // 每次 start 开**新的一代**：此前在途的启动随即失效（双 start 不重叠）。
      const token = (generation += 1);
      runtime.recordGeneration();
      writeStatus('loading');

      const task = (async (): Promise<void> => {
        try {
          await runtime.start(config);
        } catch (err) {
          const cause = err instanceof Error ? err : new Error(String(err));
          if (!isCurrent(token)) {
            // 陈旧启动的失败同样丢弃：它的结局与新一代无关。
            runtime.recordAbandonedStart();
            throw new ShellStartAbandonedError(config.pluginId, token, cause);
          }
          // 失败必须让调用方看到：状态落 `error` **且** reject 原始 cause。
          instance.error = cause.message;
          writeStatus('error');
          throw cause;
        }
        if (!commit(token, 'ready')) {
          throw new ShellStartAbandonedError(config.pluginId, token);
        }
      })();

      inFlight = task;
      try {
        await task;
      } finally {
        // 只清理仍属于本次的那条：新一代 start 可能已经换掉了它。
        if (inFlight === task) inFlight = undefined;
      }
    },

    async stop(): Promise<void> {
      // 顺序即语义：① 抬高代际作废在途启动 → ② 落 `unloaded` → ③ 等它结束。
      // 少了 ① 或 ③，迟到的 start 就会把 unloaded 写回 ready（resurrection）。
      generation += 1;
      const pending = inFlight;
      writeStatus('unloaded');
      if (pending) {
        await pending.catch(() => {});
      }
    },

    getUrl(): string {
      switch (config.form) {
        case 'local':
          return `file://${(config as LocalShellConfig).entryPath}`;
        case 'local-server':
          return `http://${(config as LocalServerShellConfig).host}:${(config as LocalServerShellConfig).port}/`;
        case 'remote-url':
          return (config as RemoteUrlShellConfig).url;
        case 'sub-webview':
          return (config as SubWebviewShellConfig).webviewUrl;
      }
    },
  };

  return instance;
}

/**
 * 创建 Shell 管理器
 */
export function createShellManager(options: ShellManagerOptions = {}): ShellManager {
  const shells = new Map<string, ShellInstance>();
  const counters = { abandonedStarts: 0, illegalTransitions: 0, generations: 0 };

  /** 装载实现：优先取该形态的注入实现，否则走内置 10ms 模拟。 */
  const start: ShellStartProvider = (config) => {
    const override = options.startProviders?.[config.form];
    // override 同样不接取消信号：它落定时代际仍不符，就只会被记成 abandonedStarts。
    return override ? override(config) : simulateShellStart(config);
  };

  const runtime: ShellRuntime = {
    start,
    recordAbandonedStart: () => {
      counters.abandonedStarts += 1;
    },
    recordIllegalTransition: () => {
      counters.illegalTransitions += 1;
    },
    recordGeneration: () => {
      counters.generations += 1;
    },
  };

  return {
    create(config: ShellConfig): ShellInstance {
      if (shells.has(config.pluginId)) {
        throw new Error(`Shell for plugin '${config.pluginId}' already exists`);
      }
      const instance = createShell(config, runtime);
      shells.set(config.pluginId, instance);
      return instance;
    },

    get(pluginId: string): ShellInstance | undefined {
      return shells.get(pluginId);
    },

    list(): ShellInstance[] {
      return Array.from(shells.values());
    },

    async destroy(pluginId: string): Promise<void> {
      const instance = shells.get(pluginId);
      if (instance) {
        // `stop()` 已作废代际并等在途启动结束：删表项之后不存在还能写状态的
        // 异步启动任务（先删后 stop 就证明不了这一点）。
        await instance.stop();
        shells.delete(pluginId);
      }
    },

    async clear(): Promise<void> {
      const entries = Array.from(shells);
      // `stop()` 在它的第一个 await **之前**就抬高代际，所以这里用 map 一次性
      // 同步作废全部实例，再一起等——逐个 `await stop()` 会漏：前面实例等待期间,
      // 后面实例的在途启动仍能提交 ready。
      const stopping = entries.map(([, instance]) => instance.stop());
      await Promise.all(stopping);
      for (const [pluginId] of entries) {
        shells.delete(pluginId);
      }
    },

    stats() {
      return { ...counters };
    },
  };
}
