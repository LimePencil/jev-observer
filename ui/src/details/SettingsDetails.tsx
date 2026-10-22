import { Database, GearSix, ShieldCheck, Trash } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { useResource } from '../api';
import { ErrorNotice, Loading } from '../components';
import type { HistoryDeletionController } from '../deletion';
import { bytes, number } from '../format';
import type { Settings } from '../types';

import { DetailHeading, Field } from './shared';

export function SettingsDetails({ controller, close }: { controller: HistoryDeletionController; close: () => void }) {
  const { data, error, reload } = useResource<Settings>('/api/settings');
  const [confirm, setConfirm] = useState(false), [typed, setTyped] = useState('');
  const { busy, error: deleteError, remove: deleteHistory } = controller;
  const active = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  async function remove() {
    if (busy || typed !== 'DELETE') return;
    const deleted = await deleteHistory(data?.demo === true);
    // Only close the Settings instance that submitted this operation.
    if (deleted && active.current) close();
  }
  return <><DetailHeading eyebrow="Workspace settings" title="Local by design." description="Capture settings, storage limits and controls for your history." icon={<GearSix size={15} />} />{error ? <ErrorNotice message={error} retry={reload} /> : !data ? <Loading label="Loading settings" /> : <div className="drawer-body">
    <section className="settings-section"><div className="settings-section-icon"><ShieldCheck size={22} weight="duotone" /></div><div><h3>Input capture</h3><p>{data.capture_state ? 'Enabled with local opt-in. Retained input uses configured redaction.' : 'Raw input state is not retained. Question definitions and model answers are stored locally.'}</p><span className={`status-tag ${data.capture_state ? 'warning' : 'success'}`}>{data.capture_state ? 'Opt-in enabled' : 'Off by default'}</span></div></section>
    <section className="settings-section"><div className="settings-section-icon"><Database size={22} weight="duotone" /></div><div><h3>History and retention</h3><dl className="detail-grid"><Field label="Detailed history">{data.retention_days === 0 ? 'No age limit' : `${data.retention_days} days`}</Field><Field label="Record cap">{number(data.max_records)}</Field><Field label="Capture budget">{bytes(data.capture_limit)} per body</Field><Field label="Mode">{data.demo ? 'Isolated sample workspace' : 'Live collection'}</Field></dl><p className="caption">History can end earlier when the record cap is reached. Settings are configured when Observer starts.</p></div></section>
    <section className="detail-section"><h3>Connection</h3><dl className="detail-grid"><Field label="Upstream">{data.upstream}</Field><Field label="Observer version">{data.version}</Field></dl><p className="caption">Registered provider keys are never returned after setup. Live inference goes to the configured provider.</p></section>
    <section className="danger-zone"><h3>Delete local history</h3><p>{data.demo ? 'Remove stored requests, answers and review labels from this sample workspace. Synthetic records will be recreated when you next start demo mode with an empty workspace.' : 'Remove stored requests, answers and review labels from this workspace. Forwarding and future collection will continue.'}</p>{!confirm ? <button className="button danger-button" disabled={busy} onClick={() => setConfirm(true)}>{busy ? 'Deleting…' : <><Trash size={16} />Delete history</>}</button> : <div className="delete-confirm"><label className="field-label">Type DELETE to confirm<input value={typed} onChange={event => setTyped(event.target.value)} placeholder="DELETE" autoComplete="off" /></label><div className="button-row"><button className="button" disabled={busy} onClick={() => { setConfirm(false); setTyped(''); }}>Cancel</button><button className="button danger-button" disabled={typed !== 'DELETE' || busy} onClick={() => void remove()}>{busy ? 'Deleting…' : 'Permanently delete history'}</button></div></div>}{deleteError && <ErrorNotice message={deleteError} />}</section>
  </div>}</>;
}
