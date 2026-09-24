import { describe, it, expect, beforeEach } from 'vitest';
import { invokePlugin, cancelPlugin, listenEvent, emitEvent } from './invoke.js';
import { MockBackend, createTestBackend } from './testing.js';
import { PluginErrorCode, type PluginInvokeResponse } from '@tauron/types';

describe('invoke', () => {
  let backend: MockBackend;

  beforeEach(() => {
    backend = createTestBackend();
  });

  describe('invokePlugin', () => {
    it('calls plugin method and returns response', async () => {
      const response = await invokePlugin(backend, 'com.example.test', 'format', { code: 'let x = 1;' });

      expect(response.ok).toBe(true);
      expect(response.callId).toBeDefined();
      expect(backend.invocations.length).toBe(1);
      expect(backend.invocations[0]?.request.pluginId).toBe('com.example.test');
      expect(backend.invocations[0]?.request.method).toBe('format');
      expect(backend.invocations[0]?.request.payload).toEqual({ code: 'let x = 1;' });
    });

    it('passes timeoutMs option', async () => {
      await invokePlugin(backend, 'com.example.test', 'format', undefined, { timeoutMs: 5000 });
      expect(backend.invocations[0]?.request.timeoutMs).toBe(5000);
    });

    it('returns error response when backend throws', async () => {
      const errorBackend = new MockBackend({
        onInvoke: async () => {
          throw new Error('Connection refused');
        },
      });

      const response = await invokePlugin(errorBackend, 'com.example.test', 'format');

      expect(response.ok).toBe(false);
      expect(response.error?.code).toBe(PluginErrorCode.CHANNEL_BROKEN);
      expect(response.error?.message).toContain('Connection refused');
    });
  });

  describe('cancelPlugin', () => {
    it('calls backend.cancel', async () => {
      await cancelPlugin(backend, 'call-123');
      // Mock: no-op, but should not throw
    });
  });

  describe('listenEvent', () => {
    it('subscribes to events and receives them', async () => {
      const received: unknown[] = [];
      const unlisten = await listenEvent(backend, 'plugin:test:done', (payload) => {
        received.push(payload);
      });

      await emitEvent(backend, 'plugin:test:done', { result: 'done' });
      expect(received.length).toBe(1);

      // Unsubscribe
      await unlisten();
      await emitEvent(backend, 'plugin:test:done', { result: 'done2' });
      expect(received.length).toBe(1);
    });
  });

  describe('emitEvent', () => {
    it('emits event to subscribers', async () => {
      const received: unknown[] = [];
      await listenEvent(backend, 'test-topic', (payload) => {
        received.push(payload);
      });

      await emitEvent(backend, 'test-topic', { data: 42 });
      expect(received).toEqual([{ data: 42 }]);
    });
  });
});
