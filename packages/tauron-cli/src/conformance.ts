/**
 * V4 Host Conformance Kit.
 *
 * The adapter exposes operations, not a single "passed" bit. This runner owns the required
 * scenario list, terminal-state requirements and resource-leak comparison.
 */

import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

export const HOST_CONFORMANCE_SCENARIOS = [
  'invoke',
  'stream',
  'event',
  'cancel',
  'settings',
  'permission',
  'lifecycle',
  'recovery',
  'shutdown',
] as const;

export type HostConformanceScenario = (typeof HOST_CONFORMANCE_SCENARIOS)[number];

export interface HostConformanceMetadata {
  host: string;
  protocolVersion: string;
  wireVersion: number;
  principal: { kind: 'plugin' | 'main-window'; id?: string };
}

export interface HostCapabilitySnapshot {
  families: string[];
  commands: string[];
  unsupported: Array<{ domain: string; reason: string }>;
  enforcement: Array<{ domain: string; level: 'hard' | 'partial' | 'unsupported'; detail: string }>;
  pluginRuntime: boolean;
  target?: unknown;
}

export interface HostResourceSnapshot {
  pendingCalls: number;
  streams: number;
  subscriptions: number;
  channels: number;
  childProcesses: number;
}

export interface HostScenarioEvidence {
  terminal: boolean;
  cleaned: boolean;
  observations?: Record<string, unknown>;
}

export interface HostConformanceAdapter {
  connect(): Promise<void>;
  disconnect(): Promise<void>;
  metadata(): Promise<HostConformanceMetadata>;
  invoke<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T>;
  resources(): Promise<HostResourceSnapshot>;
  exercise(scenario: HostConformanceScenario): Promise<HostScenarioEvidence>;
}

export interface ConformanceCheck {
  name: string;
  status: 'pass' | 'fail';
  message: string;
}

export interface HostConformanceReport {
  host: string;
  passed: boolean;
  checks: ConformanceCheck[];
  before: HostResourceSnapshot;
  after: HostResourceSnapshot;
}

function checkResources(snapshot: HostResourceSnapshot): string | null {
  for (const [name, value] of Object.entries(snapshot)) {
    if (!Number.isSafeInteger(value) || value < 0) return `${name} must be a non-negative integer`;
  }
  return null;
}

function resourceDelta(
  before: HostResourceSnapshot,
  after: HostResourceSnapshot,
): Array<{ name: keyof HostResourceSnapshot; before: number; after: number }> {
  const names: Array<keyof HostResourceSnapshot> = [
    'pendingCalls',
    'streams',
    'subscriptions',
    'channels',
    'childProcesses',
  ];
  return names
    .filter((name) => after[name] > before[name])
    .map((name) => ({ name, before: before[name], after: after[name] }));
}

export async function runHostConformance(
  adapter: HostConformanceAdapter,
): Promise<HostConformanceReport> {
  const checks: ConformanceCheck[] = [];
  let connected = false;
  let host = 'unknown';
  let before: HostResourceSnapshot = {
    pendingCalls: 0,
    streams: 0,
    subscriptions: 0,
    channels: 0,
    childProcesses: 0,
  };
  let after = before;

  const pass = (name: string, message: string): void => {
    checks.push({ name, status: 'pass', message });
  };
  const fail = (name: string, message: string): void => {
    checks.push({ name, status: 'fail', message });
  };

  try {
    await adapter.connect();
    connected = true;
    pass('connect', 'host transport connected');

    const metadata = await adapter.metadata();
    host = metadata.host;
    if (metadata.protocolVersion.trim() === '') {
      fail('protocol', 'protocolVersion is empty');
    } else if (metadata.wireVersion !== 1) {
      fail('protocol', `wireVersion ${metadata.wireVersion} is incompatible with V4 wire v1`);
    } else {
      pass('protocol', `${metadata.protocolVersion} / wire-v${metadata.wireVersion}`);
    }

    if (metadata.principal.kind === 'plugin' && !metadata.principal.id?.trim()) {
      fail('identity', 'plugin principal has no stable id');
    } else {
      pass('identity', `principal=${metadata.principal.kind}`);
    }

    const capabilities = await adapter.invoke<HostCapabilitySnapshot>('host_capabilities');
    if (!Array.isArray(capabilities.commands) || !capabilities.commands.includes('host_capabilities')) {
      fail('capabilities', 'host_capabilities is missing from its own command surface');
    } else if (!Array.isArray(capabilities.enforcement)) {
      fail('capabilities', 'V4 enforcement facts are missing');
    } else {
      const invalid = capabilities.enforcement.find(
        (item) => !['hard', 'partial', 'unsupported'].includes(item.level),
      );
      if (invalid) {
        fail('capabilities', `invalid enforcement level for ${invalid.domain}: ${invalid.level}`);
      } else {
        pass(
          'capabilities',
          `${capabilities.commands.length} commands, ${capabilities.enforcement.length} enforcement facts`,
        );
      }
    }

    before = await adapter.resources();
    const invalidBefore = checkResources(before);
    if (invalidBefore) fail('resource-baseline', invalidBefore);
    else pass('resource-baseline', 'resource counters are valid');

    for (const scenario of HOST_CONFORMANCE_SCENARIOS) {
      try {
        const evidence = await adapter.exercise(scenario);
        if (!evidence.terminal) {
          fail(scenario, 'scenario did not reach a terminal state');
        } else if (!evidence.cleaned) {
          fail(scenario, 'scenario reached terminal state but cleanup was not observed');
        } else {
          pass(scenario, 'terminal state and cleanup observed');
        }
      } catch (error) {
        fail(scenario, error instanceof Error ? error.message : String(error));
      }
    }

    after = await adapter.resources();
    const invalidAfter = checkResources(after);
    if (invalidAfter) {
      fail('resource-cleanup', invalidAfter);
    } else {
      const leaks = resourceDelta(before, after);
      if (leaks.length > 0) {
        fail(
          'resource-cleanup',
          `resource growth after conformance: ${leaks
            .map((item) => `${item.name} ${item.before}->${item.after}`)
            .join(', ')}`,
        );
      } else {
        pass('resource-cleanup', 'no tracked resource counter increased');
      }
    }
  } catch (error) {
    fail('harness', error instanceof Error ? error.message : String(error));
  } finally {
    if (connected) {
      try {
        await adapter.disconnect();
        pass('disconnect', 'host transport disconnected');
      } catch (error) {
        fail('disconnect', error instanceof Error ? error.message : String(error));
      }
    }
  }

  return { host, passed: checks.every((check) => check.status === 'pass'), checks, before, after };
}

export function formatHostConformanceReport(report: HostConformanceReport): string {
  const lines = [`tauron conform host — ${report.host}`, '========================================', ''];
  for (const check of report.checks) {
    lines.push(`${check.status === 'pass' ? '✓' : '✗'} ${check.name}: ${check.message}`);
  }
  lines.push('');
  lines.push(report.passed ? '✓ Host conformance passed.' : '✗ Host conformance failed.');
  return lines.join('\n');
}

function isAdapter(value: unknown): value is HostConformanceAdapter {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as Partial<Record<keyof HostConformanceAdapter, unknown>>;
  return (
    typeof candidate.connect === 'function' &&
    typeof candidate.disconnect === 'function' &&
    typeof candidate.metadata === 'function' &&
    typeof candidate.invoke === 'function' &&
    typeof candidate.resources === 'function' &&
    typeof candidate.exercise === 'function'
  );
}

export async function loadHostConformanceAdapter(
  modulePath: string,
  cwd = process.cwd(),
): Promise<HostConformanceAdapter> {
  const absolute = resolve(cwd, modulePath);
  const loaded = (await import(pathToFileURL(absolute).href)) as Record<string, unknown>;
  const exported =
    loaded.createTauronHostConformanceAdapter ??
    loaded.hostConformanceAdapter ??
    loaded.default;
  const candidate =
    typeof exported === 'function'
      ? await (exported as () => unknown)()
      : exported;
  if (!isAdapter(candidate)) {
    throw new Error(
      'Conformance module must export createTauronHostConformanceAdapter(), hostConformanceAdapter, or a default HostConformanceAdapter.',
    );
  }
  return candidate;
}
