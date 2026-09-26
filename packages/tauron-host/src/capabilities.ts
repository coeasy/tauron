// ──────────────────────────────────────────────────────────────────────────
// 能力探测（§4.9：每能力有 `available()`）。
//
// 能力 = 一个宿主命令 + 授权档位 + 消费方。矩阵与 Rust 侧
// `authz::COMMANDS` / `authz::ADMIN_COMMANDS` 保持同构。
// ──────────────────────────────────────────────────────────────────────────
import type { Backend } from './backend.js';

/** 授权档位，与 Rust `AuthTier` 的 kebab-case 线名一致。 */
export const AUTH_TIERS = ['self', 'scoped-read', 'privileged'] as const;
export type AuthTier = (typeof AUTH_TIERS)[number];

/** 消费方。 */
export const CONSUMERS = ['plugin', 'main-window'] as const;
export type Consumer = (typeof CONSUMERS)[number];

export interface Capability {
  /** 未加前缀的裸命令名。 */
  command: string;
  /** 授权档位。 */
  tier: AuthTier;
  /** 合法消费方。 */
  consumer: Consumer;
  /** 面向开发者的说明。 */
  description: string;
}

/**
 * 框架服务命令面（计划 §2.1，D1/D2/D15/D16 修订后定稿）。
 *
 * - 13 条插件命令：12 条 `self` + 1 条 `scoped-read`
 * - 4 条主窗特权命令：`host_registry_admin`、`host_runtime_spawn` / `host_runtime_health`
 *   （进程管理）与 `host_resource_stats`（跨插件资源占用快照）
 * - `host_grant_request` 已按 D16 在 v1 删除
 * - `host_call_begin` 已按 D2 被 `host_plugin_call` 取代
 *
 * **覆盖范围（有意如此，不是遗漏）**：本表覆盖「插件侧可触达的命令面 + 管理命令」。
 * 主窗专属命令（窗口 / i18n / notify / settings / recovery / market / dialog /
 * clipboard / brand / contributes_list 等）由 Tauri ACL 按窗口 label 管辖，不在此表内。
 * 判据是**谁可能越权**：插件 webview 能摸到的命令必须有档位（否则授权层对它没有定义），
 * 主窗命令不存在"下放"路径。该范围由 `wire-gate` 的「授权面」门禁钉住：
 * `HostClient` 触达的每条命令都必须在本表内且档位为 self/scoped-read。
 *
 * **档位与代码层判定的关系（轮 11 起）**：本表是**契约与门禁**，真正的执行判定在
 * `crates/tauron-adapter`：`privileged` 档命令（+ 主窗管理面）在包装器里
 * `Caller::from_label(window.label())` → 核心 `require_main_window` 拒绝插件主体；
 * 设置族则按键判定（插件只能碰 `plugin:<id>` / `plugin:<id>.…`）。
 * 也就是说"特权 = 仅主窗"**不再只依赖部署配置**（origin 白名单 / Tauri ACL），
 * 代码里有同名判定——表里加了档位却漏接判定，Rust 侧的同源遍历测试会红。
 */
export const CAPABILITIES: readonly Capability[] = [
  {
    command: 'host_plugin_call',
    tier: 'self',
    consumer: 'plugin',
    description: '插件 JS 身份单元调用它自己的 C/D 后端（unary/stream，raw body + Channel）',
  },
  {
    command: 'host_call_end',
    tier: 'self',
    consumer: 'plugin',
    description: 'stream 模式终帧确认',
  },
  {
    command: 'host_cancel',
    tier: 'self',
    consumer: 'plugin',
    description: '取消传播到 sidecar/supervisor',
  },
  {
    command: 'host_call_plugin',
    tier: 'self',
    consumer: 'plugin',
    description: '跨主体调用（宿主→插件 / 插件→插件，0.4-A1）：发起主体由宿主从 label 解析',
  },
  {
    command: 'host_call_result',
    tier: 'self',
    consumer: 'plugin',
    description: '执行方回填一次跨主体调用的结果（身份必须 == 该 call 的 target）',
  },
  {
    command: 'host_call_take',
    tier: 'self',
    consumer: 'plugin',
    description: '发起方取走一次已结算的跨主体调用结果（settled 取走即删）',
  },
  {
    command: 'host_lifecycle_report',
    tier: 'self',
    consumer: 'plugin',
    description: '上报生命周期事件（宿主是状态的唯一写入者）',
  },
  {
    command: 'host_contributes_register',
    tier: 'self',
    consumer: 'plugin',
    description: '注册 contributes（菜单项、设置节、命令面板条目）',
  },
  {
    command: 'host_events_publish',
    tier: 'self',
    consumer: 'plugin',
    description: '跨插件事件发布的唯一通道；越界（未声明 publish）会被丢弃并计数',
  },
  {
    command: 'host_events_subscribe',
    tier: 'self',
    consumer: 'plugin',
    description: '订阅事件选择器（跨插件订阅需审批，见 §4.4）',
  },
  {
    command: 'host_events_unsubscribe',
    tier: 'self',
    consumer: 'plugin',
    description: '按 token 退订',
  },
  {
    command: 'host_registry_list',
    tier: 'scoped-read',
    consumer: 'plugin',
    description: '按可见性过滤后返回插件清单（只看自己 + 已订阅的 public topic）',
  },
  {
    command: 'host_contributes_list',
    tier: 'scoped-read',
    consumer: 'plugin',
    description: '列出贡献表（commands/menus/panels/settings，纯只读；0.4 审计补登记）',
  },
  {
    command: 'host_events_drain',
    tier: 'self',
    consumer: 'plugin',
    description: '插件侧事件取件泵按批取件（只返回到该插件可见集内的事件）',
  },
  {
    command: 'host_stream_open',
    tier: 'self',
    consumer: 'plugin',
    description: '为一次已挂帧载体的调用开流（self 档；映射见 adapter `cmd_stream_open`）',
  },
  {
    command: 'host_stream_write',
    tier: 'self',
    consumer: 'plugin',
    description: '向已开流写入一帧（self 档）',
  },
  {
    command: 'host_stream_close',
    tier: 'self',
    consumer: 'plugin',
    description: '关闭流并校验终帧（self 档）',
  },
  {
    command: 'host_registry_admin',
    tier: 'privileged',
    consumer: 'main-window',
    description: 'disable/enable/uninstall/purge；仅主窗可调用，插件侧不可见',
  },
  {
    command: 'host_registry_install',
    tier: 'privileged',
    consumer: 'main-window',
    description: '安装本地签名插件包并审批声明权限；仅主窗可调用',
  },
  {
    command: 'host_registry_install_preview',
    tier: 'privileged',
    consumer: 'main-window',
    description: '验证签名包并返回权限审批摘要；仅主窗可调用',
  },
  {
    command: 'host_runtime_spawn',
    tier: 'privileged',
    consumer: 'main-window',
    description:
      '启动进程插件 sidecar（P0-2）；**进程执行原语**，插件 webview 不得触发（否则任何插件都能起别人的 sidecar）',
  },
  {
    command: 'host_runtime_health',
    tier: 'privileged',
    consumer: 'main-window',
    description: '按租约查 sidecar 健康（pid/崩溃计数）；暴露 PID，故与 spawn 同档',
  },
  {
    command: 'host_resource_stats',
    tier: 'privileged',
    consumer: 'main-window',
    description: '读取全局及逐插件资源配额占用，仅主窗可见',
  },
] as const;

export type CapabilityCommand = (typeof CAPABILITIES)[number]['command'];

export function capabilityOf(command: string): Capability | undefined {
  return CAPABILITIES.find((c) => c.command === command);
}

/**
 * 判断某能力在当前 Backend 上是否可用。
 *
 * 判据 = 宿主是否注册了该命令。主窗特权命令对插件 webview 不可见，
 * 因此插件侧 `available('host_registry_admin')` 应为 `false`。
 */
export function isAvailable(backend: Backend, command: string): boolean {
  if (!capabilityOf(command)) return false;
  return backend.capabilities().has(command);
}

/**
 * 能力矩阵：每个能力 → 是否可用。
 *
 * 供 `useCoreCapabilities()` / 插件管理页等消费（§4.10 `<oc-plugin-manager>`）。
 */
export function capabilityMatrix(backend: Backend): Record<string, boolean> {
  const out: Record<string, boolean> = {};
  for (const cap of CAPABILITIES) {
    out[cap.command] = isAvailable(backend, cap.command);
  }
  return out;
}
