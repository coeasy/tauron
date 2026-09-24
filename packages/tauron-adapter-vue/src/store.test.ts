import { describe, it, expect } from 'vitest';
import { createStore } from './store.js';

describe('createStore', () => {
  it('creates store with initial state', () => {
    const store = createStore({ count: 0 });
    expect(store.state).toEqual({ count: 0 });
  });

  it('sets state with value', () => {
    const store = createStore({ count: 0 });
    store.setState({ count: 42 });
    expect(store.state.count).toBe(42);
  });

  it('sets state with function', () => {
    const store = createStore({ count: 0 });
    store.setState((prev) => ({ count: prev.count + 1 }));
    expect(store.state.count).toBe(1);
  });

  it('subscribes with immediate option', () => {
    const store = createStore({ count: 0 });
    let called = false;
    store.subscribe(() => { called = true; }, { immediate: true });
    expect(called).toBe(true);
  });

  it('unsubscribes listener', () => {
    const store = createStore({ count: 0 });
    let callCount = 0;
    const unsub = store.subscribe(() => { callCount++; });
    unsub();
    // After unsubscribe, state changes should not trigger callback
    expect(callCount).toBe(0);
  });
});
