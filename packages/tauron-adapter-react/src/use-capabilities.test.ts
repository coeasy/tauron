import { describe, it, expect, vi } from 'vitest';
import { useCapabilities, useIsTauri } from './use-capabilities.js';
import { getCapabilities, isTauri, TAURI_CAPABILITIES, WEB_CAPABILITIES } from '@tauron/core';

describe('useCapabilities', () => {
  it('is a function', () => {
    expect(typeof useCapabilities).toBe('function');
  });

  it('exports getCapabilities', () => {
    expect(typeof getCapabilities).toBe('function');
  });

  it('exports isTauri', () => {
    expect(typeof isTauri).toBe('function');
  });

  it('exports TAURI_CAPABILITIES', () => {
    expect(TAURI_CAPABILITIES).toBeDefined();
    expect(typeof TAURI_CAPABILITIES.store).toBe('boolean');
  });

  it('exports WEB_CAPABILITIES', () => {
    expect(WEB_CAPABILITIES).toBeDefined();
    expect(typeof WEB_CAPABILITIES.tray).toBe('boolean');
  });
});

describe('useIsTauri', () => {
  it('is a function', () => {
    expect(typeof useIsTauri).toBe('function');
  });
});
