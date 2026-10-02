import { useId, useState } from 'react';
import { Aperture, ArrowRight, ArrowsClockwise, CheckCircle, CircleNotch, Info, WarningCircle } from '@phosphor-icons/react';
import type { ReactNode } from 'react';
import type { Group, RequestSummary, TimelineBucket } from './types';
import { compact, money, ms, number, time } from './format';

// Reuse the website's duotone mark and two-weight wordmark in every app shell.
export function Brand({ className = '', onNavigate }: { className?: string; onNavigate: () => void }) {
  return <a className={`brand ${className}`} href="#overview" aria-label="Jev Observer overview" onClick={event => { event.preventDefault(); onNavigate(); }}><Aperture size={28} weight="duotone" aria-hidden="true" /><span className="brand-wordmark">jev<span className="brand-wordmark-light"> observer</span></span></a>;
}

export function Empty({ title, children, action }: { title: string; children: ReactNode; action?: ReactNode }) {
  return <div className="empty-state"><div className="empty-icon"><Info size={24} weight="duotone" /></div><h3>{title}</h3><div className="empty-description">{children}</div>{action}</div>;
}
export function Loading({ label = 'Loading your workspace' }: { label?: string }) {
  return <div className="loading-state" role="status"><CircleNotch size={22} className="spinner" /><span>{label}…</span></div>;
}
export function ErrorNotice({ message, retry }: { message: string; retry?: () => void }) {
  return <div className="error-notice" role="alert"><WarningCircle size={19} weight="bold" /><span>{message}</span>{retry && <button className="text-button" onClick={retry}><ArrowsClockwise size={15} />Retry</button>}</div>;
}
export function KindBadge({ kind }: { kind: string }) { return <span className={`kind-badge kind-${kind}`}>{kind}</span>; }
export function StatusBadge({ status }: { status: number | null }) {
  if (status == null) return <span className="status-badge is-unknown"><Info size={13} />Unknown</span>;
  const error = status >= 400 || status === 0;
  return <span className={`status-badge ${error ? 'is-error' : ''}`}>{error ? <WarningCircle size={13} weight="fill" /> : <CheckCircle size={13} weight="fill" />}{status || 'Failed'}</span>;
}
export function SectionHeading({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return <div className="section-heading"><div><h2>{title}</h2>{description && <p>{description}</p>}</div>{action}</div>;
}
export function Definition({ value }: { value: unknown }) {
  return <pre className="json-view">{JSON.stringify(value, null, 2) ?? 'Not retained'}</pre>;
}

export function Timeline({ data, metric = 'requests', compact: small = false }: { data: TimelineBucket[]; metric?: 'requests' | 'latency' | 'cost'; compact?: boolean }) {
  const chartId = useId();
  const [active, setActive] = useState<number | null>(null);
  const width = 900, height = small ? 120 : 170, pad = metric === 'cost' ? 76 : 36;
  const get = (point: TimelineBucket) => metric === 'latency' ? point.mean_latency_ms : metric === 'cost' ? point.cost_usd : point.requests;
  const values = data.map(get);
  const valid = values.filter((value): value is number => value !== null);
  if (!data.length || !valid.length) return <div className="chart-empty">{metric === 'cost' ? 'No known cost in this window' : 'Activity will appear here as requests are recorded'}</div>;
  const largest = Math.max(...valid);
  const max = largest > 0 ? largest : 1;
  const plotWidth = width - pad - 8;
  const step = plotWidth / Math.max(data.length, 1);
  const barWidth = Math.max(1, Math.min(22, step * .55));
  const y = (value: number) => height - 22 - value / max * (height - 36);
  const label = (value: number | null) => metric === 'latency' ? ms(value) : metric === 'cost' ? money(value) : number(value);
  const activePoint = active === null ? null : data[active];
  return <div className="timeline-wrap">
    <svg className="timeline-chart" viewBox={`0 0 ${width} ${height}`} role="img" aria-labelledby={chartId}>
      <title id={chartId}>{metric === 'requests' ? 'Requests and failures' : metric === 'latency' ? 'Mean request latency' : 'Known request cost'} over time. An accessible data table follows.</title>
      {[0, .5, 1].map(fraction => <g key={fraction}><line x1={pad} x2={width} y1={y(fraction * max)} y2={y(fraction * max)} className="chart-grid" /><text x={pad - 9} y={y(fraction * max) + 4} textAnchor="end" className="chart-axis">{metric === 'cost' ? money(fraction * max) : compact(fraction * max)}</text></g>)}
      {data.map((point, index) => {
        const value = get(point), x = pad + step * index + step / 2;
        return <g key={`${point.timestamp}-${index}`} onMouseEnter={() => setActive(index)} onMouseLeave={() => setActive(null)}>
          <rect x={x - step / 2} y={0} width={step} height={height - 16} fill="transparent" />
          {value !== null && <rect x={x - barWidth / 2} y={y(value)} width={barWidth} height={Math.max(value > 0 ? 2 : 0, y(0) - y(value))} rx={2} className={`chart-bar ${active === index ? 'active' : ''}`} />}
          {metric === 'requests' && point.errors > 0 && <rect x={x - barWidth / 2} y={y(point.errors)} width={barWidth} height={Math.max(2, y(0) - y(point.errors))} rx={2} className="chart-error" />}
          <title>{time(point.timestamp)}: {label(value)}{metric === 'requests' ? ` requests, ${number(point.errors)} failures` : ''}</title>
        </g>;
      })}
      {data.filter((_, index) => index === 0 || index === Math.floor((data.length - 1) / 2) || index === data.length - 1).map((point, index, all) => <text key={`${point.timestamp}-${index}`} x={index === 0 ? pad : index === all.length - 1 ? width - 8 : width / 2} y={height - 2} textAnchor={index === 0 ? 'start' : index === all.length - 1 ? 'end' : 'middle'} className="chart-axis">{new Date(point.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })}</text>)}
    </svg>
    {activePoint && <div className="chart-tooltip" aria-hidden="true"><span>{time(activePoint.timestamp)}</span><strong>{label(get(activePoint))}{metric === 'requests' ? ' requests' : ''}</strong>{metric === 'requests' && <span>{number(activePoint.errors)} failures</span>}</div>}
    <details className="chart-accessible"><summary>View chart data</summary><div className="table-scroll"><table><thead><tr><th>Time</th><th>Requests</th><th>Failures</th><th>Mean latency</th><th>Known cost</th></tr></thead><tbody>{data.map((point, index) => <tr key={`${point.timestamp}-${index}`}><td>{time(point.timestamp)}</td><td>{number(point.requests)}</td><td>{number(point.errors)}</td><td>{ms(point.mean_latency_ms)}</td><td>{money(point.cost_usd)}</td></tr>)}</tbody></table></div></details>
  </div>;
}

export function Distribution({ group }: { group: Group }) {
  const [expanded, setExpanded] = useState(false);
  const distribution = group.distribution ?? [];
  if (!group.valid_count || !distribution.length) return <div className="chart-empty">No valid answers in this window</div>;
  const sorted = [...distribution].sort(group.kind === 'choice' ? (a, b) => b.count - a.count : (a, b) => parseFloat(a.label) - parseFloat(b.label));
  const items = sorted.slice(0, expanded ? sorted.length : group.kind === 'choice' ? 8 : 20);
  const total = distribution.reduce((sum, item) => sum + item.count, 0);
  return <div className={`distribution ${expanded ? 'expanded' : ''}`} aria-label={`${group.name} answer distribution`}>
    {items.map((item, index) => <div className="distribution-row" key={item.label}><div className="distribution-label"><span className="truncate" title={item.label}>{item.label}</span><span className="mono">{total ? (item.count / total * 100).toFixed(1) : 0}% <span className="subtle">· {number(item.count)}</span></span></div><div className="distribution-track"><div className={`distribution-fill color-${index % 4}`} style={{ width: `${total ? item.count / total * 100 : 0}%` }} /></div></div>)}
    {sorted.length > items.length && <button className="text-button" onClick={() => setExpanded(true)}>Show all {number(sorted.length)} values <ArrowRight size={13} /></button>}
  </div>;
}

export function RequestTable({ requests, open, compact: small = false }: { requests: RequestSummary[]; open: (id: string) => void; compact?: boolean }) {
  return <div className="table-scroll"><table className={`request-table ${small ? 'compact-table' : ''}`}><thead><tr><th>Request / source</th><th>Status</th>{!small && <th>Model</th>}<th className="align-right">Latency</th>{!small && <th className="align-right">Tokens</th>}<th className="align-right">Cost</th><th>Time</th><th><span className="sr-only">Inspect</span></th></tr></thead><tbody>{requests.map(request => <tr key={request.id} className={request.status != null && (request.status >= 400 || request.status === 0) ? 'error-row' : ''}>
    <td><button className="request-link" onClick={() => open(request.id)} aria-label={`Inspect request ${request.id}`}><span className="request-source">{request.source}{!request.capture_complete && <span className="mini-warning" title="Capture is incomplete"><WarningCircle size={13} weight="fill" /></span>}</span><span className="mono subtle request-id">{request.id.slice(0, 16)}</span></button></td>
    <td>{request.event_kind && request.event_kind !== 'request' ? <span className="status-badge is-unknown">Action</span> : <StatusBadge status={request.status} />}</td>{!small && <td><span className="model-name">{request.model || 'Unknown'}</span><span className="table-subline">{number(request.answer_count)} questions</span></td>}
    <td className="align-right mono">{ms(request.duration_ms)}</td>{!small && <td className="align-right mono">{request.input_tokens == null || request.output_tokens == null ? 'Unknown' : number(request.input_tokens + request.output_tokens)}</td>}
    <td className="align-right mono" title={request.cost_basis || 'No cost basis available'}>{money(request.cost_usd)}</td><td className="mono subtle">{time(request.timestamp ?? request.imported_at)}{(request.timestamp_basis === 'import' || request.timestamp == null && request.imported_at != null) && <span className="table-subline">Import time</span>}</td><td><button className="icon-button row-open" onClick={() => open(request.id)} aria-label={`Open ${request.id}`}><ArrowRight size={16} /></button></td>
  </tr>)}</tbody></table></div>;
}
