import { useEffect, useRef, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { ArrowLeft, ArrowRight, Check, ClipboardText, Database, DownloadSimple, FileArrowUp, Fingerprint, GearSix, Info, ShieldCheck, Trash, WarningCircle, X } from '@phosphor-icons/react';
import type { ChangeEvent, ReactNode } from 'react';
import type { Answer, CredentialStatus, Filters, GroupDetail, Label, RequestRecord, Settings } from './types';
import { api, query, useResource } from './api';
import { answerValue, bytes, dateTime, money, ms, number, percent, shortId } from './format';
import { Definition, Distribution, Empty, ErrorNotice, KindBadge, Loading, RequestTable, StatusBadge, Timeline } from './components';

export type Panel = { type: 'request' | 'group'; id: string } | { type: 'settings' | 'import' | 'connect' } | null;
type Props = { panel: Panel; setPanel: (value: Panel) => void; filters: Filters; changed: () => void; historyChanged: () => void; filterGroup: (id: string) => void; notify: (message: string) => void };

function DetailHeading({ eyebrow, title, description, icon }: { eyebrow: string; title: string; description: string; icon?: ReactNode }) {
  return <div className="drawer-heading"><div className="eyebrow">{icon}{eyebrow}</div><Dialog.Title>{title}</Dialog.Title><Dialog.Description>{description}</Dialog.Description></div>;
}
function Field({ label, children }: { label: string; children: ReactNode }) { return <div className="detail-field"><dt>{label}</dt><dd>{children}</dd></div>; }

export default function Details({ panel, setPanel, filters, changed, historyChanged, filterGroup, notify }: Props) {
  return <Dialog.Root open={panel !== null} onOpenChange={open => { if (!open) setPanel(null); }}><Dialog.Portal><Dialog.Overlay className="dialog-overlay" /><Dialog.Content className={`drawer ${panel?.type === 'import' || panel?.type === 'connect' ? 'drawer-small' : ''}`}>
    <Dialog.Close className="icon-button drawer-close" aria-label="Close detail panel"><X size={20} weight="bold" /></Dialog.Close>
    {panel?.type === 'request' && <RequestDetails id={panel.id} setPanel={setPanel} changed={changed} />}
    {panel?.type === 'group' && <GroupDetails id={panel.id} filters={filters} setPanel={setPanel} filterGroup={filterGroup} />}
    {panel?.type === 'settings' && <SettingsDetails changed={historyChanged} notify={notify} close={() => setPanel(null)} />}
    {panel?.type === 'import' && <ImportDetails changed={historyChanged} />}
    {panel?.type === 'connect' && <ConnectDetails />}
  </Dialog.Content></Dialog.Portal></Dialog.Root>;
}

function RequestDetails({ id, setPanel, changed }: { id: string; setPanel: Props['setPanel']; changed: () => void }) {
  const { data: record, error, reload, update } = useResource<RequestRecord>(`/api/requests/${encodeURIComponent(id)}`);
  function updateLabel(key: string, label: Label) {
    update(previous => previous.id !== id ? previous : { ...previous, labels: [...previous.labels.filter(item => (item.key ?? item.answer_key) !== key), { key, label }] });
    changed();
  }
  return <>
    <DetailHeading eyebrow="Request details" title={record?.source ?? 'Request'} description="Inspect the captured response and its individual decisions." icon={<Fingerprint size={15} />} />
    {error ? <ErrorNotice message={error} retry={reload} /> : !record ? <Loading label="Loading request" /> : <div className="drawer-body">
      <div className="request-detail-id"><code>{record.id}</code><StatusBadge status={record.status} /></div>
      {record.sample && <div className="inline-notice"><Info size={16} />Synthetic sample. No provider was called.</div>}
      {!record.capture_complete && <div className="warning-notice"><WarningCircle size={18} />This capture is incomplete. Missing fields are unknown; the forwarded request was not truncated.</div>}
      {record.transport_error && <ErrorNotice message={record.transport_error} />}
      <dl className="detail-grid"><Field label={record.timestamp == null ? "Imported at" : "Recorded"}>{dateTime(record.timestamp ?? record.imported_at)}{record.timestamp == null && <span className="table-subline">Original event time unknown</span>}</Field><Field label="Latency">{ms(record.duration_ms)}</Field><Field label="Model">{record.model || 'Unknown'}</Field><Field label="Requested model">{record.requested_model ?? 'Unknown'}</Field><Field label="Input tokens">{number(record.input_tokens)}</Field><Field label="Output tokens">{number(record.output_tokens)}</Field><Field label="Request cost">{money(record.cost_usd)}</Field><Field label="Cost basis">{record.cost_basis || 'Unknown'}</Field></dl>
      <div className="detail-section-title"><h3>Decisions</h3><span className="count-badge">{record.answers.length}</span></div>
      <p className="caption">Usage and cost belong to this request, across all its answers.</p>
      {!record.answers.length ? <Empty title="No captured answers">The provider response did not include a supported answer, or the capture was incomplete.</Empty> : record.answers.map(answer => <AnswerDetail key={`${record.id}-${answer.key}`} answer={answer} record={record} onGroup={() => setPanel({ type: 'group', id: answer.group_id })} changed={label => updateLabel(answer.key, label)} />)}
      <section className="detail-section"><h3>Input state</h3><div className="inline-notice"><ShieldCheck size={18} weight="bold" />{record.state_retained ? 'Captured with local opt-in and configured redaction.' : 'Not retained. Raw input capture is off by default.'}</div>{record.state_retained && <details><summary>Inspect retained input</summary><Definition value={record.state} /></details>}</section>
      <section className="detail-section"><h3>Application outcome</h3>{record.actions?.length ? <Definition value={record.actions} /> : <p className="muted">Not collected. An API response does not establish whether an application action ran or succeeded.</p>}</section>
      {(record.source_event_id || record.import_format) && <section className="detail-section"><h3>Import provenance</h3><dl className="detail-grid"><Field label="Source event">{record.source_event_id ?? 'Not supplied'}</Field><Field label="Format">{record.import_format ?? 'Native capture'}</Field></dl></section>}
    </div>}
  </>;
}

function AnswerDetail({ answer, record, onGroup, changed }: { answer: Answer; record: RequestRecord; onGroup: () => void; changed: (label: Label) => void }) {
  const [pending, setPending] = useState<Label | null>(null), [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const currentLabel = [...(record.labels ?? [])].reverse().find(item => (item.key ?? item.answer_key) === answer.key)?.label;
  const probabilities = Object.entries(answer.probabilities ?? {}).sort((a, b) => b[1] - a[1]);
  async function label(value: string) {
    if (!value || pending) return;
    const next = value as Label;
    setPending(next); setError(''); setSaved(false);
    try { await api(`/api/requests/${encodeURIComponent(record.id)}/label`, { method: 'POST', body: JSON.stringify({ key: answer.key, label: next }) }); changed(next); setSaved(true); }
    catch (error) { setError(error instanceof Error ? error.message : 'Could not save label'); }
    finally { setPending(null); }
  }
  return <section className="answer-card"><div className="answer-heading"><h4>{answer.key}</h4><KindBadge kind={answer.kind} /></div>
    <div className="answer-value">{answer.valid ? answerValue(answer.value, answer.kind) : 'Invalid or missing answer'}{answer.kind === 'noul' && answer.valid && <span className="caption">yes-probability</span>}</div>
    {!answer.valid && <p className="error-text">{answer.error ?? 'Excluded from answer distributions.'}</p>}
    {answer.confidence != null && <p className="caption">Reported confidence {percent(answer.confidence)} · not measured accuracy</p>}
    {probabilities.length > 0 && <div className="probability-list" aria-label="Reported probabilities">{probabilities.map(([key, value]) => <div key={key} className="probability-row"><span title={key}>{key}</span><div className="probability-track"><span style={{ width: `${Math.max(0, Math.min(1, value)) * 100}%` }} /></div><span className="mono">{percent(value)}</span></div>)}</div>}
    <div className="answer-actions"><button className="text-button" onClick={onGroup}>View recurring group <ArrowRight size={14} /></button><div className="review-control"><span className="review-status" role="status">{pending ? 'Saving…' : saved ? 'Saved' : ''}</span><label className="label-control"><span>Review</span><select aria-label={`Review ${answer.key}`} value={pending ?? currentLabel ?? ''} aria-disabled={pending !== null} onPointerDown={event => { if (pending) event.preventDefault(); }} onKeyDown={event => { if (pending && ['ArrowUp', 'ArrowDown', 'Home', 'End', ' ', 'Enter'].includes(event.key)) event.preventDefault(); }} onChange={event => void label(event.target.value)}><option value="" disabled>Not labeled</option><option value="correct">Correct</option><option value="incorrect">Incorrect</option><option value="unknown">Unknown</option></select></label></div></div>
    {error && <ErrorNotice message={error} />}
    <details className="definition-details"><summary>Definition and context</summary><dl className="detail-grid compact-fields"><Field label="Definition">{shortId(answer.definition_id)}</Field><Field label="Presentation">{shortId(answer.presentation_id)}</Field><Field label="Task version">{answer.task_version ?? 'Unverified'}</Field><Field label="Candidate set">{shortId(answer.candidate_id)}</Field></dl><Definition value={answer.definition} />{answer.family_id && <div className="family-note"><strong>{answer.family_name ?? answer.family_id}</strong><p>{answer.mapping_reason ?? 'Adapter-defined family mapping'}</p><span>Adapter {answer.adapter ?? 'Unknown'} · instance {answer.instance_ref ?? 'Unknown'}</span></div>}</details>
  </section>;
}

function GroupDetails({ id, filters, setPanel, filterGroup }: { id: string; filters: Filters; setPanel: Props['setPanel']; filterGroup: Props['filterGroup'] }) {
  const { data, error, reload } = useResource<GroupDetail>(`/api/groups/${encodeURIComponent(id)}?${query({ ...filters, group: '' })}`);
  const [compare, setCompare] = useState('');
  const group = data?.group, other = data?.versions.find(version => version.id === compare);
  return <><DetailHeading eyebrow="Recurring question" title={group?.name ?? 'Question group'} description="Definitions remain separate so meaningful changes stay visible." icon={<Fingerprint size={15} />} />
    {error ? <ErrorNotice message={error} retry={reload} /> : !data || !group ? <Loading label="Loading question group" /> : <div className="drawer-body"><div className="group-detail-meta"><KindBadge kind={group.kind} /><span>{group.source}</span><span className="mono">{shortId(group.id)}</span></div>
      <dl className="detail-grid"><Field label="Valid answers">{number(group.valid_count)} / {number(group.answer_count)}</Field><Field label="Distinct requests">{number(group.request_count)}</Field><Field label="Task context">{group.task_version ?? 'Unverified'}</Field><Field label="Presentation">{shortId(group.presentation_id)}</Field></dl>
      <div className="inline-notice"><Info size={17} />{group.task_version ? 'Grouped by source, key, definition, presentation and supplied task version.' : 'This is a definition-based group. Rules carried in input state may change without a supplied task version.'}</div>
      <section className="detail-section"><div className="detail-section-title"><h3>Answer distribution</h3><span className="caption">{number(group.valid_count)} valid answers</span></div><Distribution group={group} />{group.kind !== 'choice' && <p className="caption">Mean {group.kind === 'noul' ? 'yes-probability' : 'score'}: {answerValue(group.mean_value, group.kind)}{group.kind === 'score' ? ' · within this rubric only' : ''}</p>}</section>
      <section className="detail-section"><h3>Associated request activity</h3><Timeline data={data.timeline} compact /><p className="caption">Request-level usage and cost can overlap with other groups. They must not be added into a global total.</p></section>
      <section className="detail-section"><h3>{group.is_family ? 'Original definitions' : 'Definition versions'} <span className="count-badge">{data.versions.length}</span></h3><p className="caption">{group.is_family ? 'This adapter links original instance definitions. Instance differences do not imply a changed rule.' : 'Different definitions and presentations keep separate statistics.'} Showing {data.versions.length} of {number(group.version_count)} {group.is_family ? 'original definitions' : 'versions'}.</p><div className="version-list">{data.versions.map((version, index) => <button key={version.id} className={`version-item ${version.id === group.id ? 'selected' : ''}`} onClick={() => { setCompare(''); setPanel({ type: 'group', id: version.id }); }}><span><strong>Definition {shortId(version.definition_id)}</strong><small>Presentation {shortId(version.presentation_id)} · {version.task_version ?? 'Unverified task context'}</small></span><span>{number(version.answer_count)} answers {version.id === group.id ? <Check size={15} /> : <ArrowRight size={14} />}</span><span className="sr-only">Version {index + 1}</span></button>)}</div>
        {data.versions.length > 1 && <label className="field-label comparison-select">Compare with<select value={compare} onChange={event => setCompare(event.target.value)}><option value="">Choose a separate version</option>{data.versions.filter(version => version.id !== group.id).map(version => <option value={version.id} key={version.id}>{shortId(version.definition_id)} / {shortId(version.presentation_id)} · {version.task_version ?? 'unverified'}</option>)}</select></label>}
        {other && <div className="comparison"><div><h4>Current definition</h4><Definition value={group.definition} /></div><div><h4>Selected version</h4><Definition value={other.definition} /><p className="caption">Task: {other.task_version ?? 'Unverified'} · Presentation: {shortId(other.presentation_id)}</p></div></div>}
        {!other && <details><summary>Inspect definition</summary><Definition value={group.definition} /></details>}
      </section>
      {group.family_id && <section className="detail-section"><h3>Indexed family</h3><div className="family-note"><strong>{group.family_name ?? group.family_id}</strong><p>Mapped by {group.adapter ?? 'a local adapter'}. Original definitions and individual instances remain distinct.</p></div></section>}
      <section className="detail-section"><div className="detail-section-title"><h3>Contributing requests</h3><button className="text-button" onClick={() => filterGroup(group.id)}>Filter overview <ArrowRight size={14} /></button></div><p className="caption">Latest {data.requests.length} requests in the selected window.</p><RequestTable requests={data.requests} compact open={requestId => setPanel({ type: 'request', id: requestId })} /></section>
      <section className="detail-section"><h3>Answer observations</h3><p className="caption">Showing {data.answers.length} of {number(data.total_answers ?? group.answer_count)} answers; latest {data.detail_limit ?? 100} maximum.</p><div className="table-scroll"><table><thead><tr><th>Answer</th><th>Value</th><th>Confidence</th><th>Review</th></tr></thead><tbody>{data.answers.map((answer, index) => <tr key={`${answer.request_id}-${answer.key}-${index}`}><td><button className="text-button" onClick={() => setPanel({ type: 'request', id: answer.request_id })}>{answer.key}</button></td><td>{answer.valid ? answerValue(answer.value, group.kind) : 'Invalid'}</td><td>{percent(answer.confidence)}</td><td>{answer.label ?? 'Not labeled'}</td></tr>)}</tbody></table></div></section>
    </div>}
  </>;
}

function SettingsDetails({ changed, notify, close }: { changed: () => void; notify: Props['notify']; close: () => void }) {
  const { data, error, reload } = useResource<Settings>('/api/settings');
  const [confirm, setConfirm] = useState(false), [typed, setTyped] = useState(''), [busy, setBusy] = useState(false), [deleteError, setDeleteError] = useState('');
  const active = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  async function remove() {
    if (busy || typed !== 'DELETE') return;
    setBusy(true); setDeleteError('');
    try {
      await api('/api/data', { method: 'DELETE' });
      changed(); notify(data?.demo ? 'Sample history deleted. It will be recreated on the next demo startup.' : 'Local history deleted. New incoming requests will still be recorded.');
      // The user may have closed Settings and moved to another panel while
      // storage was busy. Keep global history current without closing it.
      if (active.current) close();
    }
    catch (error) { if (active.current) setDeleteError(error instanceof Error ? error.message : 'Could not delete history'); }
    finally { if (active.current) setBusy(false); }
  }
  return <><DetailHeading eyebrow="Workspace settings" title="Local by design." description="Capture settings, storage limits and controls for your history." icon={<GearSix size={15} />} />{error ? <ErrorNotice message={error} retry={reload} /> : !data ? <Loading label="Loading settings" /> : <div className="drawer-body">
    <section className="settings-section"><div className="settings-section-icon"><ShieldCheck size={22} weight="duotone" /></div><div><h3>Input capture</h3><p>{data.capture_state ? 'Enabled with local opt-in. Retained input uses configured redaction.' : 'Raw input state is not retained. Question definitions and model answers are stored locally.'}</p><span className={`status-tag ${data.capture_state ? 'warning' : 'success'}`}>{data.capture_state ? 'Opt-in enabled' : 'Off by default'}</span></div></section>
    <section className="settings-section"><div className="settings-section-icon"><Database size={22} weight="duotone" /></div><div><h3>History and retention</h3><dl className="detail-grid"><Field label="Detailed history">{data.retention_days === 0 ? 'No age limit' : `${data.retention_days} days`}</Field><Field label="Record cap">{number(data.max_records)}</Field><Field label="Capture budget">{bytes(data.capture_limit)} per body</Field><Field label="Mode">{data.demo ? 'Isolated sample workspace' : 'Live collection'}</Field></dl><p className="caption">History can end earlier when the record cap is reached. Settings are configured when Observer starts.</p></div></section>
    <section className="detail-section"><h3>Connection</h3><dl className="detail-grid"><Field label="Upstream">{data.upstream}</Field><Field label="Observer version">{data.version}</Field></dl><p className="caption">Registered provider keys are never returned after setup. Live inference goes to the configured provider.</p></section>
    <section className="danger-zone"><h3>Delete local history</h3><p>{data.demo ? 'Remove stored requests, answers and review labels from this sample workspace. Synthetic records will be recreated when you next start demo mode with an empty workspace.' : 'Remove stored requests, answers and review labels from this workspace. Forwarding and future collection will continue.'}</p>{!confirm ? <button className="button danger-button" onClick={() => setConfirm(true)}><Trash size={16} />Delete history</button> : <div className="delete-confirm"><label className="field-label">Type DELETE to confirm<input value={typed} onChange={event => setTyped(event.target.value)} placeholder="DELETE" autoComplete="off" /></label><div className="button-row"><button className="button" disabled={busy} onClick={() => { setConfirm(false); setTyped(''); }}>Cancel</button><button className="button danger-button" disabled={typed !== 'DELETE' || busy} onClick={() => void remove()}>{busy ? 'Deleting…' : 'Permanently delete history'}</button></div></div>}{deleteError && <ErrorNotice message={deleteError} />}</section>
  </div>}</>;
}

function ImportDetails({ changed }: { changed: () => void }) {
  const [format, setFormat] = useState('observer-jsonl'), [text, setText] = useState(''), [filename, setFilename] = useState(''), [error, setError] = useState(''), [busy, setBusy] = useState(false), [result, setResult] = useState<{ imported: number; duplicates: number } | null>(null);
  const [reading, setReading] = useState(false);
  const inputRevision = useRef(0);
  useEffect(() => () => { inputRevision.current += 1; }, []);
  const maxBytes = 8 * 1024 * 1024;
  async function fileSelected(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file || busy) return;
    event.target.value = '';
    const revision = ++inputRevision.current;
    setText(''); setFilename(file.name); setReading(false); setError(''); setResult(null);
    if (file.size > maxBytes) { setError('Choose a file no larger than 8 MiB. Split larger histories into separate imports.'); return; }
    setReading(true);
    try { const value = await file.text(); if (revision === inputRevision.current) setText(value); }
    catch { if (revision === inputRevision.current) setError('This file could not be read.'); }
    finally { if (revision === inputRevision.current) setReading(false); }
  }
  async function submit() {
    if (busy || reading || !text.trim()) return;
    setError(''); setBusy(true); setResult(null);
    try {
      if (new Blob([text]).size > maxBytes) throw new Error('Imports must not exceed 8 MiB.');
      const result = await api<{ imported: number; duplicates: number }>('/api/import', { method: 'POST', body: JSON.stringify({ text, format }) });
      setResult(result); changed();
    } catch (error) { setError(error instanceof Error ? error.message : 'Import failed'); }
    finally { setBusy(false); }
  }
  return <><DetailHeading eyebrow="Local data" title="Bring your history." description="Import exported records into this workspace. Your file stays on this machine." icon={<FileArrowUp size={15} />} /><div className="drawer-body">
    <label className="field-label">Source format<select value={format} disabled={busy} onChange={event => { setFormat(event.target.value); setError(''); setResult(null); }}><option value="observer-jsonl">Observer JSONL export</option><option value="jevrouter-receipt">JevRouter decision receipt</option></select></label>
    <label className="file-input"><FileArrowUp size={28} weight="duotone" /><strong>{filename || 'Choose a local file'}</strong><span>JSONL or JSON · up to 8 MiB</span><input type="file" disabled={busy} accept=".json,.jsonl,application/json,application/x-ndjson" onChange={event => void fileSelected(event)} aria-label="Choose import file" /></label>
    <label className="field-label">Or paste records<textarea rows={9} disabled={busy} value={text} onChange={event => { inputRevision.current += 1; setReading(false); setText(event.target.value); setFilename(''); setError(''); setResult(null); }} placeholder={format === 'observer-jsonl' ? 'One exported Observer record per line' : 'Paste a JevRouter decision receipt'} spellCheck={false} /></label>
    <div className="inline-notice"><Fingerprint size={18} /><span>Explicit source event IDs prevent duplicate imports. Identical payloads from separate calls remain separate observations.</span></div>
    {reading && <p className="caption" role="status">Reading {filename}…</p>}{error && <ErrorNotice message={error} />}{result && <div className="success-notice" role="status"><Check size={19} weight="bold" /><span><strong>{number(result.imported)} records imported.</strong> {number(result.duplicates)} duplicates skipped.</span></div>}
    <button className="button primary full-width" disabled={!text.trim() || busy || reading} onClick={() => void submit()}><DownloadSimple size={17} />{busy ? 'Importing records…' : 'Import records'}</button>
  </div></>;
}

function ConnectDetails() {
  const baseUrl = import.meta.env.DEV ? 'http://127.0.0.1:8765' : window.location.origin;
  const [copyStatus, setCopyStatus] = useState('');
  const { data: credential, error: credentialError, reload } = useResource<CredentialStatus>('/api/credentials');
  const { data: settings, error: settingsError, reload: reloadSettings } = useResource<Settings>('/api/settings');
  const [providerKey, setProviderKey] = useState('');
  const [persist, setPersist] = useState(false);
  const [clientToken, setClientToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [credentialMessage, setCredentialMessage] = useState('');
  const keyValid = providerKey.length > 0 && providerKey.length <= 4096 && /^[\x21-\x7e]+$/.test(providerKey);
  useEffect(() => { if (credential) setPersist(credential.storage === 'system'); }, [credential?.storage]);
  async function copyUrl() {
    setCopyStatus('');
    try { await navigator.clipboard.writeText(baseUrl); setCopyStatus('Local base URL copied.'); }
    catch { setCopyStatus('Could not copy. Select the URL to copy it manually.'); }
  }
  async function copyToken() {
    try { await navigator.clipboard.writeText(clientToken); setCredentialMessage('Local client token copied.'); }
    catch { setCredentialMessage('Could not copy. Select the token to copy it manually.'); }
  }
  async function registerKey() {
    if (!keyValid || busy) return;
    setBusy(true); setCredentialMessage(''); setClientToken('');
    try {
      const result = await api<{ client_token: string; storage: 'session' | 'system' }>('/api/credentials', { method: 'PUT', body: JSON.stringify({ api_key: providerKey, persist }) });
      setProviderKey(''); setClientToken(result.client_token);
      setCredentialMessage(result.storage === 'system' ? 'Key saved in your system credential store.' : 'Key available until Observer stops.');
      reload();
    } catch (error) { setCredentialMessage(error instanceof Error ? error.message : 'Could not register provider key.'); }
    finally { setBusy(false); }
  }
  async function removeKey() {
    if (busy) return;
    setBusy(true); setCredentialMessage('');
    try {
      await api('/api/credentials', { method: 'DELETE' });
      setClientToken(''); setProviderKey(''); setConfirmRemove(false); setCredentialMessage('Registered provider key removed.'); reload();
    } catch (error) { setCredentialMessage(error instanceof Error ? error.message : 'Could not remove provider key.'); }
    finally { setBusy(false); }
  }
  return <><DetailHeading eyebrow="Connect an application" title={settings?.demo ? 'Start live collection.' : 'Your next request, visible.'} description={settings?.demo ? 'This sample workspace cannot forward requests. Restart Observer in live mode to connect an application.' : 'Point a compatible TypeSafe client at this local Observer instance.'} icon={<ArrowRight size={15} />} /><div className="drawer-body">
    {settingsError && <ErrorNotice message={settingsError} retry={reloadSettings} />}
    {!settings && !settingsError && <Loading label="Loading connection settings" />}
    {settings?.demo && <div className="inline-notice"><Info size={19} /><span>Stop this sample instance. Generate and save a 64-character database key with <code>openssl rand -hex 32</code>, then set <code>JEV_OBSERVER_DB_KEY</code> to that key. Run <code>jev-observer</code> without <code>--demo</code> and connect your app from the live dashboard.</span></div>}
    {settings && !settings.demo && <><label className="field-label">Local base URL<div className="copy-field"><code>{baseUrl}</code><button className="icon-button" aria-label="Copy local base URL" onClick={() => void copyUrl()}><ClipboardText size={18} /></button></div></label>
    <p className="caption">Use this origin for your SDK’s <code>base_url</code> or <code>baseURL</code>. The SDK appends <code>/v1/systemone</code>.</p>
    <p className="caption" role="status">{copyStatus}</p>
    <section className="detail-section"><h3>Provider key</h3><p>Register a key here to use a separate local client token in your SDK. The provider key is never shown again or saved in history.</p>
      {credentialError && <ErrorNotice message={credentialError} retry={reload} />}
      {credential && <p className="caption">{credential.configured ? `Registered key: ${credential.storage === 'system' ? 'saved in system credential store' : 'this session only'}` : 'No key registered in Observer.'}</p>}
        <label className="field-label">TypeSafe API key<input type="password" value={providerKey} onChange={event => setProviderKey(event.target.value)} autoComplete="off" spellCheck={false} disabled={busy} maxLength={4096} /></label>
        {providerKey && !keyValid && <p className="error-text" role="status">Use a key with printable ASCII characters and no spaces.</p>}
        <label className="credential-choice"><input type="checkbox" checked={persist} onChange={event => setPersist(event.target.checked)} disabled={busy} /> Save in this computer’s credential store</label>
        <p className="caption">Leave unchecked to keep the key only until Observer stops. System saving needs an unlocked desktop credential store. Replacing a key rotates its local token, so connected applications need the new token.</p>
        <div className="button-row"><button className="button primary" disabled={!keyValid || busy} onClick={() => void registerKey()}>{busy ? 'Working…' : credential?.configured ? 'Replace registered key' : 'Register key'}</button>{credential?.configured && (!confirmRemove ? <button className="button" disabled={busy} onClick={() => setConfirmRemove(true)}>Remove key</button> : <><button className="button" disabled={busy} onClick={() => setConfirmRemove(false)}>Keep key</button><button className="button danger-button" disabled={busy} onClick={() => void removeKey()}>Confirm removal</button></>)}</div>
      {credentialMessage && <p className="caption" role="status">{credentialMessage}</p>}
      {clientToken && <div className="credential-token" role="status"><strong>Copy this local client token now.</strong><p>Set your SDK’s <code>api_key</code> or <code>apiKey</code> to this token. It is shown once and is required to use the registered provider key.</p><div className="copy-field"><code>{clientToken}</code><button className="icon-button" aria-label="Copy local client token" onClick={() => void copyToken()}><ClipboardText size={18} /></button></div></div>}
    </section>
    <section className="detail-section"><h3>Native endpoint</h3><code className="endpoint">POST /v1/systemone</code><p>Your SDK can also send its provider key directly when it includes <code>x-observer-access</code> with the workspace dashboard token. Registered keys are used only when the SDK sends the local client token.</p></section>
    <section className="detail-section"><h3>Keep sources recognizable</h3><p>Optionally add <code>x-observer-source</code> to name your application and <code>x-observer-task-version</code> when rules carried in state change.</p></section>
    <div className="inline-notice"><ShieldCheck size={19} /><span>The proxy is local. A hosted application cannot reach this machine’s loopback address. Import its exported records instead.</span></div>
    <p className="caption">Collection pressure does not intentionally hold up forwarding. Any missing captures are shown in collection health.</p>
    </>}
    <a className="text-button" href="#requests" onClick={() => document.querySelector<HTMLButtonElement>('.drawer-close')?.click()}><ArrowLeft size={14} />Back to your workspace</a>
  </div></>;
}
