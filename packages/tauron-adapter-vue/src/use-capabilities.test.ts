import { describe, it, expect } from 'vitest';
import { useCapabilities, useIsTauri } from './use-capabilities.js';

describe('useCapabilities', () => {
  it('is a function', () => {
    expect(typeof useCapabilities).toBe('function');
  });
});

describe('useIsTauri', () => {
  it('is a function', () => {
    expect(typeof useIsTauri).toBe('function');
  });
});
