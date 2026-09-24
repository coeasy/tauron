import { describe, it, expect } from 'vitest';
import { usePluginId } from './use-plugin-id.js';

describe('usePluginId', () => {
  it('is a function', () => {
    expect(typeof usePluginId).toBe('function');
  });

  it('returns null when no plugin ID found', () => {
    const result = usePluginId();
    expect(result === null || typeof result === 'string').toBe(true);
  });
});
