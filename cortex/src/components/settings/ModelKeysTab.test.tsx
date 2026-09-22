// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../lib/cortexApi', async () => {
  const actual = await vi.importActual<typeof import('../../lib/cortexApi')>('../../lib/cortexApi');
  return {
    ...actual,
    getProviderKeys: vi.fn(),
    saveProviderKey: vi.fn(),
    deleteProviderKey: vi.fn(),
  };
});

import { CortexApiError, getProviderKeys, saveProviderKey, deleteProviderKey } from '../../lib/cortexApi';
import ModelKeysTab from './ModelKeysTab';

afterEach(() => {
  vi.clearAllMocks();
  cleanup();
});

const SUMMARY = {
  provider: 'zen',
  last4: 'abcd',
  status: 'active',
  created_at: Date.UTC(2026, 0, 1),
  updated_at: Date.UTC(2026, 0, 1),
  last_used_at: Date.UTC(2026, 0, 5),
};

describe('ModelKeysTab', () => {
  it('never renders more than last4 from the API response', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([{ ...SUMMARY, last4: 'zzzz' }]);

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
    expect(screen.getByLabelText(/OpenCode Zen API key/i)).toHaveAttribute('type', 'password');
  });

  it('clears the input after a successful save', async () => {
    vi.mocked(getProviderKeys)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([SUMMARY]);
    vi.mocked(saveProviderKey).mockResolvedValue(undefined);

    render(<ModelKeysTab />);

    const input = await screen.findByLabelText(/OpenCode Zen API key/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'sk-secret-value' } });
    expect(input.value).toBe('sk-secret-value');

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /save key/i }));
      await Promise.resolve();
    });

    await waitFor(() => expect(saveProviderKey).toHaveBeenCalledWith('zen', 'sk-secret-value'));
    // The saved view replaces the form; there is no leftover key input holding the value.
    expect(screen.queryByLabelText(/OpenCode Zen API key/i)).not.toBeInTheDocument();
  });

  it('clears the input after a failed save and shows the server error', async () => {
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
  });

  it('shows a rejected key with Replace, and confirms before delete', async () => {
    vi.mocked(getProviderKeys).mockResolvedValue([{ ...SUMMARY, status: 'rejected' }]);
    vi.mocked(deleteProviderKey).mockResolvedValue(undefined);

    render(<ModelKeysTab />);

    await waitFor(() => expect(screen.getByText('Zen rejected this key')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: /replace/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /^delete$/i }));
    expect(screen.getByText(/delete your saved zen key/i)).toBeInTheDocument();
    expect(deleteProviderKey).not.toHaveBeenCalled();

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /confirm delete/i }));
      await Promise.resolve();
    });
    await waitFor(() => expect(deleteProviderKey).toHaveBeenCalledWith('zen'));
  });
});
