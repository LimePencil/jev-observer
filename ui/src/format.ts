const numberFormat = new Intl.NumberFormat('en-US');
const compactFormat = new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 });
const moneyFormat = (digits: number) => new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: digits, maximumFractionDigits: digits });
const dollars = moneyFormat(2), fractionalDollars = moneyFormat(4), fractionalCents = moneyFormat(6);

export const number = (value: number | null | undefined) => value == null ? 'Unknown' : numberFormat.format(value);
export const compact = (value: number | null | undefined) => value == null ? 'Unknown' : compactFormat.format(value);
export const money = (value: number | null | undefined) => value == null ? 'Unknown' : value > 0 && value < 0.000001 ? '<$0.000001' : (value > 0 && value < 0.01 ? fractionalCents : value > 0 && value < 1 ? fractionalDollars : dollars).format(value);
export const ms = (value: number | null | undefined) => value == null ? 'Unknown' : value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${Number(value.toFixed(1))} ms`;
export const percent = (value: number | null | undefined) => value == null ? 'Unknown' : `${(value * 100).toFixed(1)}%`;
const formattedDate = (value: number | null | undefined, options: Intl.DateTimeFormatOptions) => {
  if (value == null) return 'Unknown';
  if (!Number.isInteger(value)) return 'Invalid timestamp';
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toLocaleString([], { ...options, ...(options.year && date.getFullYear() <= 0 ? { era: 'short' as const } : {}) }) : 'Date out of range';
};
export const time = (value: number | null | undefined) => formattedDate(value, { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
export const dateTime = (value: number | null | undefined) => formattedDate(value, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3, timeZoneName: 'shortOffset', hour12: false });
export const bytes = (value: number) => value >= 1024 * 1024 ? `${(value / (1024 * 1024)).toFixed(1)} MiB` : value >= 1024 ? `${Math.round(value / 1024)} KiB` : `${value} B`;
export const shortId = (id: string | null | undefined) => id ? id.replace(/^(?:presentation|group|def|candidate|family)_v\d+_/, '').slice(0, 10) : 'Unknown';
export const answerValue = (value: unknown, kind: string) => value == null ? 'Unknown' : kind === 'noul' && typeof value === 'number' ? percent(value) : typeof value === 'number' ? String(Number(value.toFixed(3))) : String(value);

export const costBasis = (value: string | null | undefined) => value === 'provider_reported' ? 'Provider-reported charge' : value === 'configured_estimate' || value === 'configured_rates' || value === 'configured-estimate' || value === 'estimated' ? 'Configured estimate' : value === 'synthetic' ? 'Synthetic sample' : value || 'Unknown';
// HTML date controls use unsigned extended years; JavaScript's ISO parser
// requires a sign and six digits for years beyond the usual four digits.
const htmlDateToIso = (value: string) => value.replace(/^(\d{5,})-/, (_, year: string) => `+${year.padStart(6, '0')}-`);
export const dateInput = (value: string | undefined) => {
  if (!value || !Number.isFinite(Number(value))) return '';
  const date = new Date(Number(value));
  if (!Number.isFinite(date.getTime()) || date.getFullYear() < 1) return '';
  const pad = (part: number) => String(part).padStart(2, '0');
  const local = `${String(date.getFullYear()).padStart(4, '0')}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  // The native control also bounds its timezone-free calendar value. A valid
  // epoch at the Date limit can have an unrepresentable local calendar value.
  return Number.isFinite(new Date(`${htmlDateToIso(local)}Z`).getTime()) ? local : '';
};
export const dateFilter = (value: string) => {
  if (!value) return '';
  const timestamp = new Date(htmlDateToIso(value)).getTime();
  return Number.isFinite(timestamp) ? String(timestamp) : '';
};
// Axis midpoints may be fractional even when the stored endpoints are integer milliseconds.
export const chartTime = (value: number, showDate: boolean) => formattedDate(Math.trunc(value), { ...(showDate ? { month: 'short', day: 'numeric' } as const : {}), hour: '2-digit', minute: '2-digit', hour12: false });

export const storageHint = (category: string) => ({
  database_busy: 'The database was busy. Check whether another Observer process is using this history.',
  storage_full: 'Storage was full. Free disk space on the volume holding your history.',
  permission_denied: 'Storage access was denied. Check permissions for the database directory.',
  database_unreadable: 'History could not be read. Check the saved database key and restore a verified backup if needed.',
  storage_io: 'A storage read or write failed. Check the disk and database directory.',
  database_error: 'A database operation failed. Check the Observer terminal for details.',
}[category] ?? 'Check the Observer terminal for storage details.');
