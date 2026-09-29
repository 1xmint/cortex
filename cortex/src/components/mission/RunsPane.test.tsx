// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { CortexApiError, type RunSummary } from '../../lib/cortexApi';

const api = vi.hoisted(() => ({
  listRuns: vi.fn(),
  getRun: vi.fn(),
  streamRun: vi.fn(() => new AbortController()),
  cancelRun: vi.fn(),
  resumeRun: vi.fn(),
  createRunPullRequest: vi.fn(),
}));

vi.mock('../../lib/cortexApi', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../lib/cortexApi')>()),
  ...api,
}));

import RunsPane from './RunsPane';

function finishedRun(status: string): RunSummary {
  return {
    id: 'run-1',
    goal: 'ship it',
    status,
    profile: 'auto',
    steps: [{ id: 'step-1', status: 'verified', title: 'do the thing' }],
  };
}

function renderWith(status: string) {
  api.listRuns.mockResolvedValue([
    { id: 'run-1', goal: 'ship it', status, profile: 'auto', created_at: '2026-01-01T00:00:00Z' },
  ]);
  api.getRun.mockResolvedValue(finishedRun(status));
  return render(
    <MemoryRouter>
      <RunsPane />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
});

describe('RunsPane pull request block', () => {
  it('shows it for a run the engine reports as succeeded', async () => {
    renderWith('succeeded');
    expect(await screen.findByRole('button', { name: 'Open pull request' })).toBeInTheDocument();
    // A finished run has nothing to stream.
    expect(api.streamRun).not.toHaveBeenCalled();
  });

  it('shows it for a recovered run', async () => {
    renderWith('recovered');
    expect(await screen.findByRole('button', { name: 'Open pull request' })).toBeInTheDocument();
  });

  it('does not offer one for a failed run', async () => {
    renderWith('failed');
    await screen.findByRole('heading', { name: 'ship it' });
    expect(screen.queryByRole('button', { name: 'Open pull request' })).not.toBeInTheDocument();
  });
});

describe('RunsPane awaiting top-up banner', () => {
  it('says the run is out of credits and links to the billing top-up', async () => {
    renderWith('awaiting_top_up');
    expect(await screen.findByText('Out of credits — top up to continue')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Top up' })).toHaveAttribute('href', '/?settings=billing');
    expect(screen.getByRole('button', { name: 'Resume' })).toBeInTheDocument();
  });

  it('resumes the run and reloads it', async () => {
    renderWith('awaiting_top_up');
    api.resumeRun.mockResolvedValue({ status: 'running' });
    fireEvent.click(await screen.findByRole('button', { name: 'Resume' }));
    await screen.findByRole('heading', { name: 'ship it' });
    expect(api.resumeRun).toHaveBeenCalledWith('run-1');
    expect(api.getRun).toHaveBeenCalledTimes(2);
  });

  it('shows "Top up first" when the balance is still empty (402)', async () => {
    renderWith('awaiting_top_up');
    api.resumeRun.mockRejectedValue(new CortexApiError(402, 'need credits'));
    fireEvent.click(await screen.findByRole('button', { name: 'Resume' }));
    expect(await screen.findByText('Top up first')).toBeInTheDocument();
  });

  it('shows no banner for a run that is not waiting for credits', async () => {
    renderWith('running');
    await screen.findByRole('heading', { name: 'ship it' });
    expect(screen.queryByText('Out of credits — top up to continue')).not.toBeInTheDocument();
  });
});
