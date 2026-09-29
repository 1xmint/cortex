// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import SettingsPanel from './SettingsPanel';

afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

describe('SettingsPanel without Clerk', () => {
  it('renders the Account tab with no ClerkProvider instead of crashing', () => {
    // main.tsx mounts ClerkProvider only when a publishable key is set, so auth
    // disabled / local dev renders this panel with no provider above it.
    vi.stubEnv('VITE_CLERK_PUBLISHABLE_KEY', '');

    render(<SettingsPanel onClose={() => {}} billing={null} initialTab="account" />);

    expect(screen.getByText('Display name')).toBeInTheDocument();
    expect(screen.getByText('Email')).toBeInTheDocument();
  });
});
