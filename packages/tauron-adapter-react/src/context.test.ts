import { describe, it, expect } from 'vitest';
import { TauronContext, useTauronContext, TauronProvider } from './context.js';

describe('TauronContext', () => {
  it('TauronContext is a context object', () => {
    expect(TauronContext).toBeDefined();
    expect(TauronContext.Provider).toBeDefined();
    expect(TauronContext.Consumer).toBeDefined();
  });

  it('TauronProvider is a component', () => {
    expect(typeof TauronProvider).toBe('function');
  });

  it('useTauronContext is a function', () => {
    expect(typeof useTauronContext).toBe('function');
  });
});
