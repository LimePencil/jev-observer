/** @jsxImportSource react */
import { ArrowRight } from '@phosphor-icons/react';
import { useEffect, useId, useRef, useState } from 'react';
import { choiceLabel, typedChoiceLabel } from '../decisions';
import { chartTime, compact, dateTime, money, ms, number } from '../format';
import type { Group, TimelineBucket, TimelineMeta } from '../types';

export function Timeline({ data, meta, metric = 'requests', compact: small = false }: { data: TimelineBucket[]; meta?: TimelineMeta; metric?: 'requests' | 'latency' | 'cost'; compact?: boolean }) {
  const chartId = useId();
  const [active, setActive] = useState<number | null>(null);
  const chart = useRef<SVGSVGElement>(null);
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  const get = (point: TimelineBucket) => metric === 'latency' ? point.mean_latency_ms : metric === 'cost' ? point.cost_usd : point.requests;
  const values = data.map(get);
  const valid = values.filter((value): value is number => value !== null);
  const hasData = data.length > 0 && valid.length > 0;
  useEffect(() => {
    if (!chart.current) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      // Hidden charts keep the last usable geometry until they become visible.
      if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return;
      setSize(previous => previous?.width === width && previous.height === height ? previous : { width, height });
    });
    observer.observe(chart.current);
    return () => observer.disconnect();
  }, [hasData]);
  if (!hasData) return <div className="chart-empty">{metric === 'cost' ? 'No known cost in this window' : 'Activity will appear here as requests are recorded'}</div>;
  const pad = metric === 'cost' ? 76 : 36;
  const width = Math.max(size?.width ?? 900, pad + 9), height = Math.max(size?.height ?? (small ? 120 : 170), 37);
  const largest = Math.max(...valid);
  const max = largest > 0 ? largest : 1;
  const plotWidth = width - pad - 8;
  const first = meta?.start ?? Math.min(...data.map(point => point.timestamp));
  const last = meta?.end ?? Math.max(...data.map(point => point.timestamp));
  const bucketWidth = meta?.bucket_width ?? (data.length > 1 ? Math.min(...data.slice(1).map((point, index) => point.timestamp - data[index].timestamp).filter(value => value > 0)) : 60_000);
  const span = Math.max(last - first + (meta ? 0 : bucketWidth), 1);
  const step = Math.min(plotWidth, plotWidth * bucketWidth / span);
  const showDate = span >= 86_400_000 || new Date(first).toDateString() !== new Date(last).toDateString();
  const barWidth = Math.max(1, Math.min(22, step * .55));
  const y = (value: number) => height - 22 - value / max * (height - 36);
  const label = (value: number | null) => metric === 'latency' ? ms(value) : metric === 'cost' ? money(value) : number(value);
  const activePoint = active === null ? null : data[active];
  return <div className="timeline-wrap">
    <svg ref={chart} className="timeline-chart" viewBox={`0 0 ${width} ${height}`} role="img" aria-labelledby={chartId}>
      <title id={chartId}>{metric === 'requests' ? 'Requests and failures' : metric === 'latency' ? 'Mean request latency' : 'Known request cost'} over time. An accessible data table follows.</title>
      {[0, .5, 1].map(fraction => <g key={fraction}><line x1={pad} x2={width} y1={y(fraction * max)} y2={y(fraction * max)} className="chart-grid" /><text x={pad - 9} y={y(fraction * max) + 4} textAnchor="end" className="chart-axis">{metric === 'cost' ? money(fraction * max) : compact(fraction * max)}</text></g>)}
      {data.map((point, index) => {
        const value = get(point);
        const left = Math.max(first, point.timestamp), right = Math.min(first + span, point.timestamp + bucketWidth);
        const pointStep = Math.max(0, right - left) / span * plotWidth;
        const x = pad + ((left + right) / 2 - first) / span * plotWidth;
        const pointWidth = Math.min(barWidth, pointStep);
        return <g key={`${point.timestamp}-${index}`} onMouseEnter={() => setActive(index)} onMouseLeave={() => setActive(null)}>
          <rect x={x - pointStep / 2} y={0} width={pointStep} height={height - 16} fill="transparent" />
          {value !== null && <rect x={x - pointWidth / 2} y={y(value)} width={pointWidth} height={Math.max(value > 0 ? 2 : 0, y(0) - y(value))} rx={2} className={`chart-bar ${active === index ? 'active' : ''}`} />}
          {metric === 'requests' && point.errors > 0 && <rect x={x - pointWidth / 2} y={y(point.errors)} width={pointWidth} height={Math.max(2, y(0) - y(point.errors))} rx={2} className="chart-error" />}
          <title>{dateTime(point.timestamp)}: {label(value)}{metric === 'requests' ? ` requests, ${number(point.errors)} failures` : ''}</title>
        </g>;
      })}
      {(plotWidth < 400 ? [0, 1] : [0, .5, 1]).map(fraction => <text key={fraction} x={pad + fraction * plotWidth} y={height - 2} textAnchor={fraction === 0 ? 'start' : fraction === 1 ? 'end' : 'middle'} className="chart-axis">{chartTime(first + fraction * span, showDate)}</text>)}
    </svg>
    {activePoint && <div className="chart-tooltip" aria-hidden="true"><span>{dateTime(activePoint.timestamp)}</span><strong>{label(get(activePoint))}{metric === 'requests' ? ' requests' : ''}</strong>{metric === 'requests' && <span>{number(activePoint.errors)} failures</span>}</div>}
    {meta?.truncated && <p className="caption">Only part of this time range is displayed.</p>}
    <details className="chart-accessible"><summary>View chart data</summary><div className="table-scroll"><table><thead><tr><th>Time</th><th>Requests</th><th>Failures</th><th>Mean latency</th><th>Known cost</th></tr></thead><tbody>{data.map((point, index) => <tr key={`${point.timestamp}-${index}`}><td>{dateTime(point.timestamp)}</td><td>{number(point.requests)}</td><td>{number(point.errors)}</td><td>{ms(point.mean_latency_ms)}</td><td>{money(point.cost_usd)}</td></tr>)}</tbody></table></div></details>
  </div>;
}

export function Distribution({ group }: { group: Group }) {
  const [expanded, setExpanded] = useState(false);
  const distribution = group.distribution ?? [];
  if (group.kind === 'box') return <p className="caption box-distribution-note">Box answers retain individual geometry. Inspect observations for coordinates and reported confidence; no scalar mean or histogram is computed.</p>;
  const label = (item: Group['distribution'][number]) => typeof item.value === 'string' || typeof item.value === 'boolean' ? typedChoiceLabel(item.value) : choiceLabel(item.key ?? item.label, group.definition);
  if (!group.valid_count || !distribution.length) return <div className="chart-empty">No valid answers in this window</div>;
  const sorted = [...distribution].sort(group.kind === 'choice' ? (a, b) => b.count - a.count : (a, b) => parseFloat(a.label) - parseFloat(b.label));
  const items = sorted.slice(0, expanded ? sorted.length : group.kind === 'choice' ? 8 : 20);
  const total = distribution.reduce((sum, item) => sum + item.count, 0);
  return <div className={`distribution ${expanded ? 'expanded' : ''}`} aria-label={`${group.name} answer distribution`}>
    {items.map((item, index) => <div className="distribution-row" key={item.key ?? item.label}><div className="distribution-label"><span className="truncate" title={label(item)}>{label(item)}</span><span className="mono">{total ? (item.count / total * 100).toFixed(1) : 0}% <span className="subtle">· {number(item.count)}</span></span></div><div className="distribution-track"><div className={`distribution-fill color-${index % 4}`} style={{ width: `${total ? item.count / total * 100 : 0}%` }} /></div></div>)}
    {sorted.length > items.length && <button className="text-button" onClick={() => setExpanded(true)}>Show all {number(sorted.length)} values <ArrowRight size={13} /></button>}
  </div>;
}
