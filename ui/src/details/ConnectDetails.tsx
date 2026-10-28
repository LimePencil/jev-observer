import { ArrowLeft, ArrowRight, ClipboardText, Info, ShieldCheck } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { useResource } from '../api';
import { ErrorNotice, Loading } from '../components';
import { connectionSnippet, isLoopbackUpstream, protocolEndpoint, protocolLabel } from '../connection';
import type { CredentialController } from '../credentials';
import { number } from '../format';
import { ModelCatalog } from '../ModelCatalog';
import type { Health, Settings } from '../types';

import { DetailHeading, Field } from './shared';

export function ConnectDetails({ health, controller, importCaptures }: { health?: Health; controller: CredentialController; importCaptures: () => void }) {
  const baseUrl = import.meta.env.DEV ? 'http://127.0.0.1:8765' : window.location.origin;
  const [copyStatus, setCopyStatus] = useState('');
  const [language, setLanguage] = useState<'python' | 'javascript'>('python');
  const [model, setModel] = useState('');
  const { credential, error: credentialError, reload, busy, message: credentialMessage, clientToken, mutate } = controller;
  const { data: settings, error: settingsError, reload: reloadSettings } = useResource<Settings>('/api/settings');
  const [providerKey, setProviderKey] = useState('');
  const [persist, setPersist] = useState(false);
  const persistEdited = useRef(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const local = settings?.upstream_auth === 'none';
  const configuredEntry = settings?.model_catalog?.models.find(entry => entry.id === (settings.model_profile || settings.provider));
  const localModel = local || isLoopbackUpstream(settings?.upstream) || settings?.provider?.toLowerCase() === 'laya';
  const protocol = settings?.protocol ?? 'systemone';
  const endpoint = protocolEndpoint(protocol);
  const snippet = connectionSnippet(language, baseUrl, local, model, localModel, protocol);
  const keyValid = providerKey.length > 0 && providerKey.length <= 4096 && /^[\x21-\x7e]+$/.test(providerKey);
  useEffect(() => { if (credential && !persistEdited.current) setPersist(credential.storage === 'system'); }, [credential?.storage]);
  async function copyUrl() {
    setCopyStatus('');
    try { await navigator.clipboard.writeText(baseUrl); setCopyStatus('Local base URL copied.'); }
    catch { setCopyStatus('Could not copy. Select the URL to copy it manually.'); }
  }
  async function copyToken() {
    try { await navigator.clipboard.writeText(clientToken); setCopyStatus('Local client token copied.'); }
    catch { setCopyStatus('Could not copy. Select the token to copy it manually.'); }
  }
  async function registerKey() {
    if (!keyValid || busy) return;
    if (await mutate(providerKey, persist)) setProviderKey('');
  }
  async function removeKey() {
    if (busy) return;
    if (await mutate(null)) { setProviderKey(''); setConfirmRemove(false); }
  }
  return <><DetailHeading eyebrow="Connect an application" title={settings?.demo ? 'Start live collection.' : 'Your next request, visible.'} description={settings?.demo ? 'This sample workspace cannot forward requests. Restart Observer in live mode to connect an application.' : 'Point your application at this local Observer instance using its configured protocol.'} icon={<ArrowRight size={15} />} /><div className="drawer-body">
    {settingsError && <ErrorNotice message={settingsError} retry={reloadSettings} />}
    {!settings && !settingsError && <Loading label="Loading connection settings" />}
    {settings?.demo && <div className="inline-notice"><Info size={19} /><span>Stop this sample instance. Generate and save a 64-character database key with <code>openssl rand -hex 32</code>, then set <code>JEV_OBSERVER_DB_KEY</code> to that key. Run <code>jev-observer</code> without <code>--demo</code> and connect your app from the live dashboard.</span></div>}
    {settings?.model_catalog && <ModelCatalog catalog={settings.model_catalog} provider={settings.provider} modelProfile={settings.model_profile} importCaptures={importCaptures} />}
    {settings && !settings.demo && <><label className="field-label">Local base URL<div className="copy-field"><code>{baseUrl}</code><button className="icon-button" aria-label="Copy local base URL" onClick={() => void copyUrl()}><ClipboardText size={18} /></button></div></label>
    <p className="caption">{protocol === 'cloudflare-systemone' ? <>Send native System One JSON to <code>{endpoint}</code>. The response keeps Cloudflare’s <code>result</code> envelope.</> : <>Use {protocol === 'openai-decisions' ? <>this origin plus <code>/v1</code></> : 'this origin'} for your SDK’s <code>base_url</code> or <code>baseURL</code>. The example calls <code>{endpoint}</code>.</>}</p>
    <p className="caption" role="status" aria-label="Copy status">{copyStatus}</p>
    <dl className="detail-grid"><Field label="Protocol">{protocolLabel(protocol)}</Field><Field label="Model profile">{settings.model_profile || 'Not configured'}</Field><Field label="Configured provider">{settings.provider ?? 'typesafe'}</Field><Field label="Upstream endpoint"><code className="wrap-anywhere">{settings.upstream}</code></Field><Field label="Provider authentication">{local ? 'None · local server' : 'Bearer key'}</Field></dl>
    {local && <div className="inline-notice"><ShieldCheck size={18} /><span>Your local model server receives no provider credentials. Set <code>JEV_OBSERVER_ACCESS_TOKEN</code> to the workspace token from the file printed at startup. This token authenticates your application to Observer.</span></div>}
    {!local && <section className="detail-section"><h3>Provider key</h3><p>Register a key here to use a separate local client token in your SDK. The provider key is never shown again or saved in history.</p>
      {credentialError && <ErrorNotice message={credentialError} retry={reload} />}
      {credential && <p className="caption">{credential.configured ? `Registered key: ${credential.storage === 'system' ? 'saved in system credential store' : 'this session only'}` : 'No key registered in Observer.'}</p>}
        <label className="field-label">{settings.provider && settings.provider !== 'typesafe' ? `${settings.provider} API key` : 'TypeSafe API key'}<input type="password" value={providerKey} onChange={event => setProviderKey(event.target.value)} autoComplete="off" spellCheck={false} disabled={busy} maxLength={4096} /></label>
        {providerKey && !keyValid && <p className="error-text" role="status">Use a key with printable ASCII characters and no spaces.</p>}
        <label className="credential-choice"><input type="checkbox" checked={persist} onChange={event => { persistEdited.current = true; setPersist(event.target.checked); }} disabled={busy} /> Save in this computer’s credential store</label>
        <p className="caption">Leave unchecked to keep the key only until Observer stops. System saving needs an unlocked desktop credential store. Replacing a key rotates its local token, so connected applications need the new token.</p>
        <div className="button-row"><button className="button primary" disabled={!keyValid || busy} onClick={() => void registerKey()}>{busy ? 'Working…' : credential?.configured ? 'Replace registered key' : 'Register key'}</button>{credential?.configured && (!confirmRemove ? <button className="button" disabled={busy} onClick={() => setConfirmRemove(true)}>Remove key</button> : <><button className="button" disabled={busy} onClick={() => setConfirmRemove(false)}>Keep key</button><button className="button danger-button" disabled={busy} onClick={() => void removeKey()}>Confirm removal</button></>)}</div>
      {credentialMessage && <p className="caption" role="status">{credentialMessage}</p>}
      {clientToken && <div className="credential-token" role="status"><strong>Copy this local client token now.</strong><p>Set your SDK’s <code>api_key</code> or <code>apiKey</code> to this token. It stays available when you reopen this panel until the page reloads or closes, or the key is replaced or removed.</p><div className="copy-field"><code>{clientToken}</code><button className="icon-button" aria-label="Copy local client token" onClick={() => void copyToken()}><ClipboardText size={18} /></button></div></div>}
    </section>}
    <section className="detail-section"><h3>{protocol === 'cloudflare-systemone' ? 'Connect with native HTTP' : 'Connect your SDK'}</h3><p className="caption">Use the model name served by your configured upstream. This example sends one request when you run it; remote inference may incur charges.</p><label className="field-label">Model name<input value={model} onChange={event => setModel(event.target.value)} placeholder="Your upstream’s model name" list="configured-model-names" /><datalist id="configured-model-names">{configuredEntry?.model_names.map(name => <option key={name} value={name} />)}</datalist></label><div className="snippet-actions"><div className="segmented" aria-label="SDK language"><button className={language === 'python' ? 'selected' : ''} aria-pressed={language === 'python'} onClick={() => setLanguage('python')}>Python</button><button className={language === 'javascript' ? 'selected' : ''} aria-pressed={language === 'javascript'} onClick={() => setLanguage('javascript')}>JavaScript</button></div><button className="text-button" onClick={async () => { try { await navigator.clipboard.writeText(snippet); setCopyStatus('SDK example copied.'); } catch { setCopyStatus('Could not copy. Select the example to copy it manually.'); } }}>Copy example<ClipboardText size={15} /></button></div><pre className="json-view sdk-snippet" tabIndex={0} aria-label="SDK connection example">{snippet}</pre><p className="caption">{protocol === 'openai-decisions' ? language === 'python' ? 'openai 3.26.0 or later' : 'openai 7.30.0 or later' : protocol === 'cloudflare-systemone' ? language === 'python' ? 'Python standard library' : 'Node.js 18 or later' : language === 'python' ? 'typesafe-sdk 0.7.1' : '@typesafe-ai/sdk 0.6.0'} · <code>Accept-Encoding: identity</code> keeps typed answers inspectable. {local ? 'Use the saved workspace access token.' : 'Set JEV_OBSERVER_CLIENT_TOKEN to the local client token shown after registration.'}{localModel && ' Local model examples allow 180 seconds for startup and inference. Adjust timeout in the example for your hardware (Python: seconds; JavaScript: milliseconds).'}</p></section>
    <section className="detail-section" aria-label="Connection verification"><h3>First request check</h3><p role="status">{!health ? 'Waiting for collection health…' : health.persisted > 0 ? `${number(health.persisted)} captures saved in this Observer session.` : health.forwarded > 0 ? 'A request reached Observer. Waiting for its capture to be saved…' : 'Waiting for your application’s first request.'}</p>{Boolean(health?.truncated) && <p className="error-text">{number(health?.truncated)} captures are incomplete. Check identity encoding and capture limits.</p>}<p className="caption">After running your application, inspect its request in the workspace to confirm the model, answers and any capture warnings.</p></section>
    <section className="detail-section"><h3>Native endpoint</h3><code className="endpoint">POST {endpoint}</code>{!local && <p>Your SDK can also send its provider key directly when it includes <code>x-observer-access</code> with the workspace dashboard token. Registered keys are used only when the SDK sends the local client token.</p>}</section>
    <section className="detail-section"><h3>Keep sources recognizable</h3><p>Optionally add <code>x-observer-source</code> to name your application and <code>x-observer-task-version</code> when rules carried in state change.</p></section>
    <div className="inline-notice"><ShieldCheck size={19} /><span>The proxy is local. A hosted application cannot reach this machine’s loopback address. Import its exported records instead.</span></div>
    <p className="caption">Collection pressure does not intentionally hold up forwarding. Any missing captures are shown in collection health.</p>
    </>}
    <a className="text-button" href="#requests" onClick={() => document.querySelector<HTMLButtonElement>('.drawer-close')?.click()}><ArrowLeft size={14} />Back to your workspace</a>
  </div></>;
}
