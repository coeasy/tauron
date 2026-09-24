import { describe, it, expect } from 'vitest';
import {
  BRIDGE_MESSAGE_TYPE,
  buildInitMessage,
  buildReadyMessage,
  buildInvokeMessage,
  buildInvokeResultMessage,
  buildEmitMessage,
  buildCancelMessage,
  isHostToPlugin,
  isPluginToHost,
  type BridgeMessage,
} from './bridge.js';

describe('postMessage bridge protocol', () => {
  it('exports BRIDGE_MESSAGE_TYPE as tauron:bridge', () => {
    expect(BRIDGE_MESSAGE_TYPE).toBe('tauron:bridge');
  });

  describe('buildInitMessage', () => {
    it('creates host-to-plugin init message', () => {
      const msg = buildInitMessage('token-123', ['store:read', 'http:fetch']);
      expect(msg.type).toBe('tauron:bridge');
      expect(msg.direction).toBe('host-to-plugin');
      expect(msg.token).toBe('token-123');
      expect(msg.action).toBe('init');
      expect(msg.payload).toEqual({ permissions: ['store:read', 'http:fetch'] });
      expect(isHostToPlugin(msg)).toBe(true);
      expect(isPluginToHost(msg)).toBe(false);
    });
  });

  describe('buildReadyMessage', () => {
    it('creates plugin-to-host ready message', () => {
      const msg = buildReadyMessage('token-123');
      expect(msg.type).toBe('tauron:bridge');
      expect(msg.direction).toBe('plugin-to-host');
      expect(msg.action).toBe('ready');
      expect(msg.payload).toEqual({ token: 'token-123' });
      expect(isHostToPlugin(msg)).toBe(false);
      expect(isPluginToHost(msg)).toBe(true);
    });
  });

  describe('buildInvokeMessage', () => {
    it('creates plugin-to-host invoke message', () => {
      const msg = buildInvokeMessage('call-123', 'format', { code: 'let x = 1;' });
      expect(msg.type).toBe('tauron:bridge');
      expect(msg.direction).toBe('plugin-to-host');
      expect(msg.action).toBe('invoke');
      expect(msg.callId).toBe('call-123');
      expect(msg.payload).toEqual({ method: 'format', args: { code: 'let x = 1;' } });
    });
  });

  describe('buildInvokeResultMessage', () => {
    it('creates host-to-plugin invoke-result message', () => {
      const msg = buildInvokeResultMessage('token-123', 'call-123', { formatted: 'let x: number = 1;' });
      expect(msg.type).toBe('tauron:bridge');
      expect(msg.direction).toBe('host-to-plugin');
      expect(msg.token).toBe('token-123');
      expect(msg.action).toBe('invoke-result');
      expect(msg.payload).toEqual({
        callId: 'call-123',
        result: { formatted: 'let x: number = 1;' },
      });
    });
  });

  describe('buildEmitMessage', () => {
    it('creates plugin-to-host emit message', () => {
      const msg = buildEmitMessage('format-done', { callId: 'call-123', result: 'done' });
      expect(msg.type).toBe('tauron:bridge');
      expect(msg.direction).toBe('plugin-to-host');
      expect(msg.action).toBe('emit');
      expect(msg.payload).toEqual({
        eventName: 'format-done',
        payload: { callId: 'call-123', result: 'done' },
      });
    });
  });

  describe('buildCancelMessage', () => {
    it('creates host-to-plugin cancel message', () => {
      const msg = buildCancelMessage('token-123', 'call-456');
      expect(msg.type).toBe('tauron:bridge');
      expect(msg.direction).toBe('host-to-plugin');
      expect(msg.token).toBe('token-123');
      expect(msg.action).toBe('cancel');
      expect(msg.payload).toEqual({ callId: 'call-456' });
    });
  });

  describe('type guards', () => {
    it('isHostToPlugin returns true for host-to-plugin messages', () => {
      const msg: BridgeMessage = {
        type: 'tauron:bridge',
        direction: 'host-to-plugin',
        token: 't',
        action: 'init',
        payload: {},
      };
      expect(isHostToPlugin(msg)).toBe(true);
    });

    it('isPluginToHost returns true for plugin-to-host messages', () => {
      const msg: BridgeMessage = {
        type: 'tauron:bridge',
        direction: 'plugin-to-host',
        action: 'ready',
        payload: {},
      };
      expect(isPluginToHost(msg)).toBe(true);
    });
  });
});
