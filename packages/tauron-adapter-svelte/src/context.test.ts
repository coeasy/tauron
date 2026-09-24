import { describe, it, expect } from 'vitest';
import { setTauronContext, getTauronContext, clearTauronContext } from './context.js';

describe('context', () => {
  it('exports setTauronContext', () => {
    expect(typeof setTauronContext).toBe('function');
  });

  it('exports getTauronContext', () => {
    expect(typeof getTauronContext).toBe('function');
  });

  it('exports clearTauronContext', () => {
    expect(typeof clearTauronContext).toBe('function');
  });

  it('throws when getTauronContext called before set', () => {
    clearTauronContext();
    expect(() => getTauronContext()).toThrow();
  });

  it('sets and gets context', () => {
    const mockBackend = { invoke: () => {}, cancel: () => {}, listen: () => {}, emit: () => {} };
    setTauronContext({ backend: mockBackend as any });
    expect(getTauronContext().backend).toBe(mockBackend);
    clearTauronContext();
  });
});
