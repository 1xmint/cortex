// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../lib/cortexApi', () => ({
  getCreditsBalance: vi.fn(async () => 200),
  createTopupCheckout: vi.fn(),
}));

import CreditsCard from './CreditsCard';

afterEach(() => {
  cleanup();
});

describe('CreditsCard', () => {
  it('shows the balance in dollars with no raw credit count', async () => {
    render(<CreditsCard />);
    const balance = await screen.findByTestId('credits-balance');
    expect(balance).toHaveTextContent('$20.00');
    expect(balance.textContent).not.toMatch(/credit|200/i);
  });

  it('states the credit grant only next to the dollars paid on the purchase buttons', async () => {
    const { container } = render(<CreditsCard />);
    await screen.findByTestId('credits-balance');
    expect(screen.getByRole('button', { name: 'Pay $25.00 · get 250 credits' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Pay $10.00 · get 100 credits' })).toBeInTheDocument();
    // Everywhere else on the card the user reads dollars, not credits.
    const prose = Array.from(container.querySelectorAll('p')).map((p) => p.textContent ?? '');
    expect(prose.join(' ')).not.toMatch(/credit/i);
  });
});
