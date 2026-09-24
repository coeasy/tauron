import { describe, it, expect } from 'vitest';
import { createCapabilitiesStore, createIsTauriStore } from './use-capabilities.js';

describe('createCapabilitiesStore', () => {
  it('is a function', () => {
    expect(typeof createCapabilitiesStore).toBe('function');
  });
});

describe('createIsTauriStore', () => {
  it('is a function', () => {
    expect(typeof createIsTauriStore).toBe('function');
  });
});
