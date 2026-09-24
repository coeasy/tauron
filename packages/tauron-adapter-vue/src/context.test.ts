import { describe, it, expect } from 'vitest';
import { provideTauronContext, injectTauronContext, TauronContextKey } from './context.js';

describe('context', () => {
  it('exports provideTauronContext', () => {
    expect(typeof provideTauronContext).toBe('function');
  });

  it('exports injectTauronContext', () => {
    expect(typeof injectTauronContext).toBe('function');
  });

  it('exports TauronContextKey', () => {
    expect(TauronContextKey).toBeDefined();
  });
});
