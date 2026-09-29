// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import type { BillingStatus } from '../../lib/cortexApi';
import TrialBanner from './TrialBanner';

afterEach(() => {
  cleanup();
});

function needsCheckout(planType: string): BillingStatus {
  return {
    access_state: 'needs_checkout',
    plan: {
      plan_type: planType,
      status: 'active',
      billing_period_end: '2026-01-01',
      next_charge_amount_cents: null,
      next_charge_date: null,
      started_at: '2025-01-01',
    },
    trial: null,
    delegation: { status: 'not_issued', budget_enforced: false, delegation_id: null, expires_at: null },
    payment_method: null,
    referral: null,
  } as unknown as BillingStatus;
}

describe('TrialBanner plan name', () => {
  it('never renders "You\'re in ." when the plan name is empty', () => {
    const { container } = render(<TrialBanner billing={needsCheckout('')} onOpenBilling={() => {}} />);
    expect(container.textContent).not.toMatch(/You're in \./);
    expect(container.textContent).toMatch(/You're in Preview mode\./);
  });

  it('falls back to Preview mode when there is no billing data', () => {
    const { container } = render(<TrialBanner billing={null} onOpenBilling={() => {}} />);
    expect(container.textContent).not.toMatch(/You're in \./);
    expect(screen.getByText('Preview mode')).toBeInTheDocument();
  });
});
