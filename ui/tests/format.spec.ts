import { expect, test } from '@playwright/test';
import { compact, money, number } from '../src/format';

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
