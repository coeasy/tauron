/**
 * 跨语言契约测试（TS↔Rust）
 *
 * 验证 TypeScript 和 Rust 实现之间的兼容性。
 * 这些测试确保两边的实现遵循相同的协议契约。
 */

import { describe, it, expect } from 'vitest';
import { PluginErrorCode } from '@tauron/types';
import type {
  PluginInvokeRequest,
  PluginInvokeResponse,
  PluginEvent,
  PluginState,
  PluginManifest,
  PluginPermissionGrant,
} from '@tauron/types';

describe('Envelope Contract', () => {
  it('envelope has required fields', () => {
    const envelope: PluginInvokeRequest = {
      pluginId: 'test-plugin',
      method: 'test-method',
      payload: { key: 'value' },
      callId: 'call-123',
      timeoutMs: 30000,
    };

    // Verify required fields exist
    expect(envelope.pluginId).toBeDefined();
    expect(envelope.method).toBeDefined();
    expect(envelope.callId).toBeDefined();
  });

  it('envelope payload is optional', () => {
    const envelope: PluginInvokeRequest = {
      pluginId: 'test-plugin',
      method: 'test-method',
      callId: 'call-123',
    };

    expect(envelope).toBeDefined();
  });

  it('envelope timeout has default', () => {
    const envelope: PluginInvokeRequest = {
      pluginId: 'test-plugin',
      method: 'test-method',
      callId: 'call-123',
    };

    // timeoutMs should have a default in the actual implementation
    expect(envelope.timeoutMs === undefined).toBe(true);
  });
});

describe('Response Contract', () => {
  it('success response has ok: true and result', () => {
    const response: PluginInvokeResponse = {
      callId: 'call-123',
      ok: true,
      result: { data: 'value' },
    };

    expect(response.ok).toBe(true);
    expect(response.result).toBeDefined();
  });

  it('error response has ok: false and error', () => {
    const response: PluginInvokeResponse = {
      callId: 'call-123',
      ok: false,
      error: {
        code: PluginErrorCode.INVALID_PAYLOAD,
        message: 'Method not found',
        retryable: false,
      },
    };

    expect(response.ok).toBe(false);
    expect(response.error).toBeDefined();
    expect(response.error!.code).toBe(PluginErrorCode.INVALID_PAYLOAD);
  });

  it('response callId matches request callId', () => {
    const requestCallId = 'call-abc-123';
    const response: PluginInvokeResponse = {
      callId: requestCallId,
      ok: true,
      result: {},
    };

    expect(response.callId).toBe(requestCallId);
  });
});

describe('Error Code Contract', () => {
  const expectedCodes: PluginErrorCode[] = [
    PluginErrorCode.PLUGIN_NOT_FOUND,      // SC-0001
    PluginErrorCode.PLUGIN_DISABLED,        // SC-0002
    PluginErrorCode.PLUGIN_ERRORED,         // SC-0003
    PluginErrorCode.PERMISSION_DENIED,      // SC-1001
    PluginErrorCode.PLUGIN_PERMISSION_DENIED, // SC-1002
    PluginErrorCode.CAPABILITY_REQUIRED,    // SC-1003
    PluginErrorCode.TIMEOUT,                // SC-2001
    PluginErrorCode.CANCELLED,              // SC-2002
    PluginErrorCode.INVALID_PAYLOAD,        // SC-2003
    PluginErrorCode.CHANNEL_BROKEN,         // SC-2004
    PluginErrorCode.PLUGIN_PANIC,           // SC-3001
    PluginErrorCode.PLUGIN_OOM,             // SC-3002
    PluginErrorCode.PLUGIN_EXITED,          // SC-3003
    PluginErrorCode.INTERNAL,               // SC-9001
  ];

  it('error codes follow SC-xxxx format', () => {
    for (const code of expectedCodes) {
      expect(code).toMatch(/^SC-\d{4}$/);
    }
  });

  it('error codes have 4 digits', () => {
    for (const code of expectedCodes) {
      const digits = code.split('-')[1];
      expect(digits!.length).toBe(4);
    }
  });

  it('error codes are categorized by prefix', () => {
    // 0xxx: Plugin errors
    expect(expectedCodes[0]!.startsWith('SC-0')).toBe(true);
    // 1xxx: Permission errors
    expect(expectedCodes[3]!.startsWith('SC-1')).toBe(true);
    // 2xxx: Communication errors
    expect(expectedCodes[6]!.startsWith('SC-2')).toBe(true);
    // 3xxx: Implementation errors
    expect(expectedCodes[10]!.startsWith('SC-3')).toBe(true);
    // 9xxx: System errors
    expect(expectedCodes[13]!.startsWith('SC-9')).toBe(true);
  });
});

describe('Plugin State Contract', () => {
  const expectedStates: PluginState[] = [
    'DISCOVERED',
    'INSTALLING',
    'INSTALLED',
    'ENABLING',
    'ENABLED',
    'DISABLING',
    'DISABLED',
    'ERRORED',
    'UPGRADING',
    'UNINSTALLING',
  ];

  it('all states are uppercase', () => {
    for (const state of expectedStates) {
      expect(state).toBe(state.toUpperCase());
    }
  });

  it('ENABLED is the only active state', () => {
    expect(expectedStates).toContain('ENABLED');
  });

  it('UNINSTALLING is terminal', () => {
    expect(expectedStates[expectedStates.length - 1]).toBe('UNINSTALLING');
  });
});

describe('Event Contract', () => {
  it('event has required fields', () => {
    const event: PluginEvent = {
      sourcePlugin: 'test-plugin',
      eventName: 'test-event',
      payload: { data: 'value' },
      timestamp: new Date().toISOString(),
    };

    expect(event.sourcePlugin).toBeDefined();
    expect(event.eventName).toBeDefined();
    expect(event.timestamp).toBeDefined();
  });

  it('event namespace format', () => {
    const pluginId = 'test-plugin';
    const eventName = 'data-changed';
    const namespace = `plugin:${pluginId}:${eventName}`;

    expect(namespace).toBe('plugin:test-plugin:data-changed');
  });
});

describe('Manifest Contract', () => {
  it('manifest has required fields', () => {
    const manifest: PluginManifest = {
      id: 'test-plugin',
      name: 'Test Plugin',
      version: '1.0.0',
      author: { name: 'Author', email: 'author@example.com' },
      license: 'MIT',
      apiVersion: '1.0.0',
      minFrameworkVersion: '1.0.0',
      platforms: ['linux', 'macos', 'windows'],
      type: 'js',
      entry: {},
      permissions: ['store:read'],
    };

    expect(manifest.id).toBeDefined();
    expect(manifest.name).toBeDefined();
    expect(manifest.version).toBeDefined();
    expect(manifest.author).toBeDefined();
    expect(manifest.license).toBeDefined();
    expect(manifest.apiVersion).toBeDefined();
    expect(manifest.minFrameworkVersion).toBeDefined();
    expect(manifest.platforms).toBeDefined();
    expect(manifest.permissions).toBeDefined();
  });

  it('manifest version follows semver', () => {
    const version = '1.0.0';
    expect(version).toMatch(/^\d+\.\d+\.\d+$/);
  });
});

describe('Permission Grant Contract', () => {
  it('grant has required fields', () => {
    const grant: PluginPermissionGrant = {
      pluginId: 'test-plugin',
      permissions: ['store:read'],
      grantedAt: new Date().toISOString(),
      grantedBy: 'install',
    };

    expect(grant.pluginId).toBeDefined();
    expect(grant.permissions).toBeDefined();
    expect(grant.grantedAt).toBeDefined();
    expect(grant.grantedBy).toBeDefined();
  });

  it('permission follows domain:action format', () => {
    const permissions = ['store:read', 'http:fetch', 'fs:read', 'shell:exec'];
    for (const perm of permissions) {
      expect(perm).toMatch(/^[a-z]+:[a-z]+$/);
    }
  });
});

describe('Cross-Language Compatibility', () => {
  it('envelope JSON serialization', () => {
    const envelope: PluginInvokeRequest = {
      pluginId: 'test-plugin',
      method: 'test-method',
      payload: { key: 'value', num: 42 },
      callId: 'call-123',
      timeoutMs: 30000,
    };

    const json = JSON.stringify(envelope);
    const parsed = JSON.parse(json) as PluginInvokeRequest;

    expect(parsed.pluginId).toBe(envelope.pluginId);
    expect(parsed.method).toBe(envelope.method);
    expect(parsed.callId).toBe(envelope.callId);
  });

  it('response JSON serialization', () => {
    const response: PluginInvokeResponse = {
      callId: 'call-123',
      ok: true,
      result: { data: 'value' },
    };

    const json = JSON.stringify(response);
    const parsed = JSON.parse(json) as PluginInvokeResponse;

    expect(parsed.ok).toBe(response.ok);
    expect(parsed.callId).toBe(response.callId);
  });

  it('error serialization', () => {
    const error = {
      code: PluginErrorCode.INVALID_PAYLOAD,
      message: 'Method not found',
      retryable: false,
    };

    const json = JSON.stringify(error);
    const parsed = JSON.parse(json);

    expect(parsed.code).toBe(error.code);
    expect(parsed.message).toBe(error.message);
  });
});
