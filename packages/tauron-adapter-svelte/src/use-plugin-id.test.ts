import { describe, it, expect } from 'vitest';
import { createPluginIdStore } from './use-plugin-id.js';

describe('createPluginIdStore', () => {
  it('is a function', () => {
    expect(typeof createPluginIdStore).toBe('function');
  });
});
