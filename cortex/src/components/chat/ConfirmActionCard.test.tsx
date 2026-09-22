// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import * as cortexApi from '../../lib/cortexApi';
import { CortexApiError } from '../../lib/cortexApi';
import ConfirmActionCard from './ConfirmActionCard';
import type { ConfirmActionRequest } from '../../types';

function makeRequest(overrides: Partial<ConfirmActionRequest> = {}): ConfirmActionRequest {
  return {
    messageId: 'm-1',
    actionId: 'action-1',
    nonce: 'nonce-1',
    summary: 'Delete 3 stale branches',
    expiresAt: new Date(Date.now() + 30_000).toISOString(),
    status: 'pending',
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('ConfirmActionCard', () => {
  it('renders the summary and a countdown', () => {
    render(<ConfirmActionCard request={makeRequest()} onStatusChange={vi.fn()} />);
    expect(screen.getByText('Delete 3 stale branches')).toBeInTheDocument();
    expect(screen.getByText('0:30')).toBeInTheDocument();
  });

  it('posts the nonce once even on a double click', async () => {
    const confirmSpy = vi
      .spyOn(cortexApi, 'confirmAgentAction')
      .mockImplementation(() => new Promise((resolve) => setTimeout(() => resolve({ status: 'confirmed' }), 20)));
    const onStatusChange = vi.fn();
    render(<ConfirmActionCard request={makeRequest()} onStatusChange={onStatusChange} />);

    const confirmButton = screen.getByRole('button', { name: /confirm:/i });
    fireEvent.click(confirmButton);
    fireEvent.click(confirmButton);

    expect(confirmButton).toBeDisabled();

    await waitFor(() => expect(onStatusChange).toHaveBeenCalledWith('m-1', 'confirmed'));
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(confirmSpy).toHaveBeenCalledWith('action-1', 'nonce-1');
  });

  it('cancels and reports the resolution', async () => {
    const cancelSpy = vi
      .spyOn(cortexApi, 'cancelAgentAction')
      .mockResolvedValue({ status: 'cancelled' });
    const onStatusChange = vi.fn();
    render(<ConfirmActionCard request={makeRequest()} onStatusChange={onStatusChange} />);

    fireEvent.click(screen.getByRole('button', { name: /cancel:/i }));

    await waitFor(() => expect(onStatusChange).toHaveBeenCalledWith('m-1', 'cancelled'));
    expect(cancelSpy).toHaveBeenCalledWith('action-1', 'nonce-1');
  });

  it('expires on its own when the countdown hits zero', () => {
    vi.useFakeTimers();
    const onStatusChange = vi.fn();
    const request = makeRequest({ expiresAt: new Date(Date.now() + 2000).toISOString() });
    render(<ConfirmActionCard request={request} onStatusChange={onStatusChange} />);

    act(() => {
      vi.advanceTimersByTime(2500);
    });

    expect(onStatusChange).toHaveBeenCalledWith('m-1', 'expired');
  });

  it('shows no-longer-available on a 404', async () => {
    vi.spyOn(cortexApi, 'confirmAgentAction').mockRejectedValue(
      new CortexApiError(404, 'not found'),
    );
    const onStatusChange = vi.fn();
    render(<ConfirmActionCard request={makeRequest()} onStatusChange={onStatusChange} />);

    fireEvent.click(screen.getByRole('button', { name: /confirm:/i }));

    await waitFor(() => expect(onStatusChange).toHaveBeenCalledWith('m-1', 'unavailable'));
  });

  it('renders a static line once resolved', () => {
    render(
      <ConfirmActionCard
        request={makeRequest({ status: 'confirmed' })}
        onStatusChange={vi.fn()}
      />,
    );
    expect(screen.getByText(/Confirmed: Delete 3 stale branches/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /confirm:/i })).not.toBeInTheDocument();
  });

  it('renders "Replaced" for a superseded card', () => {
    render(
      <ConfirmActionCard
        request={makeRequest({ status: 'replaced' })}
        onStatusChange={vi.fn()}
      />,
    );
    expect(screen.getByText('Replaced')).toBeInTheDocument();
  });
});
