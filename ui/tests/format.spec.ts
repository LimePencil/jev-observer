import { expect, test } from '@playwright/test';
import { chartTime, compact, dateTime, money, number, time } from '../src/format';

test('number formatting preserves unknown values, zero, and compact totals', () => {
  for (const format of [number, compact, money]) {
    expect(format(null)).toBe('Unknown');
    expect(format(undefined)).toBe('Unknown');
  }
  expect(number(0)).toBe('0');
  expect(number(1234567)).toBe('1,234,567');
  expect(compact(0)).toBe('0');
  expect(compact(1234)).toBe('1.2K');
  expect(compact(1234567)).toBe('1.2M');
});

test('cost formatting retains small known costs across precision boundaries', () => {
  for (const [value, expected] of [
    [0, '$0.00'],
    [0.0000009, '<$0.000001'],
    [0.000001, '$0.000001'],
    [0.009999, '$0.009999'],
    [0.01, '$0.0100'],
    [0.9999, '$0.9999'],
    [1, '$1.00'],
    [1234.56, '$1,234.56'],
  ] as const) expect(money(value), `Cost ${value}`).toBe(expected);
});

test('date formatting distinguishes unknown, invalid, and out-of-range legacy timestamps', () => {
  for (const format of [time, dateTime]) {
    expect(format(null)).toBe('Unknown');
    expect(format(undefined)).toBe('Unknown');
    for (const value of [NaN, Infinity, -Infinity, 1.5]) expect(format(value)).toBe('Invalid timestamp');
    for (const value of [8_640_000_000_000_001, -8_640_000_000_000_001, Number.MAX_SAFE_INTEGER]) {
      expect(format(value)).toBe('Date out of range');
    }
  }
  for (const showDate of [false, true]) {
    expect(chartTime(NaN, showDate)).toBe('Invalid timestamp');
    expect(chartTime(9_223_372_036_854_776_000, showDate)).toBe('Date out of range');
    for (const value of [-8_640_000_000_000_000, -1.5, -1, -0.5, 0, 0.5, 1, 1.5, 8_640_000_000_000_000]) {
      expect(chartTime(value, showDate)).toBe(new Date(value).toLocaleString([], {
        ...(showDate ? { month: 'short', day: 'numeric' } as const : {}), hour: '2-digit', minute: '2-digit', hour12: false,
      }));
    }
  }
  for (const value of [-8_640_000_000_000_000, -1, 0, 1, 8_640_000_000_000_000]) {
    expect(time(value)).toBe(new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }));
    expect(dateTime(value)).toBe(new Date(value).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }));
  }
});
