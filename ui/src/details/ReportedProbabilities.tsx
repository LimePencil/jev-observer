import { useState } from 'react';
import { number, percent } from '../format';

const PROBABILITIES_PER_PAGE = 25;
type Probability = { key: string; label: string; value: number | null };

export function ReportedProbabilities({ values, answerKey }: { values: Probability[]; answerKey: string }) {
  const [selectedPage, setSelectedPage] = useState(0);
  const page = Math.min(selectedPage, Math.max(0, Math.ceil(values.length / PROBABILITIES_PER_PAGE) - 1));
  const start = page * PROBABILITIES_PER_PAGE;
  const end = Math.min(start + PROBABILITIES_PER_PAGE, values.length);
  return <div className="probability-list" aria-label="Reported probabilities">
    {values.length > PROBABILITIES_PER_PAGE && <nav className="history-pagination" aria-label={`Probability pages for ${answerKey}`}>
      <button className="text-button" aria-disabled={page === 0} onClick={() => { if (page > 0) setSelectedPage(page - 1); }}>Previous probabilities</button>
      <span role="status">Probabilities {number(start + 1)}–{number(end)} of {number(values.length)}</span>
      <button className="text-button" aria-disabled={end === values.length} onClick={() => { if (end < values.length) setSelectedPage(page + 1); }}>Next probabilities</button>
    </nav>}
    {values.slice(start, end).map(({ key, label, value }) => <div key={key} className="probability-row">
      <span title={label}>{label}</span>
      <div className="probability-track">{value !== null && <span style={{ width: `${value * 100}%` }} />}</div>
      <span className="mono">{value === null ? 'Invalid' : percent(value)}</span>
    </div>)}
  </div>;
}
