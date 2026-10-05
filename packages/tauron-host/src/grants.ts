// ──────────────────────────────────────────────────────────────────────────
// §4.5 权限授予与审批契约（Rust `tauron-acl` 的线上协议镜像）。
//
// 关键约束：
// - 授予集落盘带版本并**签名防篡改**（HMAC-SHA256，`signature` 为 hex）；
// - 权限或 scope 变多必须重新审批（{@link GrantDiff.requiresReapproval}）；
// - 撤销语义如实（ADR-05）：收缩不需要重新审批；
// - 高危档逐条展开成人话并默认不勾（`defaultChecked` 由宿主判定，见 ApprovalRow 注释）；
// - 审批 UI 的**文案与判定都不在本文件重述**：行由 Rust `tauron_acl::build_approval_rows`
//   单源生成，本文件只是它的线上协议镜像（轮 56 收口：此前这里有一份字段名与线上不一致、
//   且无人构造的平行类型，已改为逐字段镜像）。
// ──────────────────────────────────────────────────────────────────────────

import type { JsonValue } from './events.js';

/** 风险档位。线名与 Rust `Risk` 的 lowercase 序列化一致。 */
export const RISKS = ['low', 'elevated', 'high'] as const;
export type Risk = (typeof RISKS)[number];

/** 授予集 schema 版本。破坏性变更时与 Rust 侧同步自增。 */
export const GRANT_SET_SCHEMA_VERSION = 1;

/** 一条已批准的权限。 */
export interface GrantEntry {
  permission: string;
  risk: Risk;
  /** 人话描述，来自权限词表。 */
  description: string;
  scoped: boolean;
}

/** 一个插件的已批准授予集。字段名与 Rust 的 camelCase 序列化一致。 */
export interface GrantSet {
  schemaVersion: number;
  /** 授予集修订号：每次变更自增。 */
  revision: number;
  pluginId: string;
  pluginVersion: string;
  /** manifest 的 `framework` range。 */
  framework: string;
  grants: GrantEntry[];
  /** 权限标识 -> scope 值。 */
  scopes: Record<string, JsonValue>;
  /** 批准时刻（unix 秒）。 */
  approvedAtUnix: number;
  approver: string;
}

/** 签名后的授予集（落盘形态）。 */
export interface SignedGrantSet {
  grantSet: GrantSet;
  /** HMAC-SHA256(canonical JSON) 的十六进制。 */
  signature: string;
}

/** 单个权限的 scope 变化。 */
export interface ScopeChange {
  permission: string;
  oldScope?: JsonValue;
  newScope?: JsonValue;
  /** `true` = scope 变宽（需要重新审批）。 */
  grew: boolean;
}

/** diff 结果。 */
export interface GrantDiff {
  added: string[];
  removed: string[];
  unchanged: string[];
  scopeChanges: ScopeChange[];
  /** 信息性：不单独触发重审。 */
  frameworkChanged: boolean;
  pluginVersionChanged: boolean;
  requiresReapproval: boolean;
}

/**
 * 审批 UI 的一行——**宿主 `tauron_acl::build_approval_rows` 输出的线上协议镜像**（轮 56）。
 *
 * 字段名与 Rust 的 camelCase 序列化逐字一致，消费方是壳层安装流
 * （`shell-controller.ts` 的 `_installPlugin`）：它按 `defaultChecked` 决定问法是
 * 「是否授予」还是「逐字输入 `confirmationHint`」，本文件不参与判定、也不得重述规则。
 */
export interface ApprovalRow {
  permission: string;
  risk: Risk;
  /** 人话描述：宿主权限词表的 `description`，不是本地另拟的文案。 */
  description: string;
  /** 默认勾态由宿主按 §4.5 判定（高危档为 `false`）；壳层照办，不自行按 risk 重算。 */
  defaultChecked: boolean;
  scope?: string;
  /** 高危行的确认词，**唯一来源是宿主**；缺失即视为宿主不合规，不代为拟定。 */
  confirmationHint?: string;
}

/** 物化出的 Tauri capability 形状（`add_capability` 的入参）。 */
export interface TauriCapability {
  identifier: string;
  webviews: string[];
  permissions: string[];
}

/** 插件身份 webview 的 label 前缀，与 Rust `IDENTITY_LABEL_PREFIX` 一致。 */
export const IDENTITY_LABEL_PREFIX = 'plugin-';

export function capabilityOfGrantSet(grantSet: GrantSet): TauriCapability {
  return {
    identifier: `${IDENTITY_LABEL_PREFIX}${grantSet.pluginId}`,
    webviews: [`${IDENTITY_LABEL_PREFIX}${grantSet.pluginId}`],
    permissions: grantSet.grants.map((g) => g.permission),
  };
}

/**
 * 判定一条 scope 是否变宽（含"从无到有"）。
 *
 * 与 Rust `tauron_acl::diff::scope_grew` 同语义：
 * - 新集合条目多于旧集合 → 变宽；
 * - 新集合含旧集合里没有的条目 → 变宽；
 * - 旧集合无 scope、新集合有 → 变宽；
 * - 仅顺序不同或严格子集 → 不变宽。
 */
export function scopeGrew(
  oldScope: JsonValue | undefined,
  newScope: JsonValue | undefined,
): boolean {
  const toList = (v: JsonValue | undefined): string[] | null => {
    if (Array.isArray(v)) return v.map((x) => (typeof x === 'string' ? x : JSON.stringify(x)));
    if (typeof v === 'string') return [v];
    return null;
  };

  const n = toList(newScope);
  if (n === null) return false;
  const o = toList(oldScope);
  if (o === null) return true;
  if (n.length > o.length) return true;
  return n.some((s) => !o.includes(s));
}

/**
 * 判定一次授予集变更是否需要重新审批。
 *
 * 规则（计划 §4.5）：**权限或 scope 变多必须重新审批**；
 * 收缩（删权限、缩 scope）如实撤销，不需要重新审批（ADR-05）。
 */
export function requiresReapproval(diff: GrantDiff): boolean {
  return diff.added.length > 0 || diff.scopeChanges.some((c) => c.grew);
}
