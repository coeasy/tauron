import { describe, it, expect } from 'vitest';
import { useEvent } from './use-event.js';

describe('useEvent', () => {
  it('is a function', () => {
    expect(typeof useEvent).toBe('function');
  });

  it('exports proper interface', () => {
    expect(useEvent).toBeDefined();
  });
});
