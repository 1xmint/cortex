// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../lib/cortexApi', async () => {
  const actual = await vi.importActual<typeof import('../../lib/cortexApi')>('../../lib/cortexApi');
  return {
    ...actual,
    getProviderKeys: vi.fn(),
    saveProviderKey: vi.fn(),
    deleteProviderKeyDevice: vi.fn(),
    deleteProviderKeyAllDevices: vi.fn(),
  };
});

import {
  CortexApiError,
  getProviderKeys,
  saveProviderKey,
  deleteProviderKeyDevice,
  deleteProviderKeyAllDevices,
} from '../../lib/cortexApi';
import { loadZenDeviceKey } from '../../lib/zenDeviceKey';
import ModelKeysTab from './ModelKeysTab';

afterEach(() => {
  vi.clearAllMocks();
  cleanup();
  window.localStorage.clear();
});

beforeEach(() => {
  window.localStorage.clear();
});

const THIS_DEVICE_ID = 'device-this-11111111-1111-1111-1111-111111111111';

const SUMMARY = {
  provider: 'zen',
  device_id: THIS_DEVICE_ID,
  last4: 'abcd',
  status: 'active',
  created_at: Date.UTC(2026, 0, 1),
  updated_at: Date.UTC(2026, 0, 1),
  last_used_at: Date.UTC(2026, 0, 5),
};

/** Seeds localStorage with a device key for the local (non-Clerk) user id used
 * by `useAuthGate` in this test environment (`VITE_CLERK_PUBLISHABLE_KEY` is
 * unset here, so `useAuthGate` returns the `'local'` gate). */
function seedLocalDeviceKey(deviceId: string) {
  window.localStorage.setItem(
    'cortex.zenDeviceKey.local',
    JSON.stringify({ deviceId, secret: 'test-secret-not-real' }),
  );
}

describe('ModelKeysTab', () => {
  it('never renders more than last4 from the API response', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([{ ...SUMMARY, last4: 'zzzz' }]);
    seedLocalDeviceKey(THIS_DEVICE_ID);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText(/••••zzzz/)).toBeInTheDocument());
    // The saved-key line never contains anything that looks like a full key.
    expect(screen.queryByText(/sk-/)).not.toBeInTheDocument();
  });

  it('shows the empty state with the Zen BYOK copy when no key is saved', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([]);

    render(<ModelKeysTab />);

    await waitFor(() =>
      expect(
        screen.getByText(/Cortex does not charge credits for these; Zen bills you directly\./),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(/kept only in this browser/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/OpenCode Zen API key/i)).toHaveAttribute('type', 'password');
  });

  it('generates a device key, sends it on save, and only stores it locally after a 2xx', async () => {
    vi.mocked(getProviderKeys)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([SUMMARY]);
    vi.mocked(saveProviderKey).mockResolvedValue(undefined);

    render(<ModelKeysTab />);

    const input = await screen.findByLabelText(/OpenCode Zen API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-secret-value' } });
    expect(input.value).toBe('sk-secret-value');

    expect(loadZenDeviceKey('local')).toBeNull();

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /save key/i }));
      await Promise.resolve();
    });

    expect(saveProviderKey).toHaveBeenCalledTimes(1);
    const [provider, apiKey, deviceId, unlock] = vi.mocked(saveProviderKey).mock.calls[0];
    expect(provider).toBe('zen');
    expect(apiKey).toBe('sk-secret-value');
    expect(typeof deviceId).toBe('string');
    expect(deviceId.length).toBeGreaterThan(0);
    expect(typeof unlock).toBe('string');

    // The saved view replaces the form; there is no leftover key input holding the value.
    await waitFor(() => expect(screen.queryByLabelText(/OpenCode Zen API key/i)).not.toBeInTheDocument());

    const stored = loadZenDeviceKey('local');
    expect(stored?.deviceId).toBe(deviceId);
  });

  it('does not persist a device key locally when the save fails', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([]);
    vi.mocked(saveProviderKey).mockRejectedValue(new CortexApiError(400, 'that does not look like a Zen key'));

    render(<ModelKeysTab />);

    const input = await screen.findByLabelText(/OpenCode Zen API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'not-a-real-key' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /save key/i }));
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByText('that does not look like a Zen key')).toBeInTheDocument());
    expect(input.value).toBe('');
    expect(loadZenDeviceKey('local')).toBeNull();
  });

  it('shows the server 409 message when the 10-device cap is hit', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([]);
    vi.mocked(saveProviderKey).mockRejectedValue(
      new CortexApiError(409, 'you already have 10 devices saved for this provider; remove one before adding another'),
    );

    render(<ModelKeysTab />);

    const input = await screen.findByLabelText(/OpenCode Zen API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-secret-value' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /save key/i }));
      await Promise.resolve();
    });

    await waitFor(() =>
      expect(screen.getByText(/you already have 10 devices saved/i)).toBeInTheDocument(),
    );
  });

  it('shows this device vs other devices, and removes only this device on "Remove from this device"', async () => {
    const otherDeviceEntry = {
      ...SUMMARY,
      device_id: 'device-other-22222222-2222-2222-2222-222222222222',
      last4: '9999',
    };
    vi.mocked(getProviderKeys).mockResolvedValue([SUMMARY, otherDeviceEntry]);
    vi.mocked(deleteProviderKeyDevice).mockResolvedValue(undefined);
    seedLocalDeviceKey(THIS_DEVICE_ID);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText('This device')).toBeInTheDocument());
    expect(screen.getByText('Other devices')).toBeInTheDocument();
    expect(screen.getByText(/••••9999/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /remove from this device/i }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /confirm remove$/i }));
      await Promise.resolve();
    });

    await waitFor(() => expect(deleteProviderKeyDevice).toHaveBeenCalledWith('zen', THIS_DEVICE_ID));
    expect(loadZenDeviceKey('local')).toBeNull();
  });

  it('removes from all devices via "Remove from all devices"', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([SUMMARY]);
    vi.mocked(deleteProviderKeyAllDevices).mockResolvedValue(undefined);
    seedLocalDeviceKey(THIS_DEVICE_ID);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText('This device')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: /remove from all devices/i }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /confirm remove from all devices/i }));
      await Promise.resolve();
    });

    await waitFor(() => expect(deleteProviderKeyAllDevices).toHaveBeenCalledWith('zen'));
    expect(loadZenDeviceKey('local')).toBeNull();
  });

  it('reuses this device\'s deviceId on replace, changing only the secret', async () => {
    vi.mocked(getProviderKeys)
      .mockResolvedValueOnce([{ ...SUMMARY, status: 'rejected' }])
      .mockResolvedValueOnce([SUMMARY]);
    vi.mocked(saveProviderKey).mockResolvedValue(undefined);
    seedLocalDeviceKey(THIS_DEVICE_ID);
    const before = loadZenDeviceKey('local');

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByRole('button', { name: /replace/i })).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /replace/i }));

    const input = await screen.findByLabelText(/OpenCode Zen API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-new-secret-value' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /save key/i }));
      await Promise.resolve();
    });

    await waitFor(() => expect(saveProviderKey).toHaveBeenCalledTimes(1));
    const [, , deviceId, unlock] = vi.mocked(saveProviderKey).mock.calls[0];
    expect(deviceId).toBe(THIS_DEVICE_ID);
    expect(unlock).not.toBe(before?.secret);

    const after = loadZenDeviceKey('local');
    expect(after?.deviceId).toBe(THIS_DEVICE_ID);
    expect(after?.secret).not.toBe(before?.secret);
  });

  it('keeps the old stored key when a replace save fails', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([{ ...SUMMARY, status: 'rejected' }]);
    vi.mocked(saveProviderKey).mockRejectedValue(new CortexApiError(400, 'zen rejected that key'));
    seedLocalDeviceKey(THIS_DEVICE_ID);
    const before = loadZenDeviceKey('local');

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByRole('button', { name: /replace/i })).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /replace/i }));

    const input = await screen.findByLabelText(/OpenCode Zen API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-new-secret-value' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /save key/i }));
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByText('zen rejected that key')).toBeInTheDocument());
    expect(loadZenDeviceKey('local')).toEqual(before);
  });

  it('removes another device via its row button and refreshes the list', async () => {
    const otherDeviceEntry = {
      ...SUMMARY,
      device_id: 'device-other-33333333-3333-3333-3333-333333333333',
      last4: '4242',
    };
    vi.mocked(getProviderKeys)
      .mockResolvedValueOnce([SUMMARY, otherDeviceEntry])
      .mockResolvedValueOnce([SUMMARY]);
    vi.mocked(deleteProviderKeyDevice).mockResolvedValue(undefined);
    seedLocalDeviceKey(THIS_DEVICE_ID);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText(/••••4242/)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /remove device ••••4242/i }));
    fireEvent.click(await screen.findByRole('button', { name: /confirm remove/i }));

    await waitFor(() => expect(deleteProviderKeyDevice).toHaveBeenCalledWith('zen', otherDeviceEntry.device_id));
    await waitFor(() => expect(screen.queryByText(/••••4242/)).not.toBeInTheDocument());
    // Removing another device must not touch this device's stored key.
    expect(loadZenDeviceKey('local')?.deviceId).toBe(THIS_DEVICE_ID);
  });

  it('asks for confirmation before removing another device, and sends no DELETE on cancel', async () => {
    const otherDeviceEntry = {
      ...SUMMARY,
      device_id: 'device-other-33333333-3333-3333-3333-333333333333',
      last4: '4242',
    };
    vi.mocked(getProviderKeys).mockResolvedValue([SUMMARY, otherDeviceEntry]);
    seedLocalDeviceKey(THIS_DEVICE_ID);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText(/••••4242/)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /remove device ••••4242/i }));

    // The confirm row appears, and cancelling it must send no DELETE at all.
    fireEvent.click(await screen.findByRole('button', { name: /cancel/i }));

    expect(deleteProviderKeyDevice).not.toHaveBeenCalled();
    expect(screen.getByText(/••••4242/)).toBeInTheDocument();
  });

  it('shows a rejected key for this device with Replace available', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([{ ...SUMMARY, status: 'rejected' }]);
    seedLocalDeviceKey(THIS_DEVICE_ID);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText('Zen rejected this key')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: /replace/i })).toBeInTheDocument();
  });
});
