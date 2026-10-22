/** @jsxImportSource react */
import { Aperture, ArrowsClockwise, CheckCircle, CircleNotch, Info, Pulse, WarningCircle } from '@phosphor-icons/react';
import type { ReactNode } from 'react';
import { useState } from 'react';
import { number, tokens } from '../format';
import type { Summary } from '../types';

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
export function StatusBadge({ status, failed = false }: { status: number | null; failed?: boolean }) {
  if (failed) return <span className="status-badge is-error"><WarningCircle size={13} weight="fill" />{status != null && status >= 400 ? status : 'Transport failed'}</span>;
  if (status == null) return <span className="status-badge is-unknown"><Info size={13} />Unknown</span>;
  const error = status >= 400 || status === 0;
  return <span className={`status-badge ${error ? 'is-error' : ''}`}>{error ? <WarningCircle size={13} weight="fill" /> : <CheckCircle size={13} weight="fill" />}{status || 'Failed'}</span>;
}
export function SectionHeading({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return <div className="section-heading"><div><h2>{title}</h2>{description && <p>{description}</p>}</div>{action}</div>;
}
export function TokenMetric({ summary }: { summary: Summary }) {
  const inputCoverage = summary.input_tokens_known_requests;
  const outputCoverage = summary.output_tokens_known_requests;
  const coverageKnown = inputCoverage != null && outputCoverage != null;
  const complete = coverageKnown && inputCoverage === summary.request_count && outputCoverage === summary.request_count;
  const overflow = summary.input_tokens_overflow || summary.output_tokens_overflow;
  const total = summary.input_tokens == null && summary.output_tokens == null
    ? null : (summary.input_tokens ?? 0) + (summary.output_tokens ?? 0);
  const approximate = total != null && !Number.isSafeInteger(total);
  return <div className={`metric${overflow ? ' metric-token-overflow' : approximate ? ' metric-token-approximate' : ''}`} role="group" aria-label="Token usage">
    <span className="metric-label">{complete ? 'Reported tokens' : 'Known tokens'}<Pulse size={14} /></span>
    <span className="metric-value">{overflow ? 'Total exceeds range' : tokens(total, true)}</span>
    <span className="metric-foot">{summary.input_tokens_overflow ? 'Exceeds range' : tokens(summary.input_tokens, true)} in / {summary.output_tokens_overflow ? 'Exceeds range' : tokens(summary.output_tokens, true)} out</span>
    <span className="metric-foot metric-coverage">{coverageKnown ? <>
      <span>Input: {number(inputCoverage)} / {number(summary.request_count)} requests</span>
      <span>Output: {number(outputCoverage)} / {number(summary.request_count)} requests</span>
    </> : 'Usage coverage unavailable'}</span>
  </div>;
}

export function Definition({ value }: { value: unknown }) {
  return <pre className="json-view">{JSON.stringify(value, null, 2) ?? 'Not retained'}</pre>;
}

export function LazyDetails({ summary, children, className }: { summary: ReactNode; children: ReactNode; className?: string }) {
  const [open, setOpen] = useState(false);
  return <details className={className} onToggle={event => setOpen(event.currentTarget.open)}><summary>{summary}</summary>{open && children}</details>;
}
