import { describe, expect, it } from 'vitest';

import { creditsForDollars, formatCreditGrant, formatDollars } from './formatDollars';

describe('formatDollars', () => {
  it('converts credits at $0.10 each with two decimals', () => {
    expect(formatDollars(5)).toBe('$0.50');
    expect(formatDollars(12.3)).toBe('$1.23');
    expect(formatDollars(200)).toBe('$20.00');
    expect(formatDollars(1)).toBe('$0.10');
  });

  it('shows zero as $0.00', () => {
    expect(formatDollars(0)).toBe('$0.00');
  });

  it('shows a tiny non-zero amount as <$0.01', () => {
    expect(formatDollars(0.01)).toBe('<$0.01');
    expect(formatDollars(0.04)).toBe('<$0.01');
  });

  it('adds thousands separators', () => {
    expect(formatDollars(12345)).toBe('$1,234.50');
  });

  it('does not invent an amount for a missing or invalid value', () => {
    expect(formatDollars(null)).toBe('—');
    expect(formatDollars(undefined)).toBe('—');
    expect(formatDollars(Number.NaN)).toBe('—');
  });

  it('prices a top-up grant at exactly $0.10 per credit', () => {
    expect(creditsForDollars(25)).toBe(250);
    expect(formatCreditGrant(250)).toBe('250 credits');
    expect(formatCreditGrant(1)).toBe('1 credit');
  });
});
