import { useState } from 'react';
import { protocolLabel } from './connection';
import type { ModelCatalog as Catalog } from './types';

export function ModelCatalog({ catalog, provider, modelProfile, importCaptures }: { catalog: Catalog; provider?: string; modelProfile?: string; importCaptures: () => void }) {
  const [selected, setSelected] = useState('');
  const entry = catalog.models.find(model => model.id === (selected || modelProfile || provider)) ?? catalog.models[0];
  if (!entry) return null;
  const local = entry.upstream?.startsWith('http://127.0.0.1:');
  const command = entry.upstream ? `jev-observer --upstream ${entry.upstream} --provider ${entry.provider ?? entry.id}${local ? ' --upstream-auth none' : ''} --protocol ${entry.protocol ?? 'systemone'} --model-profile ${entry.model_profile ?? entry.id}` : null;
  return <section className="detail-section" aria-label="Decision model catalog">
    <h3>Decision models</h3>
    <p className="caption">{catalog.models.length} model families and serving integrations · catalog updated {catalog.checked_at}. Choose a connection path for your model.</p>
    <label className="field-label">Model integration<select value={entry.id} onChange={event => setSelected(event.target.value)}>
      <optgroup label="Hosted models and gateways">{catalog.models.filter(model => model.upstream?.startsWith('https:')).map(model => <option key={model.id} value={model.id}>{model.name}</option>)}</optgroup>
      <optgroup label="Local model servers">{catalog.models.filter(model => model.upstream?.startsWith('http:')).map(model => <option key={model.id} value={model.id}>{model.name}</option>)}</optgroup>
      <optgroup label="Library and custom API captures">{catalog.models.filter(model => model.integration === 'capture').map(model => <option key={model.id} value={model.id}>{model.name}</option>)}</optgroup>
    </select></label>
    <p><strong>{entry.access === 'closed' ? 'Closed model' : 'Open model or runtime'}</strong> · {entry.integration === 'proxy' ? `${protocolLabel(entry.protocol ?? 'systemone')} proxy` : 'Mapped capture import'}</p>
    <p>{entry.note}</p>{entry.checked_at && <p className="caption">This integration reviewed {entry.checked_at}.</p>}
    {entry.model_names.length > 0 && <p className="caption">Documented model names: {entry.model_names.map((name, index) => <span key={name}>{index > 0 && ', '}<code>{name}</code></span>)}. Use the name accepted by your running server.</p>}
    {command ? <><p className="caption">Start the model server separately, then start or restart Observer with your saved database key set. This selection does not change the running upstream.</p><pre className="json-view sdk-snippet" tabIndex={0} aria-label="Observer startup command">{command}</pre></> : <><p className="caption">Map the original question definitions and reported decisions to a decision request/response capture. Import the JSONL from your application using the capture format; no model call is made by importing.</p><button className="button" onClick={importCaptures}>Import mapped decisions</button></>}
    <p className="caption">Source reviewed; fixtures verify capture and display behavior. Live inference has been checked only for Jev through OpenRouter and Laya. <a href={entry.source_url} target="_blank" rel="noreferrer">Model documentation</a></p>
  </section>;
}
