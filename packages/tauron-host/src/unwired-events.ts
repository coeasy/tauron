// ──────────────────────────────────────────────────────────────────────────
// 未接线的壳层事件**显式登记表**（1.0-W3）。
//
// 为什么需要这张表：此前的做法是「在 `shell-controller.ts` 的头注里写一句
// 『属接入方域』就算交代过了」——注释不参与任何检查，于是「有派发、无消费者」
// 的事件可以长期存在（`oc-command-select` 就是这样：命令面板点了没反应，
// 而且没有任何出口可查）。
//
// 现在的规则（由 `unwired-events.test.ts` 强制）：
//   每个 `SHELL_EVENTS` 成员，**要么**被 `ShellController` 真实监听，
//   **要么**在本表里显式登记并写明原因。两者都不是 → 门禁红。
//   两者都是（既监听又登记为未接线）→ 同样红（自相矛盾）。
//
// 新增事件时请先想清楚它属于哪一类，不要顺手加进本表——本表是「欠账清单」，
// 不是「免责清单」。
// ──────────────────────────────────────────────────────────────────────────

import { SHELL_EVENTS } from '@tauron/shell-events';

/** 一条未接线的壳层事件及其原因。 */
export interface UnwiredEvent {
  /** 事件名（取自 `SHELL_EVENTS`，不写字面量）。 */
  name: string;
  /** 为什么宿主控制器不接它、以及由谁负责。 */
  reason: string;
}

/**
 * 未被 `ShellController` 监听的事件（每条都必须有真实原因与归属方）。
 *
 * 判定「真的未接线」的口径是**宿主命令面没有对应动作**：
 * 如果只是「还没人接」，那应该去接线，而不是登记进本表。
 */
export const UNWIRED_EVENTS: readonly UnwiredEvent[] = [
  {
    name: SHELL_EVENTS.trayItem,
    reason: '托盘是原生菜单，动作发生在应用装配层（Tauri tray API），宿主命令面无可路由的动作',
  },
  {
    name: SHELL_EVENTS.shortcutChange,
    reason: '快捷键持久化没有宿主命令；接入方自行落盘（宿主不代管用户快捷键偏好）',
  },
  {
    name: SHELL_EVENTS.themeChange,
    reason: '主题应用是改 CSS 变量 / data-theme，纯前端关注点，属接入方域',
  },
  {
    name: SHELL_EVENTS.toastAction,
    reason: '通知内的动作按钮由**发起该 toast 的调用方**处理，控制器无从得知其语义',
  },
  {
    name: SHELL_EVENTS.updaterDismiss,
    reason:
      '「稍后」是收起更新对话框（组件自身已收起），不产生宿主命令；' +
      '宿主命令面无「更新偏好」命令，持久化由接入方监听本事件后自行落盘',
  },
];

/** 未接线事件名集合（门禁与接入方自检用）。 */
export const UNWIRED_EVENT_NAMES: readonly string[] = UNWIRED_EVENTS.map((e) => e.name);
