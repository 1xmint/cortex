// @vitest-environment jsdom
//
// PR #65 hid the Projects sidebar link behind SHOW_PROJECTS_LINK but left the
// /projects route reachable by typing the URL directly. This asserts the
// route is gated the same way: rendering the same NotFoundPage fallback as an
// unknown route while the flag is off, and the real ProjectsView once it is
// flipped on.
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const fetchMock = vi.fn(async () => new Response('[]', { status: 200 }));

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockClear();
  window.history.pushState({}, '', '/projects');
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.doUnmock('./lib/featureFlags');
});

describe('/projects route respects SHOW_PROJECTS_LINK', () => {
  it('renders the unknown-route fallback instead of Projects while the flag is off', async () => {
    vi.doMock('./lib/featureFlags', () => ({ SHOW_PROJECTS_LINK: false }));
    const { default: App } = await import('./App');

    render(<App />);

    await screen.findByText('Page not found');
    expect(screen.queryByText('Manage your development projects')).toBeNull();
  });

  it('renders the Projects view when the flag is on', async () => {
    vi.doMock('./lib/featureFlags', () => ({ SHOW_PROJECTS_LINK: true }));
    const { default: App } = await import('./App');

    render(<App />);

    await waitFor(() => screen.getByText('Manage your development projects'));
    expect(screen.queryByText('Page not found')).toBeNull();
  });
});
