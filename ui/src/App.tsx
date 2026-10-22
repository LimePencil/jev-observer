import { Aperture, ArrowDown, ArrowRight, CaretDown, Clock, DownloadSimple, Pause, Play, UploadSimple, WarningCircle } from '@phosphor-icons/react';
import * as Dropdown from '@radix-ui/react-dropdown-menu';
import { lazy, Suspense } from 'react';
import { downloadExport } from './api';
import { Empty, ErrorNotice, Loading } from './components';
import { DetailBoundary, DetailFallback } from './DetailBoundary';
import { number } from './format';

import { DashboardContent } from './app/DashboardContent';
import { DashboardFilters } from './app/DashboardFilters';
import { useAppShell } from './app/useAppShell';
import { useDashboardWorkspace } from './app/useDashboardWorkspace';
import { useWorkspaceControllers } from './app/useWorkspaceControllers';
import { WorkspaceShell } from './app/WorkspaceShell';

const Details = lazy(() => import('./Details'));

export default function App() {
  const shell = useAppShell();
  const workspace = useDashboardWorkspace(shell.panel !== null);
  const { panel, setPanel, notify, mainRef, exportRef, searchRef, navigate } = shell;
  const { filters, historyRevision, historyChanged, invalidate, data, health, sample, includesSamples, error, connectionText, paused, setPaused, gap, pageFailure, navigatePage, slow, frozen, pending, setFilter } = workspace;
  const controllers = useWorkspaceControllers(panel, historyRevision, invalidate, historyChanged, notify);
  async function exportData(format: 'jsonl' | 'csv') {
    try { downloadExport(filters, format); notify(`${format.toUpperCase()} export requested. Your browser handles the download; server errors open separately.`); }
    catch (error) { notify(error instanceof Error ? error.message : 'Export failed.'); }
  }

  return <WorkspaceShell state={shell} data={data} sample={sample} includesSamples={includesSamples} error={error} connectionText={connectionText}
    details={panel && <DetailBoundary close={() => setPanel(null)}><Suspense fallback={<DetailFallback close={() => setPanel(null)} />}><Details health={health} panel={panel} setPanel={setPanel} filters={filters} {...controllers} historyRevision={historyRevision} filterGroup={id => { setFilter('group', id); setPanel(null); navigate('requests'); }} /></Suspense></DetailBoundary>}>
      <main id="main" ref={mainRef} tabIndex={-1}><section id="overview" className="overview-header"><div><div className="eyebrow">YOUR DECISION WORKSPACE</div><h1>Workspace overview</h1><p>Follow your questions. Understand every request.</p></div><div className="overview-actions"><Dropdown.Root><Dropdown.Trigger ref={exportRef} className="button"><DownloadSimple size={16} />Export<CaretDown size={13} /></Dropdown.Trigger><Dropdown.Portal><Dropdown.Content className="dropdown-content" align="end" sideOffset={6}><Dropdown.Label className="dropdown-label">Export current filters</Dropdown.Label><Dropdown.Item onSelect={() => void exportData('jsonl')}><DownloadSimple size={15} />JSONL records<span>Full detail</span></Dropdown.Item><Dropdown.Item onSelect={() => void exportData('csv')}><DownloadSimple size={15} />CSV spreadsheet</Dropdown.Item><Dropdown.Separator /><Dropdown.Item onSelect={() => setPanel({ type: 'import' }, exportRef.current)}><UploadSimple size={15} />Import local records</Dropdown.Item></Dropdown.Content></Dropdown.Portal></Dropdown.Root><button className={`button ${paused ? 'paused-button' : 'live-button'}`} onClick={() => setPaused(!paused)} aria-pressed={paused}>{paused ? <Play size={14} weight="fill" /> : <Pause size={14} weight="fill" />}{paused ? 'Resume live' : 'Live updates'}</button></div></section>

      {includesSamples && <div className="sample-notice"><span className="sample-mark"><Aperture size={16} weight="bold" /></span><span><strong>{sample ? 'A workspace to explore.' : 'Sample records included.'}</strong> {sample ? 'These are synthetic records; forwarding is disabled until you restart without --demo.' : 'This window includes synthetic observations. Their costs are not real charges.'}</span><button className="text-button" onClick={() => setPanel({ type: 'connect' })}>{sample ? 'See live setup' : 'Connect your app'}<ArrowRight size={14} /></button></div>}
      {(error || !data) && gap && <div className="warning-notice" role="status"><WarningCircle size={18} /><span>Collection history is incomplete: {number(health?.dropped)} dropped, {number(health?.truncated ?? 0)} incomplete captures, {number(health?.write_failures)} write failures. These counters are independent of history storage.</span></div>}
      {pageFailure && <ErrorNotice message={`Could not load the requested history page: ${pageFailure.message}. Showing the last successfully loaded page.`} retry={() => { const navigation = pageFailure.navigation; navigatePage(navigation.filters, navigation.requestPages, navigation.groupPages); }} />}
      {error && <ErrorNotice message={`${error}. ${data ? 'Showing the last loaded snapshot; values may be stale.' : 'Check that your local Observer service is running.'}`} retry={invalidate} />}
      {slow && <div className="inline-notice" role="status"><Clock size={18} /><span>History query is still running. You can narrow the time window or filters.</span></div>}

      <DashboardFilters workspace={workspace} searchRef={searchRef} />
      {frozen && <div className="pause-notice"><Pause size={15} weight="fill" /><span>{panel ? 'The visible dashboard is held while you inspect.' : 'The visible dashboard is paused.'} Collection continues.{pending > 0 && ` ${number(pending)} newer records available.`}</span>{!panel && <button className="text-button" onClick={() => setPaused(false)}>Show latest<ArrowDown size={13} /></button>}</div>}

      {!data ? error ? <Empty title="Waiting for your local workspace">The dashboard will reconnect automatically. Stored history stays on your machine.<button className="button retry-empty" onClick={invalidate}>Try again</button></Empty> : <Loading /> : <>
      <DashboardContent workspace={workspace} data={data} shell={shell} />
      </>}
      </main>
  </WorkspaceShell>;
}
