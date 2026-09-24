import { describe, it, expect, vi } from 'vitest';
import {
  createSignal,
  createComputed,
  createEffect,
  batch,
  enqueueBatch,
  useSignal,
  watchSignal,
  Signal,
  Computed,
  Effect,
} from './signals.js';

describe('Signal', () => {
  it('创建并读取初始值', () => {
    const sig = createSignal(0);
    expect(sig.value).toBe(0);
  });

  it('写入新值触发通知', () => {
    const sig = createSignal(0);
    const values: number[] = [];
    sig.subscribe((v) => values.push(v));
    sig.set(1);
    sig.set(2);
    expect(values).toEqual([0, 1, 2]);
  });

  it('相同值不触发通知', () => {
    const sig = createSignal(0);
    const values: number[] = [];
    sig.subscribe((v) => values.push(v));
    sig.set(1);
    sig.set(1); // 相同值
    expect(values).toEqual([0, 1]);
  });

  it('支持函数式更新', () => {
    const sig = createSignal(0);
    sig.set((prev) => prev + 1);
    expect(sig.value).toBe(1);
    sig.set((prev) => prev + 10);
    expect(sig.value).toBe(11);
  });

  it('支持自定义相等检查', () => {
    const sig = createSignal({ x: 1 }, (a, b) => a.x === b.x);
    const values: { x: number }[] = [];
    sig.subscribe((v) => values.push(v));
    sig.set({ x: 1 }); // x 相同，不触发
    sig.set({ x: 2 }); // x 不同，触发
    expect(values.length).toBe(2); // 初始 + x=2
  });

  it('取消订阅后不再通知', () => {
    const sig = createSignal(0);
    const values: number[] = [];
    const unsub = sig.subscribe((v) => values.push(v));
    sig.set(1);
    unsub();
    sig.set(2);
    expect(values).toEqual([0, 1]);
  });

  it('force 跳过相等检查', () => {
    const sig = createSignal(0);
    const values: number[] = [];
    sig.subscribe((v) => values.push(v));
    sig.force(0); // force 触发即使值相同
    expect(values).toEqual([0, 0]);
  });

  it('listenerCount 反映订阅数', () => {
    const sig = createSignal(0);
    expect(sig.listenerCount).toBe(0);
    const u1 = sig.subscribe(() => {});
    const u2 = sig.subscribe(() => {});
    expect(sig.listenerCount).toBe(2);
    u1();
    expect(sig.listenerCount).toBe(1);
    u2();
    expect(sig.listenerCount).toBe(0);
  });

  it('多个订阅者各自独立', () => {
    const sig = createSignal(0);
    const a: number[] = [];
    const b: number[] = [];
    sig.subscribe((v) => a.push(v));
    sig.subscribe((v) => b.push(v));
    sig.set(1);
    expect(a).toEqual([0, 1]);
    expect(b).toEqual([0, 1]);
  });
});

describe('Computed', () => {
  it('计算值正确', () => {
    const a = createSignal(1);
    const b = createSignal(2);
    const sum = createComputed(() => a.value + b.value);
    expect(sum.value).toBe(3);
  });

  it('依赖变化后重新计算', () => {
    const a = createSignal(1);
    const b = createSignal(2);
    const sum = createComputed(() => a.value + b.value);
    expect(sum.value).toBe(3);
    a.set(10);
    expect(sum.value).toBe(12);
  });

  it('invalidate 强制重算', () => {
    let counter = 0;
    const val = createComputed(() => {
      counter++;
      return counter;
    });
    expect(val.value).toBe(1);
    expect(val.value).toBe(1); // 缓存
    val.invalidate();
    expect(val.value).toBe(2); // 重算
  });

  it('订阅 computed 变更', () => {
    const a = createSignal(1);
    const sum = createComputed(() => a.value * 2);
    const values: number[] = [];
    sum.subscribe((v) => values.push(v));
    a.set(2);
    sum.invalidate();
    sum.notify();
    expect(values).toContain(4);
  });

  it('有依赖时仍然缓存（依赖订阅不得使自身永久失效）', () => {
    const a = createSignal(1);
    let calls = 0;
    const doubled = createComputed(() => {
      calls++;
      return a.value * 2;
    });

    expect(doubled.value).toBe(2);
    expect(doubled.value).toBe(2);
    expect(doubled.value).toBe(2);
    expect(calls).toBe(1); // 重复读取只计算一次

    a.set(3); // 依赖变化 → 失效
    expect(doubled.value).toBe(6);
    expect(calls).toBe(2);
  });

  it('计算值未变化时 notify 不下发（isEqual 生效）', () => {
    const a = createSignal(1);
    const parity = createComputed(() => (a.value % 2 === 0 ? 'even' : 'odd'));
    const seen: string[] = [];
    parity.subscribe((v) => seen.push(v)); // 首次立即推送 'odd'

    a.set(3); // 奇偶性未变
    parity.notify();
    expect(seen).toEqual(['odd']);

    a.set(2); // 奇偶性改变
    parity.notify();
    expect(seen).toEqual(['odd', 'even']);
  });
});

describe('Effect', () => {
  it('首次运行', () => {
    const sig = createSignal(0);
    let count = 0;
    const effect = createEffect(() => {
      void sig.value; // 裸读是刻意的：向 effect 注册依赖，不是无用表达式
      count++;
    });
    expect(count).toBe(1);
    effect.dispose();
    expect(effect.isDisposed).toBe(true);
  });

  it('dispose 后不再运行', () => {
    const sig = createSignal(0);
    let count = 0;
    const effect = createEffect(() => {
      void sig.value; // 裸读是刻意的：向 effect 注册依赖
      count++;
    });
    effect.dispose();
    sig.set(1);
    expect(count).toBe(1); // 不增加
  });

  it('recompute 重新运行', () => {
    const sig = createSignal(0);
    let count = 0;
    const effect = createEffect(() => {
      void sig.value; // 裸读是刻意的：向 effect 注册依赖
      count++;
    });
    expect(count).toBe(1);
    effect.recompute();
    expect(count).toBe(2);
  });
});

describe('batch', () => {
  it('批量写入', () => {
    const sig = createSignal(0);
    let notifications = 0;
    sig.subscribe(() => notifications++);

    batch(() => {
      sig.set(1);
      sig.set(2);
      sig.set(3);
    });

    // 通知可能在 batch 结束时触发
    expect(sig.value).toBe(3);
  });

  it('嵌套 batch 只 flush 最外层', () => {
    const sig = createSignal(0);
    batch(() => {
      batch(() => {
        sig.set(1);
      });
      sig.set(2);
    });
    expect(sig.value).toBe(2);
  });

  it('回调内再次 enqueueBatch：迭代消费而非递归展开（无栈溢出）', () => {
    const ran: number[] = [];
    let depth = 0;
    let maxDepth = 0;

    const enqueueChain = (n: number): void => {
      enqueueBatch(() => {
        depth++;
        maxDepth = Math.max(maxDepth, depth);
        ran.push(n);
        if (n < 3) {
          enqueueChain(n + 1);
        }
        depth--;
      });
    };

    enqueueChain(1);

    // 三个回调都要跑到（重入的入队不会被丢）。
    expect(ran).toEqual([1, 2, 3]);
    // 关键：maxDepth 恒为 1 —— 回调不在彼此内部嵌套执行。
    // 修复前每次重入都递归 flush，maxDepth 会等于 3（链越长栈越深）。
    expect(maxDepth).toBe(1);
  });
});

describe('useSignal', () => {
  it('返回 [signal, setter]', () => {
    const [sig, set] = useSignal(0);
    expect(sig.value).toBe(0);
    set(1);
    expect(sig.value).toBe(1);
  });

  it('setter 支持函数式更新', () => {
    const [sig, set] = useSignal(0);
    set((prev) => prev + 5);
    expect(sig.value).toBe(5);
  });
});

describe('watchSignal', () => {
  it('返回清理函数', () => {
    const sig = createSignal(0);
    const values: number[] = [];
    const unsub = watchSignal(sig, (v) => values.push(v));
    sig.set(1);
    unsub();
    sig.set(2);
    expect(values).toEqual([0, 1]);
  });
});
