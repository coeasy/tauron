import { describe, it, expect } from 'vitest';
import { createInvokeStore } from './use-invoke.js';

describe('createInvokeStore', () => {
  it('is a function', () => {
    expect(typeof createInvokeStore).toBe('function');
  });

  it('exports proper interface', () => {
    expect(createInvokeStore).toBeDefined();
  });
});
