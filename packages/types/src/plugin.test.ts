import { describe, it, expect } from 'vitest';
import {
  TRANSITIONS,
  TERMINAL_STATES,
  ACTIVE_STATES,
  isValidTransition,
  allowedTransitions,
  eventNamespace,
  pluginForm,
  type PluginState,
} from './plugin.js';

describe('plugin state machine', () => {
  describe('TRANSITIONS', () => {
    it('has entries for all 10 states', () => {
      const states: PluginState[] = [
        'DISCOVERED', 'INSTALLING', 'INSTALLED', 'ENABLING', 'ENABLED',
        'DISABLING', 'DISABLED', 'ERRORED', 'UNINSTALLING', 'UPGRADING',
      ];
      for (const state of states) {
        expect(TRANSITIONS[state]).toBeDefined();
      }
    });

    it('DISCOVERED can only go to INSTALLING', () => {
      expect(allowedTransitions('DISCOVERED')).toEqual(['INSTALLING']);
    });

    it('INSTALLING can go to INSTALLED or ERRORED', () => {
      expect(allowedTransitions('INSTALLING')).toEqual(['INSTALLED', 'ERRORED']);
    });

    it('ENABLED can go to DISABLING, ERRORED, or UPGRADING', () => {
      expect(allowedTransitions('ENABLED')).toEqual(['DISABLING', 'ERRORED', 'UPGRADING']);
    });

    it('UNINSTALLING is terminal (no transitions)', () => {
      expect(allowedTransitions('UNINSTALLING')).toEqual([]);
    });

    it('UPGRADING can go to INSTALLED or ERRORED', () => {
      expect(allowedTransitions('UPGRADING')).toEqual(['INSTALLED', 'ERRORED']);
    });
  });

  describe('TERMINAL_STATES', () => {
    it('contains UNINSTALLING', () => {
      expect(TERMINAL_STATES.has('UNINSTALLING')).toBe(true);
    });

    it('does not contain other states', () => {
      expect(TERMINAL_STATES.has('ENABLED')).toBe(false);
      expect(TERMINAL_STATES.has('DISABLED')).toBe(false);
    });
  });

  describe('ACTIVE_STATES', () => {
    it('contains only ENABLED', () => {
      expect(ACTIVE_STATES.has('ENABLED')).toBe(true);
      expect(ACTIVE_STATES.size).toBe(1);
    });
  });

  describe('isValidTransition', () => {
    it('allows valid transitions', () => {
      expect(isValidTransition('DISCOVERED', 'INSTALLING')).toBe(true);
      expect(isValidTransition('INSTALLING', 'INSTALLED')).toBe(true);
      expect(isValidTransition('ENABLED', 'DISABLING')).toBe(true);
    });

    it('rejects invalid transitions', () => {
      expect(isValidTransition('DISCOVERED', 'ENABLED')).toBe(false);
      expect(isValidTransition('ENABLED', 'DISCOVERED')).toBe(false);
      expect(isValidTransition('UNINSTALLING', 'ENABLED')).toBe(false);
    });

    it('rejects self-loops', () => {
      expect(isValidTransition('ENABLED', 'ENABLED')).toBe(false);
    });
  });

  describe('eventNamespace', () => {
    it('generates plugin:<id>:<event> format', () => {
      expect(eventNamespace('com.example.formatter', 'format-done')).toBe(
        'plugin:com.example.formatter:format-done',
      );
    });
  });

  describe('pluginForm', () => {
    it('maps types to forms', () => {
      expect(pluginForm('rust')).toBe('A');
      expect(pluginForm('js')).toBe('B');
      expect(pluginForm('process')).toBe('C');
      expect(pluginForm('wasm')).toBe('D');
    });
  });
});
