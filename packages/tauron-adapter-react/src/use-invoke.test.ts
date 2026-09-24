import { describe, it, expect } from 'vitest';
import { useInvoke } from './use-invoke.js';

describe('useInvoke', () => {
  it('is a function', () => {
    expect(typeof useInvoke).toBe('function');
  });
});
