import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  isTauri,
  getRuntimeMode,
  getCapabilities,
  isCapabilityAvailable,
  TAURI_CAPABILITIES,
  WEB_CAPABILITIES,
} from './runtime.js';

describe('runtime', () => {
  const originalWindow = globalThis.window;

  beforeEach(() => {
    vi.restoreAllMocks();
  });

  describe('isTauri', () => {
    it('returns false in non-Tauri environment', () => {
      // Default: no __TAURI_INTERNALS__
      delete (globalThis as any).window;
      expect(isTauri()).toBe(false);
    });

    it('returns true when __TAURI_INTERNALS__ is present', () => {
      (globalThis as any).window = { __TAURI_INTERNALS__: {} };
      expect(isTauri()).toBe(true);
    });

    it('returns false when window is undefined', () => {
      delete (globalThis as any).window;
      expect(isTauri()).toBe(false);
    });
  });

  describe('getRuntimeMode', () => {
    it('returns "web" in non-Tauri environment', () => {
      delete (globalThis as any).window;
      expect(getRuntimeMode()).toBe('web');
    });

    it('returns "tauri" in Tauri environment', () => {
      (globalThis as any).window = { __TAURI_INTERNALS__: {} };
      expect(getRuntimeMode()).toBe('tauri');
    });
  });

  describe('getCapabilities', () => {
    it('returns Tauri capabilities in Tauri environment', () => {
      (globalThis as any).window = { __TAURI_INTERNALS__: {} };
      const caps = getCapabilities();
      expect(caps).toEqual(TAURI_CAPABILITIES);
    });

    it('returns Web capabilities in non-Tauri environment', () => {
      delete (globalThis as any).window;
      const caps = getCapabilities();
      expect(caps).toEqual(WEB_CAPABILITIES);
    });

    it('returns independent copies (mutations do not affect original)', () => {
      (globalThis as any).window = { __TAURI_INTERNALS__: {} };
      const caps1 = getCapabilities();
      const caps2 = getCapabilities();
      caps1.store = false;
      expect(caps2.store).toBe(true);
      expect(TAURI_CAPABILITIES.store).toBe(true);
    });
  });

  describe('isCapabilityAvailable', () => {
    it('returns true for available capabilities', () => {
      (globalThis as any).window = { __TAURI_INTERNALS__: {} };
      expect(isCapabilityAvailable('store')).toBe(true);
      expect(isCapabilityAvailable('http')).toBe(true);
    });

    it('returns false for unavailable capabilities in web mode', () => {
      delete (globalThis as any).window;
      expect(isCapabilityAvailable('store')).toBe(true); // store is available in web
      expect(isCapabilityAvailable('tray')).toBe(false);
      expect(isCapabilityAvailable('updater')).toBe(false);
    });
  });
});
