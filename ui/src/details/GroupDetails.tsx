import { ArrowRight, Check, Fingerprint, Info } from '@phosphor-icons/react';
import { useState } from 'react';
import { query, useResource } from '../api';
import { Definition, Distribution, ErrorNotice, KindBadge, LazyDetails, Loading, RequestTable, Timeline } from '../components';
import { answerValue, money, ms, number, percent, shortId, tokens } from '../format';
import type { Filters, Group, GroupDetail } from '../types';

import { DetailHeading, Field } from './shared';
import type { DetailProps } from './types';

export function GroupDetails({ id, filters, setPanel, filterGroup, historyRevision }: { id: string; filters: Filters; setPanel: DetailProps['setPanel']; filterGroup: DetailProps['filterGroup']; historyRevision: number }) {
  const { data, error, reload } = useResource<GroupDetail>(`/api/groups/${encodeURIComponent(id)}?${query(filters)}`, historyRevision);
  const [compare, setCompare] = useState('');
  const group = data?.group, other = data?.versions.find(version => version.id === compare);
  return <><DetailHeading eyebrow="Recurring question" title={group?.name ?? 'Question group'} description="Definitions remain separate so meaningful changes stay visible." icon={<Fingerprint size={15} />} />
    {error ? <ErrorNotice message={error} retry={reload} /> : !data || !group ? <Loading label="Loading question group" /> : <div className="drawer-body"><div className="group-detail-meta"><KindBadge kind={group.kind} /><span>{group.source}</span><span className="mono">{shortId(group.id)}</span></div>
      <dl className="detail-grid"><Field label="Valid answers">{number(group.valid_count)} / {number(group.answer_count)}</Field><Field label="Distinct requests">{number(group.request_count)}</Field><Field label="Task context">{group.task_version ?? 'Unverified'}</Field><Field label="Presentation">{shortId(group.presentation_id)}</Field></dl>
      <div className="inline-notice"><Info size={17} />{group.task_version ? 'Grouped by source, key, definition, presentation and supplied task version.' : 'This is a definition-based group. Rules carried in input state may change without a supplied task version.'}</div>
      <section className="detail-section"><div className="detail-section-title"><h3>Answer distribution</h3><span className="caption">{number(group.valid_count)} valid answers</span></div><Distribution group={group} />{group.kind !== 'choice' && <p className="caption">Mean {group.kind === 'noul' ? 'yes-probability' : 'score'}: {answerValue(group.mean_value, group.kind)}{group.kind === 'score' ? ' · within this rubric only' : ''}</p>}</section>
      <section className="detail-section"><h3>Associated request activity</h3><Timeline data={data.timeline} meta={data.timeline_meta} compact /><p className="caption">Request-level usage and cost can overlap with other groups. They must not be added into a global total.</p></section>
      <section className="detail-section"><h3>{group.is_family ? 'Original definitions' : 'Definition versions'} <span className="count-badge">{data.versions.length}</span></h3><p className="caption">{group.is_family ? 'This adapter links original instance definitions. Instance differences do not imply a changed rule.' : 'Different definitions and presentations keep separate statistics.'} Showing {data.versions.length} of {number(group.version_count)} {group.is_family ? 'original definitions' : 'versions'}.</p><div className="version-list">{data.versions.map((version, index) => <button key={version.id} className={`version-item ${version.id === group.id ? 'selected' : ''}`} onClick={() => { setCompare(''); setPanel({ type: 'group', id: version.id }); }}><span><strong>Definition {shortId(version.definition_id)}</strong><small>Presentation {shortId(version.presentation_id)} · {version.task_version ?? 'Unverified task context'}</small></span><span>{number(version.answer_count)} answers {version.id === group.id ? <Check size={15} /> : <ArrowRight size={14} />}</span><span className="sr-only">Version {index + 1}</span></button>)}</div>
        {data.versions.length > 1 && <label className="field-label comparison-select">Compare with<select value={compare} onChange={event => setCompare(event.target.value)}><option value="">Choose a separate version</option>{data.versions.filter(version => version.id !== group.id).map(version => <option value={version.id} key={version.id}>{shortId(version.definition_id)} / {shortId(version.presentation_id)} · {version.task_version ?? 'unverified'}</option>)}</select></label>}
        {other && <VersionComparison current={group} other={other} />}
        {!other && <LazyDetails summary="Inspect definition"><Definition value={group.definition} /></LazyDetails>}
      </section>
      {group.family_id && <section className="detail-section"><h3>Indexed family</h3><div className="family-note"><strong>{group.family_name ?? group.family_id}</strong><p>Mapped by {group.adapter ?? 'a local adapter'}. Original definitions and individual instances remain distinct.</p></div></section>}
      <section className="detail-section"><div className="detail-section-title"><h3>Contributing requests</h3><button className="text-button" onClick={() => filterGroup(group.id)}>Filter overview <ArrowRight size={14} /></button></div><p className="caption">Latest {data.requests.length} requests in the selected window.</p><RequestTable requests={data.requests} compact open={requestId => setPanel({ type: 'request', id: requestId })} /></section>
      <section className="detail-section"><h3>Answer observations</h3><p className="caption">Showing {data.answers.length} of {number(data.total_answers ?? group.answer_count)} answers; latest {data.detail_limit ?? 100} maximum.</p><div className="table-scroll"><table><thead><tr><th>Answer</th><th>Value</th><th>Confidence</th><th>Review</th></tr></thead><tbody>{data.answers.map((answer, index) => <tr key={`${answer.request_id}-${answer.key}-${index}`}><td><button className="text-button" onClick={() => setPanel({ type: 'request', id: answer.request_id })}>{answer.key}</button></td><td>{answer.valid ? answerValue(answer.value, group.kind) : 'Invalid'}</td><td>{percent(answer.confidence)}</td><td>{answer.label ?? 'Not labeled'}</td></tr>)}</tbody></table></div></section>
    </div>}
  </>;
}

function VersionComparison({ current, other }: { current: Group; other: Group }) {
  const reviewed = (group: Group) => group.review_counts ? group.review_counts.correct + group.review_counts.incorrect : null;
  const reviewedAccuracy = (group: Group) => {
    const count = reviewed(group);
    return count ? `${percent(group.review_counts!.correct / count)} (${number(group.review_counts!.correct)} / ${number(count)})` : 'No correct/incorrect reviews';
  };
  return <section className="version-comparison" aria-label="Version outcome comparison">
    <h4>Outcomes in the selected scope</h4>
    <p className="caption">These are separate observed samples, not a matched replay. Different traffic and model choices can explain differences. Human reviews cover only the labeled sample.</p>
    <div className="table-scroll"><table><thead><tr><th>Measure</th><th>Current version</th><th>Selected version</th></tr></thead><tbody>
      <tr><th>Distinct requests</th><td>{number(current.request_count)}</td><td>{number(other.request_count)}</td></tr>
      <tr><th>Valid answers / all answers</th><td>{number(current.valid_count)} / {number(current.answer_count)}</td><td>{number(other.valid_count)} / {number(other.answer_count)}</td></tr>
      <tr><th>Invalid answers</th><td>{number(current.answer_count - current.valid_count)}</td><td>{number(other.answer_count - other.valid_count)}</td></tr>
      <tr><th>Answers with warnings</th><td>{number(current.warning_count)}</td><td>{number(other.warning_count)}</td></tr>
      <tr><th>Failed requests</th><td>{number(current.error_count)} / {number(current.request_count)}</td><td>{number(other.error_count)} / {number(other.request_count)}</td></tr>
      <tr><th>Mean request latency</th><td>{ms(current.mean_latency_ms)}</td><td>{ms(other.mean_latency_ms)}</td></tr>
      <tr><th>Known request cost</th><td>{money(current.cost_usd)}<span className="table-subline">{number(current.cost_known_requests)} / {number(current.request_count)} requests covered</span></td><td>{money(other.cost_usd)}<span className="table-subline">{number(other.cost_known_requests)} / {number(other.request_count)} requests covered</span></td></tr>
      <tr><th>Reported input tokens</th><td>{current.input_tokens_overflow ? 'Total exceeds range' : tokens(current.input_tokens)}<span className="table-subline">{number(current.input_tokens_known_requests)} / {number(current.request_count)} requests covered</span></td><td>{other.input_tokens_overflow ? 'Total exceeds range' : tokens(other.input_tokens)}<span className="table-subline">{number(other.input_tokens_known_requests)} / {number(other.request_count)} requests covered</span></td></tr>
      <tr><th>Reported output tokens</th><td>{current.output_tokens_overflow ? 'Total exceeds range' : tokens(current.output_tokens)}<span className="table-subline">{number(current.output_tokens_known_requests)} / {number(current.request_count)} requests covered</span></td><td>{other.output_tokens_overflow ? 'Total exceeds range' : tokens(other.output_tokens)}<span className="table-subline">{number(other.output_tokens_known_requests)} / {number(other.request_count)} requests covered</span></td></tr>
      {current.kind === other.kind && current.kind !== 'choice' && <tr><th>Mean {current.kind === 'noul' ? 'yes-probability' : 'score'}</th><td>{answerValue(current.mean_value, current.kind)}</td><td>{answerValue(other.mean_value, other.kind)}</td></tr>}
      <tr><th>Correct among judged answers</th><td>{reviewedAccuracy(current)}</td><td>{reviewedAccuracy(other)}</td></tr>
      <tr><th>Unknown reviews</th><td>{number(current.review_counts?.unknown)}</td><td>{number(other.review_counts?.unknown)}</td></tr>
      <tr><th>Not labeled</th><td>{number(current.review_counts?.unlabeled)}</td><td>{number(other.review_counts?.unlabeled)}</td></tr>
    </tbody></table></div>
    <p className="caption">Requests may contribute to both versions. Usage and cost must not be added across columns. Known costs can include provider-reported charges and configured estimates; inspect requests for their basis.</p>
    {current.kind === 'score' && <p className="caption">Score means use each definition’s own rubric; changed rubrics are not directly comparable.</p>}
    <div className="comparison"><div><h4>Current outcomes</h4><Distribution group={current} /></div><div><h4>Selected outcomes</h4><Distribution group={other} /></div></div>
    <LazyDetails summary="Compare definitions and context"><div className="comparison"><div><h4>Current definition</h4><Definition value={current.definition} /><p className="caption">Task: {current.task_version ?? 'Unverified'} · Presentation: {shortId(current.presentation_id)}</p></div><div><h4>Selected version</h4><Definition value={other.definition} /><p className="caption">Task: {other.task_version ?? 'Unverified'} · Presentation: {shortId(other.presentation_id)}</p></div></div></LazyDetails>
  </section>;
}
