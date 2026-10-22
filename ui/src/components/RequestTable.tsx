/** @jsxImportSource react */
import { ArrowRight, WarningCircle } from '@phosphor-icons/react';
import { costBasis, dateTime, money, ms, number, time, tokens } from '../format';
import type { RequestSummary } from '../types';

import { StatusBadge } from './primitives';

export function RequestTable({ requests, open, compact: small = false }: { requests: RequestSummary[]; open: (id: string) => void; compact?: boolean }) {
  return <div className="table-scroll"><table className={`request-table ${small ? 'compact-table' : ''}`}><thead><tr><th>Request / source</th><th>Status</th>{!small && <th>Model</th>}<th className="align-right">Latency</th>{!small && <th className="align-right">Tokens</th>}<th className="align-right">Cost</th><th>Time</th><th><span className="sr-only">Inspect</span></th></tr></thead><tbody>{requests.map(request => <tr key={request.id} className={request.failed || request.status != null && (request.status >= 400 || request.status === 0) ? 'error-row' : ''}>
    <td><button className="request-link" onClick={() => open(request.id)} aria-label={`Inspect request ${request.id}`}><span className="request-source">{request.source}{!request.capture_complete && <span className="mini-warning" title="Capture is incomplete"><WarningCircle size={13} weight="fill" /></span>}</span><span className="mono subtle request-id">{request.id.slice(0, 16)}</span></button></td>
    <td>{request.event_kind && request.event_kind !== 'request' ? <span className="status-badge is-unknown">Action</span> : <StatusBadge status={request.status} failed={request.failed} />}</td>{!small && <td><span className="model-name">{request.model || 'Unknown'}</span><span className="table-subline">{number(request.answer_count)} questions</span></td>}
    <td className="align-right mono">{ms(request.duration_ms)}</td>{!small && <td className="align-right mono">{request.input_tokens == null || request.output_tokens == null ? 'Unknown' : tokens(request.input_tokens + request.output_tokens)}</td>}
    <td className="align-right mono" title={costBasis(request.cost_basis)}>{money(request.cost_usd)}{request.cost_usd != null && <span className="table-subline cost-basis">{costBasis(request.cost_basis)}</span>}</td><td className="mono subtle"><span title={dateTime(request.timestamp ?? request.imported_at)}><span aria-hidden="true">{time(request.timestamp ?? request.imported_at)}</span><span className="sr-only">{dateTime(request.timestamp ?? request.imported_at)}</span></span>{(request.timestamp_basis === 'import' || request.timestamp == null && request.imported_at != null) && <span className="table-subline">Import time</span>}</td><td><button className="icon-button row-open" onClick={() => open(request.id)} aria-label={`Open ${request.id}`}><ArrowRight size={16} /></button></td>
  </tr>)}</tbody></table></div>;
}
