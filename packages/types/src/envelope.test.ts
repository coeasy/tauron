import { describe, it, expect } from 'vitest';
import {
  generateCallId,
  buildRequest,
  buildOkResponse,
  buildErrorResponse,
  validateRequest,
  DEFAULT_TIMEOUT_MS,
  type PluginInvokeRequest,
  type PluginInvokeResponse,
} from './envelope.js';
import { PluginErrorCode } from './errors.js';

describe('envelope', () => {
  describe('generateCallId', () => {
    it('generates a valid UUID v4', () => {
      const id = generateCallId();
      expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    });

    it('generates unique IDs', () => {
      const ids = new Set(Array.from({ length: 1000 }, () => generateCallId()));
      expect(ids.size).toBe(1000);
    });
  });

  describe('buildRequest', () => {
    it('creates a request with auto-generated callId', () => {
      const req = buildRequest({ pluginId: 'com.example.test', method: 'format' });
      expect(req.pluginId).toBe('com.example.test');
      expect(req.method).toBe('format');
      expect(req.callId).toBeDefined();
      expect(req.payload).toBeUndefined();
      expect(req.timeoutMs).toBeUndefined();
    });

    it('includes payload when provided', () => {
      const payload = { code: 'let x = 1;', language: 'ts' };
      const req = buildRequest({ pluginId: 'com.example.test', method: 'format', payload });
      expect(req.payload).toEqual(payload);
    });

    it('includes timeoutMs when provided', () => {
      const req = buildRequest({ pluginId: 'com.example.test', method: 'format', timeoutMs: 5000 });
      expect(req.timeoutMs).toBe(5000);
    });
  });

  describe('buildOkResponse', () => {
    it('creates a successful response', () => {
      const res = buildOkResponse('call-123', { formatted: 'let x: number = 1;' });
      expect(res.ok).toBe(true);
      expect(res.callId).toBe('call-123');
      expect(res.result).toEqual({ formatted: 'let x: number = 1;' });
      expect(res.error).toBeUndefined();
    });
  });

  describe('buildErrorResponse', () => {
    it('creates an error response', () => {
      const res = buildErrorResponse('call-123', PluginErrorCode.TIMEOUT, 'Call timed out', true);
      expect(res.ok).toBe(false);
      expect(res.callId).toBe('call-123');
      expect(res.result).toBeUndefined();
      expect(res.error).toEqual({
        code: PluginErrorCode.TIMEOUT,
        message: 'Call timed out',
        retryable: true,
      });
    });
  });

  describe('validateRequest', () => {
    it('returns null for valid request', () => {
      const req: PluginInvokeRequest = {
        pluginId: 'com.example.test',
        method: 'format',
        callId: 'call-123',
      };
      expect(validateRequest(req)).toBeNull();
    });

    it('rejects missing pluginId', () => {
      const req = { pluginId: '', method: 'format', callId: 'call-123' };
      expect(validateRequest(req as PluginInvokeRequest)).toBe('pluginId is required');
    });

    it('rejects missing method', () => {
      const req = { pluginId: 'com.example.test', method: '', callId: 'call-123' };
      expect(validateRequest(req as PluginInvokeRequest)).toBe('method is required');
    });

    it('rejects missing callId', () => {
      const req = { pluginId: 'com.example.test', method: 'format', callId: '' };
      expect(validateRequest(req as PluginInvokeRequest)).toBe('callId is required');
    });

    it('rejects negative timeoutMs', () => {
      const req = { pluginId: 'com.example.test', method: 'format', callId: 'call-123', timeoutMs: -1 };
      expect(validateRequest(req as PluginInvokeRequest)).toBe('timeoutMs must be a non-negative number');
    });

    it('allows zero timeoutMs (no timeout)', () => {
      const req = { pluginId: 'com.example.test', method: 'format', callId: 'call-123', timeoutMs: 0 };
      expect(validateRequest(req as PluginInvokeRequest)).toBeNull();
    });
  });

  it('exports DEFAULT_TIMEOUT_MS as 30000', () => {
    expect(DEFAULT_TIMEOUT_MS).toBe(30000);
  });
});
