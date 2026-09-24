import { describe, it, expect, beforeEach } from 'vitest';
import { EventBus, EventBusError } from './event-bus.js';
import { type PluginEvent } from '@tauron/types';

describe('EventBus', () => {
  let bus: EventBus;

  const makeEvent = (pluginId: string, eventName: string, payload: unknown = {}): PluginEvent => ({
    sourcePlugin: pluginId,
    eventName,
    payload,
    timestamp: new Date().toISOString(),
  });

  beforeEach(() => {
    bus = new EventBus();
  });

  describe('emit', () => {
    it('delivers event to subscribers', () => {
      const received: PluginEvent[] = [];
      const subscriber = { deliver: (e: PluginEvent) => received.push(e) };

      bus.subscribe('com.example.test', 'done', subscriber);
      bus.emit(makeEvent('com.example.test', 'done', { result: 'ok' }));

      expect(received.length).toBe(1);
      expect(received[0]?.payload).toEqual({ result: 'ok' });
    });

    it('does not deliver to unsubscribed subscribers', () => {
      const received: PluginEvent[] = [];
      const subscriber = { deliver: (e: PluginEvent) => received.push(e) };

      bus.subscribe('com.example.test', 'done', subscriber);
      bus.unsubscribe('com.example.test', 'done', subscriber);
      bus.emit(makeEvent('com.example.test', 'done'));

      expect(received.length).toBe(0);
    });

    it('delivers to multiple subscribers', () => {
      let count1 = 0;
      let count2 = 0;

      bus.subscribe('test', 'event', { deliver: () => count1++ });
      bus.subscribe('test', 'event', { deliver: () => count2++ });
      bus.emit(makeEvent('test', 'event'));

      expect(count1).toBe(1);
      expect(count2).toBe(1);
    });
  });

  describe('backpressure', () => {
    it('drops oldest when queue overflows', () => {
      const bus = new EventBus({ maxQueueSize: 3, dropOldest: true });
      const received: PluginEvent[] = [];
      const subscriber = { deliver: (e: PluginEvent) => received.push(e) };

      bus.subscribe('test', 'event', subscriber);

      // Emit 5 events with queue size 3
      for (let i = 0; i < 5; i++) {
        bus.emit(makeEvent('test', 'event', { seq: i }));
      }

      // All 5 should be delivered (oldest dropped from queue, not from subscribers)
      expect(received.length).toBe(5);

      const stats = bus.getQueueStats();
      expect(stats.queueSize).toBe(3);
      expect(stats.droppedCount).toBe(2);
    });

    it('returns error when overflow with dropOldest=false', () => {
      const bus = new EventBus({ maxQueueSize: 1, dropOldest: false });

      bus.emit(makeEvent('test', 'event'));
      const error = bus.emit(makeEvent('test', 'event'));

      expect(error).toBeInstanceOf(EventBusError);
    });
  });

  describe('clearPlugin', () => {
    it('removes all queues and subscribers for a plugin', () => {
      let delivered = 0;
      bus.subscribe('test', 'event', { deliver: () => delivered++ });
      bus.emit(makeEvent('test', 'event'));

      bus.clearPlugin('test');
      bus.emit(makeEvent('test', 'event'));

      expect(delivered).toBe(1); // Only the first one before clear
    });
  });

  describe('getQueueStats', () => {
    it('returns accurate stats', () => {
      bus.emit(makeEvent('plugin-a', 'event1'));
      bus.emit(makeEvent('plugin-a', 'event2'));
      bus.emit(makeEvent('plugin-b', 'event1'));

      const stats = bus.getQueueStats();
      expect(stats.queueSize).toBe(3);
      expect(stats.droppedCount).toBe(0);
    });
  });

  describe('clear', () => {
    it('clears all queues and stats', () => {
      bus.emit(makeEvent('test', 'event'));
      bus.clear();

      const stats = bus.getQueueStats();
      expect(stats.queueSize).toBe(0);
      expect(stats.droppedCount).toBe(0);
    });
  });
});
