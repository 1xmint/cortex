/**
 * The browser half of split-key BYOK for OpenCode Zen (see
 * `crates/api/src/byok.rs` for the server half and the threat model this
 * buys).
 *
 * This module owns the 32-byte AES-256-GCM secret that encrypts a customer's
 * Zen API key. It is generated here, in the browser, with
 * `crypto.getRandomValues`, and never derived from anything else (not the
 * Clerk session, not the API key itself). The server only ever sees it
 * inside a save request's `unlock` field or the `X-Cortex-Key-Unlock`
 * header of a chat request -- it uses the secret once, in memory, and never
 * persists it.
 *
 * Storage is `localStorage`, namespaced per Clerk user id so that two
 * accounts signed into the same browser profile never share (or clobber)
 * each other's device key. It is deliberately kept across sign-out -- like
 * "remember this device" -- since the whole point of a device key is that
 * this browser can keep decrypting its own chats without re-entering the
 * Zen API key every session. Call `clearZenDeviceKey` only when the customer
 * explicitly removes this device's key.
 */

/** A device's split-key BYOK identity: an id and its unlock secret. */
export interface ZenDeviceKey {
  /** Stable per-device id, generated once with `crypto.randomUUID()`. */
  deviceId: string;
  /** 32 random bytes, base64url-encoded without padding. Never logged. */
  secret: string;
}

const SECRET_LEN = 32;

/**
 * `useAuthGate` falls back to the literal string `'anonymous'` for `userId`
 * while Clerk has not finished loading (or has no signed-in user). Saving or
 * loading a device key under that placeholder would let an unauthenticated
 * caller read -- or silently overwrite -- whatever device key later loads
 * for the *real* signed-in user on this browser once auth resolves. Every
 * entry point in this module refuses `''` and `'anonymous'` for that reason.
 */
function isUsableUserId(userId: string): boolean {
  return userId !== '' && userId !== 'anonymous';
}

function storageKey(userId: string): string {
  return `cortex.zenDeviceKey.${userId}`;
}

/** Base64url (no padding) encoding, matching `crates/api/src/byok.rs`'s
 * `URL_SAFE_NO_PAD` decoder. */
function toBase64Url(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  const std = btoa(binary);
  return std.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/**
 * Generates a fresh device id and secret. Does not touch storage -- callers
 * save it (via `saveZenDeviceKey`) only after the server has accepted the
 * key that was encrypted with it.
 */
export function generateZenDeviceKey(): ZenDeviceKey {
  const deviceId = crypto.randomUUID();
  const bytes = new Uint8Array(SECRET_LEN);
  crypto.getRandomValues(bytes);
  return { deviceId, secret: toBase64Url(bytes) };
}

function isZenDeviceKey(value: unknown): value is ZenDeviceKey {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.deviceId === 'string' && candidate.deviceId.length > 0
    && typeof candidate.secret === 'string' && candidate.secret.length > 0;
}

/**
 * Loads this browser's saved device key for `userId`, or `null` if there is
 * none, storage is unavailable, or the stored value is malformed. Never
 * throws -- every storage access is wrapped so a customer with storage
 * disabled (private browsing, extensions, quota) just sees "no local key"
 * rather than a crash.
 */
export function loadZenDeviceKey(userId: string): ZenDeviceKey | null {
  if (!isUsableUserId(userId)) return null;
  try {
    const raw = window.localStorage.getItem(storageKey(userId));
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isZenDeviceKey(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/** Saves `key` as this browser's device key for `userId`. Never throws. */
export function saveZenDeviceKey(userId: string, key: ZenDeviceKey): void {
  if (!isUsableUserId(userId)) return;
  try {
    window.localStorage.setItem(storageKey(userId), JSON.stringify(key));
  } catch {
    // Storage unavailable (quota, private mode, disabled) -- the customer
    // simply will not have a persisted device key on this browser.
  }
}

/**
 * Removes this browser's device key for `userId`. Call only when the
 * customer explicitly removes the key from this device -- not on sign-out.
 */
export function clearZenDeviceKey(userId: string): void {
  if (!isUsableUserId(userId)) return;
  try {
    window.localStorage.removeItem(storageKey(userId));
  } catch {
    // ignore -- nothing to clean up if storage is unavailable
  }
}

/**
 * Formats the `X-Cortex-Key-Unlock` header value / `unlock` save-request
 * value's device-id half: `"<device_id>.<secret>"`. See
 * `crates/api/src/chat_zen.rs`'s `parse_unlock_header`.
 */
export function formatUnlockHeader(key: ZenDeviceKey): string {
  return `${key.deviceId}.${key.secret}`;
}
