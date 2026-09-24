import { describe, it, expect } from 'vitest';
import { DEFAULT_SANDBOX_CAPABILITIES } from './types.js';

describe('DEFAULT_SANDBOX_CAPABILITIES', () => {
  it('has minimal permissions by default', () => {
    expect(DEFAULT_SANDBOX_CAPABILITIES.fs).toBe(false);
    expect(DEFAULT_SANDBOX_CAPABILITIES.network).toBe(false);
    expect(DEFAULT_SANDBOX_CAPABILITIES.dom).toBe(false);
    expect(DEFAULT_SANDBOX_CAPABILITIES.workers).toBe(false);
    expect(DEFAULT_SANDBOX_CAPABILITIES.wasm).toBe(false);
  });

  it('allows timers by default', () => {
    expect(DEFAULT_SANDBOX_CAPABILITIES.timers).toBe(true);
  });

  it('is a complete capability set', () => {
    expect(Object.keys(DEFAULT_SANDBOX_CAPABILITIES).length).toBe(6);
  });
});
