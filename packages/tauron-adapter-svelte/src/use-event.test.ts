import { describe, it, expect } from 'vitest';
import { createEventStore } from './use-event.js';

describe('createEventStore', () => {
  it('is a function', () => {
    expect(typeof createEventStore).toBe('function');
  });

  it('exports proper interface', () => {
    expect(createEventStore).toBeDefined();
  });
});
