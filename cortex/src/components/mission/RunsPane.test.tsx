// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { RunSummary } from '../../lib/cortexApi';

const api = vi.hoisted(() => ({
  listRuns: vi.fn(),
  getRun: vi.fn(),
  streamRun: vi.fn(() => new AbortController()),
  cancelRun: vi.fn(),
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
