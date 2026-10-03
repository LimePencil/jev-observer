const numberFormat = new Intl.NumberFormat('en-US');
const compactFormat = new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 });
const moneyFormat = (digits: number) => new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: digits, maximumFractionDigits: digits });
const dollars = moneyFormat(2), fractionalDollars = moneyFormat(4), fractionalCents = moneyFormat(6);

export const number = (value: number | null | undefined) => value == null ? 'Unknown' : numberFormat.format(value);
export const compact = (value: number | null | undefined) => value == null ? 'Unknown' : compactFormat.format(value);
export const money = (value: number | null | undefined) => value == null ? 'Unknown' : value > 0 && value < 0.000001 ? '<$0.000001' : (value > 0 && value < 0.01 ? fractionalCents : value > 0 && value < 1 ? fractionalDollars : dollars).format(value);
export const ms = (value: number | null | undefined) => value == null ? 'Unknown' : value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${Number(value.toFixed(1))} ms`;
export const percent = (value: number | null | undefined) => value == null ? 'Unknown' : `${(value * 100).toFixed(1)}%`;
export const time = (value: number | null | undefined) => value == null ? 'Unknown' : new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
export const dateTime = (value: number | null | undefined) => value == null ? 'Unknown' : new Date(value).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
export const bytes = (value: number) => value >= 1024 * 1024 ? `${(value / (1024 * 1024)).toFixed(1)} MiB` : value >= 1024 ? `${Math.round(value / 1024)} KiB` : `${value} B`;
export const shortId = (id: string | null | undefined) => id ? id.replace(/^(?:presentation|group|def|candidate|family)_v\d+_/, '').slice(0, 10) : 'Unknown';
export const answerValue = (value: unknown, kind: string) => value == null ? 'Unknown' : kind === 'noul' && typeof value === 'number' ? percent(value) : typeof value === 'number' ? String(Number(value.toFixed(3))) : String(value);

export const costBasis = (value: string | null | undefined) => value === 'provider_reported' ? 'Provider-reported charge' : value === 'configured_estimate' || value === 'configured_rates' || value === 'configured-estimate' || value === 'estimated' ? 'Configured estimate' : value === 'synthetic' ? 'Synthetic sample' : value || 'Unknown';
export const dateInput = (value: string | undefined) => {
  if (!value || !Number.isFinite(Number(value))) return '';
  const date = new Date(Number(value));
  if (!Number.isFinite(date.getTime())) return '';
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
};
export const dateFilter = (value: string) => value && Number.isFinite(new Date(value).getTime()) ? String(new Date(value).getTime()) : '';
export const chartTime = (value: number, showDate: boolean) => new Date(value).toLocaleString([], { ...(showDate ? { month: 'short', day: 'numeric' } as const : {}), hour: '2-digit', minute: '2-digit', hour12: false });

export const storageHint = (category: string) => ({
  database_busy: 'The database was busy. Check whether another Observer process is using this history.',
  storage_full: 'Storage was full. Free disk space on the volume holding your history.',
  permission_denied: 'Storage access was denied. Check permissions for the database directory.',
  database_unreadable: 'History could not be read. Check the saved database key and restore a verified backup if needed.',
  storage_io: 'A storage read or write failed. Check the disk and database directory.',
  database_error: 'A database operation failed. Check the Observer terminal for details.',
}[category] ?? 'Check the Observer terminal for storage details.');
