// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  clearZenDeviceKey,
  formatUnlockHeader,
  generateZenDeviceKey,
  loadZenDeviceKey,
  saveZenDeviceKey,
} from './zenDeviceKey';

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe('generateZenDeviceKey', () => {
  it('produces a deviceId and a secret that decodes to exactly 32 bytes', () => {
    const key = generateZenDeviceKey();
    expect(typeof key.deviceId).toBe('string');
    expect(key.deviceId.length).toBeGreaterThan(0);

    // base64url, no padding
    expect(key.secret).not.toContain('+');
    expect(key.secret).not.toContain('/');
    expect(key.secret).not.toContain('=');

    const std = key.secret.replace(/-/g, '+').replace(/_/g, '/');
    const padded = std + '='.repeat((4 - (std.length % 4)) % 4);
    const bytes = atob(padded);
    expect(bytes.length).toBe(32);
  });

  it('generates different keys each call', () => {
    const a = generateZenDeviceKey();
    const b = generateZenDeviceKey();
    expect(a.deviceId).not.toBe(b.deviceId);
    expect(a.secret).not.toBe(b.secret);
  });
});

describe('save / load / clear round trip', () => {
  it('round-trips a saved key', () => {
    const key = generateZenDeviceKey();
    saveZenDeviceKey('user-1', key);
    expect(loadZenDeviceKey('user-1')).toEqual(key);
  });

  it('returns null when nothing is saved', () => {
    expect(loadZenDeviceKey('user-nothing-saved')).toBeNull();
  });

  it('clears the saved key', () => {
    const key = generateZenDeviceKey();
    saveZenDeviceKey('user-1', key);
    clearZenDeviceKey('user-1');
    expect(loadZenDeviceKey('user-1')).toBeNull();
  });

  it('keeps different users\' keys separate', () => {
    const keyA = generateZenDeviceKey();
    const keyB = generateZenDeviceKey();
    saveZenDeviceKey('user-a', keyA);
    saveZenDeviceKey('user-b', keyB);

    expect(loadZenDeviceKey('user-a')).toEqual(keyA);
    expect(loadZenDeviceKey('user-b')).toEqual(keyB);

    clearZenDeviceKey('user-a');
    expect(loadZenDeviceKey('user-a')).toBeNull();
    expect(loadZenDeviceKey('user-b')).toEqual(keyB);
  });
});

describe('anonymous / empty user id', () => {
  it('never saves, loads, or clears a device key for the "anonymous" placeholder', () => {
    const key = generateZenDeviceKey();
    saveZenDeviceKey('anonymous', key);
    expect(loadZenDeviceKey('anonymous')).toBeNull();
    expect(window.localStorage.getItem('cortex.zenDeviceKey.anonymous')).toBeNull();

    // Seed as if a bug elsewhere wrote under the placeholder anyway --
    // loading it back must still refuse.
    window.localStorage.setItem('cortex.zenDeviceKey.anonymous', JSON.stringify(key));
    expect(loadZenDeviceKey('anonymous')).toBeNull();

    expect(() => clearZenDeviceKey('anonymous')).not.toThrow();
  });

  it('never saves, loads, or clears a device key for an empty user id', () => {
    const key = generateZenDeviceKey();
    saveZenDeviceKey('', key);
    expect(loadZenDeviceKey('')).toBeNull();
    expect(window.localStorage.getItem('cortex.zenDeviceKey.')).toBeNull();
  });
});

describe('malformed / unavailable storage', () => {
  it('returns null for malformed JSON', () => {
    window.localStorage.setItem('cortex.zenDeviceKey.user-1', 'not-json{{{');
    expect(loadZenDeviceKey('user-1')).toBeNull();
  });

  it('returns null for a value missing required fields', () => {
    window.localStorage.setItem('cortex.zenDeviceKey.user-1', JSON.stringify({ deviceId: 'x' }));
    expect(loadZenDeviceKey('user-1')).toBeNull();

    window.localStorage.setItem('cortex.zenDeviceKey.user-1', JSON.stringify({ secret: 'y' }));
    expect(loadZenDeviceKey('user-1')).toBeNull();

    window.localStorage.setItem('cortex.zenDeviceKey.user-1', JSON.stringify({ deviceId: '', secret: '' }));
    expect(loadZenDeviceKey('user-1')).toBeNull();

    window.localStorage.setItem('cortex.zenDeviceKey.user-1', JSON.stringify(null));
    expect(loadZenDeviceKey('user-1')).toBeNull();
  });

  it('does not throw when storage read throws, and returns null', () => {
    vi.spyOn(window.localStorage.__proto__, 'getItem').mockImplementation(() => {
      throw new Error('storage disabled');
    });
    expect(() => loadZenDeviceKey('user-1')).not.toThrow();
    expect(loadZenDeviceKey('user-1')).toBeNull();
  });

  it('does not throw when storage write throws', () => {
    vi.spyOn(window.localStorage.__proto__, 'setItem').mockImplementation(() => {
      throw new Error('quota exceeded');
    });
    expect(() => saveZenDeviceKey('user-1', generateZenDeviceKey())).not.toThrow();
  });

  it('does not throw when storage removal throws', () => {
    vi.spyOn(window.localStorage.__proto__, 'removeItem').mockImplementation(() => {
      throw new Error('storage disabled');
    });
    expect(() => clearZenDeviceKey('user-1')).not.toThrow();
  });
});

describe('formatUnlockHeader', () => {
  it('joins deviceId and secret with a dot', () => {
    expect(formatUnlockHeader({ deviceId: 'abc', secret: 'def' })).toBe('abc.def');
  });
});
