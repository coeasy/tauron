import { describe, it, expect } from 'vitest';
import { usePluginId } from './use-plugin-id.js';

describe('usePluginId', () => {
  it('is a function', () => {
    expect(typeof usePluginId).toBe('function');
  });

  it('exports proper interface', () => {
    expect(usePluginId).toBeDefined();
  });
});
