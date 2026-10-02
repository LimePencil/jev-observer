const values = Array.from({ length: 1000 }, (_, i) => (i % 9 === 0 ? 0 : (i + .125) * 10 ** (i % 9 - 5)));
const moneyDigits = value => value > 0 && value < .01 ? 6 : value > 0 && value < 1 ? 4 : 2;
const old = [
  value => new Intl.NumberFormat('en-US').format(value),
  value => new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(value),
  value => value > 0 && value < .000001 ? '<$0.000001' : new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: moneyDigits(value), maximumFractionDigits: moneyDigits(value) }).format(value),
];
const integers = new Intl.NumberFormat('en-US');
const compact = new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 });
const dollars = Object.fromEntries([2, 4, 6].map(digits => [digits, new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: digits, maximumFractionDigits: digits })]));
const cached = [value => integers.format(value), value => compact.format(value), value => value > 0 && value < .000001 ? '<$0.000001' : dollars[moneyDigits(value)].format(value)];
for (const value of values) for (let i = 0; i < old.length; i++) if (old[i](value) !== cached[i](value)) throw new Error('Different output');
function bench(formatters) {
  let chars = 0;
  const started = performance.now();
  for (let repeat = 0; repeat < 10; repeat++) for (const value of values) for (const format of formatters) chars += format(value).length;
  return { milliseconds: Math.round(performance.now() - started), chars };
}
console.log(JSON.stringify({ fixtureValues: values.length, formattingCalls: 30000, before: bench(old), after: bench(cached) }));
