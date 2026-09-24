import { describe, it, expect, vi } from 'vitest';
import { createStore, useStore, useStoreSelector } from './store.js';

describe('createStore', () => {
  it('creates store with initial state', () => {
    const store = createStore({ count: 0 });
    expect(store.getState()).toEqual({ count: 0 });
  });

  it('sets state with value', () => {
    const store = createStore(0);
    store.setState(42);
    expect(store.getState()).toBe(42);
  });

  it('sets state with function', () => {
    const store = createStore({ count: 0 });
    store.setState((prev) => ({ count: prev.count + 1 }));
    expect(store.getState()).toEqual({ count: 1 });
  });

  it('does not notify listeners on same value', () => {
    const store = createStore(42);
    const listener = vi.fn();
    store.subscribe(listener);
    store.setState(42);
    expect(listener).not.toHaveBeenCalled();
  });

  it('notifies listeners on state change', () => {
    const store = createStore(0);
    const listener = vi.fn();
    store.subscribe(listener);
    store.setState(1);
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('unsubscribes listener', () => {
    const store = createStore(0);
    const listener = vi.fn();
    const unsub = store.subscribe(listener);
    store.setState(1);
    expect(listener).toHaveBeenCalledTimes(1);

    unsub();
    store.setState(2);
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('supports multiple listeners', () => {
    const store = createStore(0);
    const l1 = vi.fn();
    const l2 = vi.fn();
    store.subscribe(l1);
    store.subscribe(l2);
    store.setState(1);
    expect(l1).toHaveBeenCalledTimes(1);
    expect(l2).toHaveBeenCalledTimes(1);
  });
});

describe('useStore', () => {
  it('is a function', () => {
    expect(typeof useStore).toBe('function');
  });
});

describe('useStoreSelector', () => {
  it('is a function', () => {
    expect(typeof useStoreSelector).toBe('function');
  });
});
