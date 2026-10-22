import { ArrowRight, Fingerprint, Info, ShieldCheck, WarningCircle } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { useResource } from '../api';
import { Definition, Empty, ErrorNotice, KindBadge, LazyDetails, Loading, StatusBadge } from '../components';
import { costBasis, dateTime, money, ms, number, percent, shortId, tokens } from '../format';
import { BoxValue } from '../components/BoxValue';
import { boxGeometry, choiceLabel, decisionValue } from '../decisions';
import type { ReviewController, ReviewState } from '../reviews';
import type { Answer, Label, RequestRecord } from '../types';

import { DetailHeading, Field } from './shared';
import type { DetailProps } from './types';

const DECISIONS_PER_PAGE = 50;
const emptyReview: ReviewState = { pending: null, error: '', savedLabel: null };

export function RequestDetails({ id, setPanel, controller, historyRevision }: { id: string; setPanel: DetailProps['setPanel']; controller: ReviewController; historyRevision: number }) {
  const { data: record, error, reload, update } = useResource<RequestRecord>(`/api/requests/${encodeURIComponent(id)}`, historyRevision);
  const [answerPage, setAnswerPage] = useState(0);
  const reviews = controller.states.get(id);
  const latest = useRef({ historyRevision, reload, update });
  latest.current = { historyRevision, reload, update };
  useEffect(() => controller.register(id, (key, label, startedRevision) => {
    // A history refresh can read labels before this save commits. Otherwise
    // update the current drawer in place, even if the original drawer closed.
    const current = latest.current;
    if (current.historyRevision !== startedRevision) current.reload();
    else current.update(previous => previous.id !== id ? previous : { ...previous, labels: [...previous.labels.filter(item => (item.key ?? item.answer_key) !== key), { key, label }] });
  }), [id, controller.register]);
  const pendingReviews = [...(reviews?.values() ?? [])].some(review => review.pending !== null);
  const page = Math.min(answerPage, Math.max(0, Math.ceil((record?.answers.length ?? 0) / DECISIONS_PER_PAGE) - 1));
  const answerStart = page * DECISIONS_PER_PAGE;
  return <>
    <DetailHeading eyebrow="Request details" title={record?.source ?? 'Request'} description="Inspect the captured response and its individual decisions." icon={<Fingerprint size={15} />} />
    {error ? <ErrorNotice message={error} retry={reload} /> : !record ? <Loading label="Loading request" /> : <div className="drawer-body">
      <div className="request-detail-id"><code>{record.id}</code><StatusBadge status={record.status} failed={record.failed} /></div>
      {record.sample && <div className="inline-notice"><Info size={16} />Synthetic sample. No provider was called.</div>}
      {!record.capture_complete && <div className="warning-notice"><WarningCircle size={18} />This capture is incomplete. Missing fields are unknown; the forwarded request was not truncated.</div>}
      {record.transport_error && <ErrorNotice message={record.transport_error} />}
      {typeof record.normalization_error === 'string' && record.normalization_error.trim() !== '' && <ErrorNotice message={record.normalization_error} />}
      <dl className="detail-grid"><Field label={record.timestamp == null ? "Imported at" : "Recorded"}>{dateTime(record.timestamp ?? record.imported_at)}{record.timestamp == null && <span className="table-subline">Original event time unknown</span>}</Field><Field label="Latency">{ms(record.duration_ms)}</Field><Field label="Provider">{record.provider || 'Unknown'}</Field><Field label="Protocol">{record.protocol ?? 'systemone'}</Field>{record.model_profile && <Field label="Model profile">{record.model_profile}</Field>}<Field label="Model">{record.model || 'Unknown'}</Field><Field label="Requested model">{record.requested_model ?? 'Unknown'}</Field><Field label="Input tokens">{tokens(record.input_tokens)}</Field><Field label="Output tokens">{tokens(record.output_tokens)}</Field><Field label="Request cost">{money(record.cost_usd)}</Field><Field label="Cost basis">{costBasis(record.cost_basis)}</Field></dl>
      <div className="detail-section-title"><h3>Decisions</h3><span className="count-badge">{record.answers.length}</span></div>
      <p className="caption">Usage and cost belong to this request, across all its answers.</p>
      {record.answers.length > DECISIONS_PER_PAGE && <nav className="history-pagination" aria-label="Decision pages"><button className="text-button" aria-disabled={page === 0 || pendingReviews} onClick={() => { if (page > 0 && !pendingReviews) setAnswerPage(page - 1); }}>Previous decisions</button><span role="status">Decisions {number(answerStart + 1)}–{number(Math.min(answerStart + DECISIONS_PER_PAGE, record.answers.length))} of {number(record.answers.length)}{pendingReviews && ' · Saving reviews…'}</span><button className="text-button" aria-disabled={answerStart + DECISIONS_PER_PAGE >= record.answers.length || pendingReviews} onClick={() => { if (answerStart + DECISIONS_PER_PAGE < record.answers.length && !pendingReviews) setAnswerPage(page + 1); }}>Next decisions</button></nav>}
      {!record.answers.length ? <Empty title="No captured answers">The provider response did not include a supported answer, or the capture was incomplete.</Empty> : record.answers.slice(answerStart, answerStart + DECISIONS_PER_PAGE).map(answer => <AnswerDetail key={`${record.id}-${answer.key}`} answer={answer} record={record} onGroup={() => setPanel({ type: 'group', id: answer.group_id })} review={reviews?.get(answer.key) ?? emptyReview} onReview={label => void controller.save(id, answer.key, label)} />)}
      <section className="detail-section"><h3>Request input state</h3><div className="inline-notice"><ShieldCheck size={18} weight="bold" />{record.state_retained ? 'Captured with local opt-in and configured redaction.' : 'This record has no retained request state.'}</div>{record.import_format && !!record.actions?.length && <p className="caption">Imported application data is shown under Application outcome.</p>}{record.state_retained && <LazyDetails summary="Inspect retained input"><Definition value={record.state} /></LazyDetails>}</section>
      <section className="detail-section"><h3>Application outcome</h3>{record.actions?.length ? <Definition value={record.actions} /> : <p className="muted">Not collected. An API response does not establish whether an application action ran or succeeded.</p>}</section>
      {(record.source_event_id || record.import_format) && <section className="detail-section"><h3>Import provenance</h3><dl className="detail-grid"><Field label="Source event">{record.source_event_id ?? 'Not supplied'}</Field><Field label="Format">{record.import_format ?? 'Native capture'}</Field></dl></section>}
    </div>}
  </>;
}

function AnswerDetail({ answer, record, onGroup, review: { pending, error, savedLabel }, onReview }: { answer: Answer; record: RequestRecord; onGroup: () => void; review: ReviewState; onReview: (label: Label) => void }) {
  const currentLabel = [...(record.labels ?? [])].reverse().find(item => (item.key ?? item.answer_key) === answer.key)?.label;
  // A successful response may belong to a row deleted before it arrived.
  const saved = savedLabel !== null && savedLabel === currentLabel;
  const probabilities = Object.entries(answer.probabilities ?? {}).map(([key, value]) => ({
    key, value: typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1 ? value : null,
  })).sort((a, b) => (b.value ?? -1) - (a.value ?? -1));
  const invalidProbabilities = probabilities.some(item => item.value === null);
  const invalidGeometry = answer.kind === 'box' && !boxGeometry(answer.value);
  return <section className="answer-card"><div className="answer-heading"><h4>{answer.key}</h4><KindBadge kind={answer.kind} /></div>
    <div className={`answer-value${answer.kind === 'box' ? ' answer-value-box' : ''}`}>{answer.kind === 'box' && answer.valid ? <BoxValue value={answer.value} found={answer.box_found} present={answer.box_present} /> : answer.valid ? decisionValue(answer.value, answer.kind, answer.choice_key) : 'Invalid or missing answer'}{answer.kind === 'noul' && answer.valid && <span className="caption">yes-probability</span>}</div>
    {!answer.valid && <p className="error-text">{answer.error ?? 'Excluded from answer distributions.'}</p>}
    {answer.warnings?.map(warning => <div className="warning-notice" key={warning}><WarningCircle size={17} /><span>{warning}</span></div>)}
    {answer.confidence != null && <p className="caption">Reported confidence {percent(answer.confidence)} · not measured accuracy</p>}
    {probabilities.length > 0 && <div className="probability-list" aria-label="Reported probabilities">{probabilities.map(({ key, value }) => <div key={key} className="probability-row"><span title={choiceLabel(key, answer.definition, record.protocol === 'openai-decisions')}>{choiceLabel(key, answer.definition, record.protocol === 'openai-decisions')}</span><div className="probability-track">{value !== null && <span style={{ width: `${value * 100}%` }} />}</div><span className="mono">{value === null ? 'Invalid' : percent(value)}</span></div>)}</div>}
    {(!answer.valid || invalidProbabilities || invalidGeometry || answer.kind === 'box' || Boolean(answer.choice_key)) && <LazyDetails className="raw-answer-details" summary="Inspect raw answer"><Definition value={answer.raw_answer} /></LazyDetails>}
    <div className="answer-actions"><button className="text-button" onClick={onGroup}>View recurring group <ArrowRight size={14} /></button><div className="review-control"><span className="review-status" role="status">{pending ? 'Saving…' : saved ? 'Saved' : ''}</span><label className="label-control"><span>Review</span><select aria-label={`Review ${answer.key}`} value={pending ?? currentLabel ?? ''} aria-disabled={pending !== null} onPointerDown={event => { if (pending) event.preventDefault(); }} onKeyDown={event => { if (pending && ['ArrowUp', 'ArrowDown', 'Home', 'End', ' ', 'Enter'].includes(event.key)) event.preventDefault(); }} onChange={event => { if (event.target.value && !pending) onReview(event.target.value as Label); }}><option value="" disabled>Not labeled</option><option value="correct">Correct</option><option value="incorrect">Incorrect</option><option value="unknown">Unknown</option></select></label></div></div>
    {error && <ErrorNotice message={error} />}
    <LazyDetails className="definition-details" summary="Definition and context"><dl className="detail-grid compact-fields"><Field label="Definition">{shortId(answer.definition_id)}</Field><Field label="Presentation">{shortId(answer.presentation_id)}</Field><Field label="Task version">{answer.task_version ?? 'Unverified'}</Field><Field label="Candidate set">{shortId(answer.candidate_id)}</Field></dl><Definition value={answer.definition} />{answer.family_id && <div className="family-note"><strong>{answer.family_name ?? answer.family_id}</strong><p>{answer.mapping_reason ?? 'Adapter-defined family mapping'}</p><span>Adapter {answer.adapter ?? 'Unknown'} · instance {answer.instance_ref ?? 'Unknown'}</span></div>}</LazyDetails>
  </section>;
}
