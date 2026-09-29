/**
 * Credits are the backend's unit; dollars are what people see.
 *
 * 1 credit = $0.10 (100_000 micro-USD), so dollars = credits / 10. This is the
 * one place that conversion happens. Everything that shows a balance, a cost or
 * a receipt amount to a user goes through `formatDollars`. The only user-facing
 * place a raw credit count is allowed is the top-up purchase, which states the
 * grant next to the dollars paid (`formatCreditGrant`).
 */
export const CREDITS_PER_USD = 10;

const USD = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

/** "$1.23"; "<$0.01" for a non-zero amount under a cent; "—" for a non-number. */
export function formatDollars(credits: number | null | undefined): string {
  if (typeof credits !== 'number' || !Number.isFinite(credits)) return '—';
  const cents = Math.round((credits / CREDITS_PER_USD) * 100);
  if (credits > 0 && cents < 1) return '<$0.01';
  if (credits < 0 && cents > -1) return '-<$0.01';
  return USD.format(cents / 100);
}

/** Credits a dollar top-up grants: exactly $0.10 per credit. */
export function creditsForDollars(amountUsd: number): number {
  return Math.round(amountUsd * CREDITS_PER_USD);
}

/** "250 credits" -- for the purchase page only, next to the dollars paid. */
export function formatCreditGrant(credits: number): string {
  return `${credits.toLocaleString('en-US')} credit${credits === 1 ? '' : 's'}`;
}
