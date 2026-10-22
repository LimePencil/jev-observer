import { Check, DownloadSimple, FileArrowUp, Fingerprint } from '@phosphor-icons/react';
import type { ChangeEvent } from 'react';
import { useEffect, useRef, useState } from 'react';
import { ErrorNotice } from '../components';
import { number } from '../format';
import type { ImportController } from '../imports';

import { DetailHeading } from './shared';

export function ImportDetails({ controller, initialFormat }: { controller: ImportController; initialFormat?: string }) {
  const { busy, result, error: importError, clear, submit: importRecords } = controller;
  const [format, setFormat] = useState((busy ? controller.format : initialFormat ?? controller.format) ?? 'observer-jsonl'), [text, setText] = useState(''), [filename, setFilename] = useState(''), [error, setError] = useState('');
  const [reading, setReading] = useState(false);
  const inputRevision = useRef(0);
  useEffect(() => () => { inputRevision.current += 1; }, []);
  const maxBytes = 8 * 1024 * 1024;
  async function fileSelected(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file || busy) return;
    event.target.value = '';
    const revision = ++inputRevision.current;
    setText(''); setFilename(file.name); setReading(false); setError(''); clear();
    if (file.size > maxBytes) { setError('Choose a file no larger than 8 MiB. Split larger histories into separate imports.'); return; }
    setReading(true);
    try {
      const buffer = await file.arrayBuffer();
      if (revision !== inputRevision.current) return;
      try { setText(new TextDecoder('utf-8', { fatal: true }).decode(buffer)); }
      catch { setError('This file is not valid UTF-8. Save it as UTF-8 and try again.'); }
    }
    catch { if (revision === inputRevision.current) setError('This file could not be read.'); }
    finally { if (revision === inputRevision.current) setReading(false); }
  }
  async function submit() {
    if (busy || reading || !text.trim()) return;
    setError(''); clear();
    if (new Blob([text]).size > maxBytes) { setError('Imports must not exceed 8 MiB.'); return; }
    await importRecords(text, format);
  }
  return <><DetailHeading eyebrow="Local data" title="Bring your history." description="Import exported records into this workspace. Your file stays on this machine." icon={<FileArrowUp size={15} />} /><div className="drawer-body">
    <label className="field-label">Source format<select value={format} disabled={busy} onChange={event => { if (busy) return; setFormat(event.target.value); setError(''); clear(); }}><option value="observer-jsonl">Observer JSONL export</option><option value="jevrouter-receipt">JevRouter decision receipt</option><option value="systemone-capture">Decision request/response capture</option></select></label>
    {format === 'systemone-capture' && <p className="caption">Keep original definitions and full reported distributions. Missing confidence stays unknown. Include id, timestamp (Unix milliseconds), source, provider, status, capture_complete, request and response in every line. System One is the default shape. For native OpenAI Decisions or wrapped Cloudflare responses, include protocol: openai-decisions or cloudflare-systemone. Include model_profile when a model-specific adapter is needed; keep the original request and response.</p>}<label className="file-input"><FileArrowUp size={28} weight="duotone" /><strong>{filename || 'Choose a local file'}</strong><span>JSONL or JSON · up to 8 MiB</span><input type="file" disabled={busy} accept=".json,.jsonl,application/json,application/x-ndjson" onChange={event => void fileSelected(event)} aria-label="Choose import file" /></label>
    <label className="field-label">Or paste records<textarea rows={9} disabled={busy} value={text} onChange={event => { if (busy) return; inputRevision.current += 1; setReading(false); setText(event.target.value); setFilename(''); setError(''); clear(); }} placeholder={format === 'observer-jsonl' ? 'One exported Observer record per line' : format === 'systemone-capture' ? 'One call per line: id, timestamp, source, provider, status, capture_complete, request, response' : 'Paste a JevRouter decision receipt'} spellCheck={false} /></label>
    <div className="inline-notice"><Fingerprint size={18} /><span>Explicit source event IDs prevent duplicate imports. Identical payloads from separate calls remain separate observations.</span></div>
    {reading && <p className="caption" role="status">Reading {filename}…</p>}{(error || importError) && <ErrorNotice message={error || importError} />}{result && <div className="success-notice" role="status"><Check size={19} weight="bold" /><span><strong>{number(result.imported)} records imported.</strong> {number(result.duplicates)} duplicates skipped.</span></div>}
    <button className="button primary full-width" disabled={!text.trim() || busy || reading} onClick={() => void submit()}><DownloadSimple size={17} />{busy ? 'Importing records…' : 'Import records'}</button>
  </div></>;
}
