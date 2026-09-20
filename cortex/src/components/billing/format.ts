export function formatMoney(cents: number | null | undefined) {
  if (typeof cents !== 'number') return 'TBD';
  return new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(cents / 100);
}

export function formatDate(value: string | null | undefined) {
  if (!value) return 'Not scheduled';
  return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric' }).format(new Date(value));
}
