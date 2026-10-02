import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import * as Dropdown from '@radix-ui/react-dropdown-menu';
import * as Dialog from '@radix-ui/react-dialog';
import { Aperture, ArrowDown, ArrowRight, ArrowSquareOut, ArrowsClockwise, CaretDown, ChartBar, Info, Clock, Database, Desktop, DownloadSimple, FunnelSimple, GearSix, List, MagnifyingGlass, Moon, Pause, Play, Plus, Pulse, Rows, ShieldCheck, Sun, UploadSimple, WarningCircle, X } from '@phosphor-icons/react';
import { defaultFilters, downloadExport, latestHealth, parentScope, query, readFilters, useDashboard, useLiveHealth, useResource } from './api';
import { bytes, compact, dateFilter, dateInput, dateTime, money, ms, number, percent, storageHint, time } from './format';
import { Brand, Distribution, Empty, ErrorNotice, KindBadge, Loading, RequestTable, SectionHeading, Timeline } from './components';
import type { Dashboard, Filters, Settings } from './types';
import type { Panel } from './Details';

const Details = lazy(() => import('./Details'));
const defaults = defaultFilters;

export default function App() {
  const [filters, setFilters] = useState<Filters>(readFilters);
  const [search, setSearch] = useState(() => filters.search), [groupSearch, setGroupSearch] = useState(() => filters.group_search ?? '');
  const [requestPages, setRequestPages] = useState<string[]>([]), [groupPages, setGroupPages] = useState<string[]>([]);
  const [datesOpen, setDatesOpen] = useState(() => Boolean(filters.from || filters.to));
  const [paused, setPaused] = useState(false), [panel, updatePanel] = useState<Panel>(null);
  const [displayed, setDisplayed] = useState<{ data: Dashboard; scope: string } | null>(null);
  const [selectedGroup, setSelectedGroup] = useState('');
  const [metric, setMetric] = useState<'requests' | 'latency' | 'cost'>('requests');
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme ?? 'light');
  const [toast, setToast] = useState(''), [sidebarOpen, setSidebarOpen] = useState(false);
  const [moreRequests, setMoreRequests] = useState(false);
  const [activeNav, setActiveNav] = useState('overview');
  const toastTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const searchRef = useRef<HTMLInputElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const mobileMenuRef = useRef<HTMLButtonElement>(null);
  const { snapshot, error, invalidate, reset, scope } = useDashboard(filters);
  const liveHealth = useLiveHealth();
  const { data: settings } = useResource<Settings>('/api/settings');
  const frozen = paused || panel !== null;

  useEffect(() => {
    if (snapshot && (!frozen || displayed?.scope !== snapshot.scope)) setDisplayed(snapshot);
  }, [snapshot, frozen, displayed?.scope]);
  useEffect(() => { const timer = setTimeout(() => setFilters(value => value.search === search ? value : ({ ...value, search, request_cursor: '', group_cursor: '' })), 250); return () => clearTimeout(timer); }, [search]);
  useEffect(() => { const timer = setTimeout(() => setFilters(value => (value.group_search ?? '') === groupSearch ? value : ({ ...value, group_search: groupSearch, group_cursor: '' })), 250); return () => clearTimeout(timer); }, [groupSearch]);
  useEffect(() => { window.history.replaceState(null, '', `${window.location.pathname}?${query(filters)}${window.location.hash}`); }, [filters]);
  useEffect(() => {
    const restore = () => { const next = readFilters(); setFilters(next); setSearch(next.search); setGroupSearch(next.group_search ?? ''); setRequestPages([]); setGroupPages([]); setDatesOpen(Boolean(next.from || next.to)); };
    window.addEventListener('popstate', restore);
    return () => window.removeEventListener('popstate', restore);
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('observer-theme', theme); } catch { /* Browser storage can be disabled. */ }
  }, [theme]);
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === 'k') { event.preventDefault(); if (!panel) searchRef.current?.focus(); }
    };
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  }, [panel]);
  useEffect(() => () => { if (toastTimer.current) clearTimeout(toastTimer.current); }, []);
  useEffect(() => {
    const mobile = matchMedia('(max-width: 560px)');
    const closeOnDesktop = () => { if (!mobile.matches) setSidebarOpen(false); };
    mobile.addEventListener('change', closeOnDesktop);
    return () => mobile.removeEventListener('change', closeOnDesktop);
  }, []);

  function notify(message: string) { setToast(message); if (toastTimer.current) clearTimeout(toastTimer.current); toastTimer.current = setTimeout(() => setToast(''), 6000); }
  function setPanel(value: Panel) {
    if (value && !panel) openerRef.current = sidebarOpen ? mobileMenuRef.current : document.activeElement instanceof HTMLElement ? document.activeElement : null;
    updatePanel(value);
    if (value) setSidebarOpen(false);
    if (!value) requestAnimationFrame(() => { if (openerRef.current?.isConnected) openerRef.current.focus(); });
  }
  function setFilter(key: keyof Filters, value: string) {
    setRequestPages([]); setGroupPages([]);
    setFilters(previous => ({ ...previous, request_cursor: '', group_cursor: '', ...(key === 'window' ? { from: '', to: '' } : {}), ...((key === 'from' || key === 'to') ? { window: 'all' } : {}), [key]: value }));
  }
  function pageRequests(cursor: string) { setRequestPages(previous => [...previous, filters.request_cursor ?? '']); setFilters(previous => ({ ...previous, request_cursor: cursor })); setMoreRequests(true); }
  function pageGroups(cursor: string) { setGroupPages(previous => [...previous, filters.group_cursor ?? '']); setFilters(previous => ({ ...previous, group_cursor: cursor })); }
  function newerRequests() { const cursor = requestPages.at(-1) ?? ''; setRequestPages(previous => previous.slice(0, -1)); setFilters(previous => ({ ...previous, request_cursor: cursor })); }
  function newerGroups() { const cursor = groupPages.at(-1) ?? ''; setGroupPages(previous => previous.slice(0, -1)); setFilters(previous => ({ ...previous, group_cursor: cursor })); }
  function historyChanged() { setDisplayed(null); setSelectedGroup(''); setRequestPages([]); setGroupPages([]); setFilters(previous => ({ ...previous, request_cursor: '', group_cursor: '' })); reset(); }
  function resetFilters() { setFilters(defaults); setSearch(''); setGroupSearch(''); setRequestPages([]); setGroupPages([]); setDatesOpen(false); }
  function navigate(section: string) { setActiveNav(section); setSidebarOpen(false); document.getElementById(section)?.scrollIntoView({ behavior: 'smooth', block: 'start' }); }
  const explorerLoading = Boolean(displayed && displayed.scope !== scope && parentScope(displayed.scope) === parentScope(scope));
  const data = displayed && (displayed.scope === scope || explorerLoading) ? displayed.data : null;
  const latest = snapshot?.scope === scope ? snapshot.data : null;
  // Catalogs describe retained history, independently of the selected scope.
  // Keep them during refreshes and keep active values visible after retention.
  const catalog = snapshot?.data ?? displayed?.data;
  const sources = [...new Set([...(catalog?.sources ?? []), filters.source].filter(Boolean))];
  const models = [...new Set([...(catalog?.models ?? []), filters.model].filter(Boolean))];
  // Either endpoint can fail independently. Use the freshest successful health
  // sample without discarding known gaps when a later refresh fails.
  const health = latestHealth(snapshot && { data: snapshot.data.health, requestOrder: snapshot.requestOrder }, liveHealth);
  const pending = latest && data ? Math.max(0, latest.health.persisted - data.health.persisted, latest.summary.request_count - data.summary.request_count) : 0;
  const filtered = Boolean(filters.source || filters.model || filters.status || filters.search || filters.group || filters.from || filters.to);
  const groups = data?.groups ?? [];
  const group = groups.find(item => item.id === selectedGroup) ?? groups[0];
  const summary = data?.summary;
  const gap = Boolean(health && (health.dropped > 0 || health.write_failures > 0 || (health.truncated ?? 0) > 0));
  const sample = Boolean(settings?.demo);
  const includesSamples = Boolean(data?.sample || sample);
  const hasRetainedHistory = Boolean(catalog?.sources.length);
  const connectionText = error ? 'Connection interrupted' : snapshot ? 'Connected locally' : 'Connecting';
  async function exportData(format: 'jsonl' | 'csv') {
    try { downloadExport(filters, format); notify(`${format.toUpperCase()} export requested. Your browser handles the download; server errors open separately.`); }
    catch (error) { notify(error instanceof Error ? error.message : 'Export failed.'); }
  }

  const sidebar = <>
      <Brand onNavigate={() => navigate('overview')} />
      <div className="workspace-switch"><div className="workspace-monogram"><Desktop size={18} weight="bold" /></div><div><strong>Local workspace</strong><span>{sample ? 'Sample environment' : 'On this machine'}</span></div><span className={`connection-dot ${error ? 'offline' : ''}`} title={connectionText} /></div>
      <div className="nav-section-label">WORKSPACE</div>
      <nav className="navigation"><button className={activeNav === 'overview' ? 'active' : ''} onClick={() => navigate('overview')}><ChartBar size={19} weight={activeNav === 'overview' ? 'fill' : 'bold'} />Overview</button><button className={activeNav === 'questions' ? 'active' : ''} onClick={() => navigate('questions')}><Rows size={19} weight="bold" />Question groups{data && <span className="nav-count">{compact(data.groups.length)}</span>}</button><button className={activeNav === 'requests' ? 'active' : ''} onClick={() => navigate('requests')}><Pulse size={19} weight="bold" />Requests</button></nav>
      <div className="nav-section-label second">WORKSPACE DATA</div>
      <nav className="navigation"><button onClick={() => setPanel({ type: 'import' })}><UploadSimple size={19} weight="bold" />Import records</button><button onClick={() => setPanel({ type: 'settings' })}><GearSix size={19} weight="bold" />Settings</button></nav>
      <div className="sidebar-bottom"><div className="local-note"><ShieldCheck size={21} weight="duotone" /><strong>Your history stays here.</strong><p>Local storage. No account.<br />No automatic uploads.</p></div><button className="sidebar-connect" onClick={() => setPanel({ type: 'connect' })}><Plus size={17} weight="bold" />Connect an application<ArrowSquareOut size={14} /></button><div className="sidebar-foot"><span className={`connection-dot ${error ? 'offline' : ''}`} />{connectionText}</div></div>
  </>;

  return <Dialog.Root open={sidebarOpen} onOpenChange={setSidebarOpen}><div className="app-shell">
    <a href="#main" className="skip-link">Skip to dashboard</a>
    <aside className="sidebar" aria-label="Workspace navigation">{sidebar}</aside>
    <Dialog.Portal><Dialog.Overlay className="mobile-shade" /><Dialog.Content className="sidebar is-open mobile-navigation" aria-describedby={undefined} onCloseAutoFocus={event => { if (panel) event.preventDefault(); }}>
      <Dialog.Title className="sr-only">Workspace navigation</Dialog.Title>
      <Dialog.Close className="icon-button navigation-close" aria-label="Close navigation"><X size={18} /></Dialog.Close>
      {sidebar}
    </Dialog.Content></Dialog.Portal>

    <div className="main-shell"><header className="topbar"><div className="breadcrumb"><Dialog.Trigger asChild><button ref={mobileMenuRef} className="icon-button mobile-menu" aria-label="Open navigation"><List size={22} /></button></Dialog.Trigger><Brand className="mobile-brand" onNavigate={() => navigate('overview')} /><span className="breadcrumb-location"><span>Workspace</span><span className="breadcrumb-slash">/</span><strong>Overview</strong></span>{includesSamples && <span className="sample-badge">{sample ? 'Sample data' : 'Includes samples'}</span>}</div><div className="topbar-actions"><span className="local-only"><Database size={14} />Local storage</span><button className="icon-button" onClick={() => setTheme(theme === 'light' ? 'dark' : 'light')} aria-label={`Switch to ${theme === 'light' ? 'dark' : 'light'} theme`}>{theme === 'light' ? <Moon size={19} /> : <Sun size={19} />}</button><button className="icon-button" onClick={() => setPanel({ type: 'settings' })} aria-label="Open settings"><GearSix size={19} /></button></div></header>

      <main id="main"><section id="overview" className="overview-header"><div><div className="eyebrow">YOUR DECISION WORKSPACE</div><h1>Workspace overview</h1><p>Follow your questions. Understand every request.</p></div><div className="overview-actions"><Dropdown.Root><Dropdown.Trigger className="button"><DownloadSimple size={16} />Export<CaretDown size={13} /></Dropdown.Trigger><Dropdown.Portal><Dropdown.Content className="dropdown-content" align="end" sideOffset={6}><Dropdown.Label className="dropdown-label">Export current filters</Dropdown.Label><Dropdown.Item onSelect={() => void exportData('jsonl')}><DownloadSimple size={15} />JSONL records<span>Full detail</span></Dropdown.Item><Dropdown.Item onSelect={() => void exportData('csv')}><DownloadSimple size={15} />CSV spreadsheet</Dropdown.Item><Dropdown.Separator /><Dropdown.Item onSelect={() => setPanel({ type: 'import' })}><UploadSimple size={15} />Import local records</Dropdown.Item></Dropdown.Content></Dropdown.Portal></Dropdown.Root><button className={`button ${paused ? 'paused-button' : 'live-button'}`} onClick={() => setPaused(!paused)} aria-pressed={paused}>{paused ? <Play size={14} weight="fill" /> : <Pause size={14} weight="fill" />}{paused ? 'Resume live' : 'Live updates'}</button></div></section>

      {includesSamples && <div className="sample-notice"><span className="sample-mark"><Aperture size={16} weight="bold" /></span><span><strong>{sample ? 'A workspace to explore.' : 'Sample records included.'}</strong> {sample ? 'These are synthetic records; forwarding is disabled until you restart without --demo.' : 'This window includes synthetic observations. Their costs are not real charges.'}</span><button className="text-button" onClick={() => setPanel({ type: 'connect' })}>{sample ? 'See live setup' : 'Connect your app'}<ArrowRight size={14} /></button></div>}
      {(error || !data) && gap && <div className="warning-notice" role="status"><WarningCircle size={18} /><span>Collection history is incomplete: {number(health?.dropped)} dropped, {number(health?.truncated ?? 0)} incomplete captures, {number(health?.write_failures)} write failures. These counters are independent of history storage.</span></div>}
      {error && <ErrorNotice message={`${error}. ${data ? 'Showing the last loaded snapshot; values may be stale.' : 'Check that your local Observer service is running.'}`} retry={invalidate} />}

      <div className="filterbar"><div className="filter-select"><FunnelSimple size={15} /><select aria-label="Filter by source" value={filters.source} onChange={event => setFilter('source', event.target.value)}><option value="">All sources</option>{sources.map(source => <option key={source}>{source}</option>)}</select><CaretDown size={12} /></div><div className="filter-select"><select aria-label="Filter by model" value={filters.model} onChange={event => setFilter('model', event.target.value)}><option value="">All models</option>{models.map(model => <option key={model}>{model}</option>)}</select><CaretDown size={12} /></div><div className="filter-divider" /><label className="search-box"><span className="search-symbol"><MagnifyingGlass size={15} /></span><input ref={searchRef} value={search} onChange={event => { setRequestPages([]); setGroupPages([]); setSearch(event.target.value); }} aria-label="Search requests" placeholder="Search requests…" /><kbd>{/Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl'} K</kbd></label><div className="filter-select time-select"><Clock size={15} /><select aria-label="Time window" value={filters.window} onChange={event => setFilter('window', event.target.value)}><option value="1h">Last hour</option><option value="24h">Last 24 hours</option><option value="7d">Last 7 days</option><option value="all">All history</option></select><CaretDown size={12} /></div><button className="icon-button refresh-button" onClick={invalidate} aria-label="Refresh dashboard"><ArrowsClockwise size={17} /></button></div>
      <div className="date-range-toggle"><button className="text-button" aria-expanded={datesOpen} onClick={() => setDatesOpen(!datesOpen)}><Clock size={14} />Custom date range</button><span className="caption">Filters are saved in this page’s URL.</span></div>
      {datesOpen && <div className="date-range"><label className="field-label">From (local time)<input aria-label="From date" type="datetime-local" value={dateInput(filters.from)} onChange={event => setFilter('from', dateFilter(event.target.value))} /></label><label className="field-label">To (local time)<input aria-label="To date" type="datetime-local" value={dateInput(filters.to)} min={dateInput(filters.from)} onChange={event => setFilter('to', dateFilter(event.target.value))} /></label>{filters.from && filters.to && Number(filters.from) > Number(filters.to) && <p className="error-text" role="alert">The end must be after the start.</p>}</div>}
      {filtered && <div className="active-filters"><span>Filtered view</span>{filters.status && <button onClick={() => setFilter('status', '')}>Failures only<X size={12} /></button>}{filters.group && <button onClick={() => setFilter('group', '')}>Selected question group<X size={12} /></button>}<button className="text-button" onClick={resetFilters}>Clear filters</button></div>}
      {frozen && <div className="pause-notice"><Pause size={15} weight="fill" /><span>{panel ? 'The visible dashboard is held while you inspect.' : 'The visible dashboard is paused.'} Collection continues.{pending > 0 && ` ${number(pending)} newer records available.`}</span>{!panel && <button className="text-button" onClick={() => setPaused(false)}>Show latest<ArrowDown size={13} /></button>}</div>}

      {!data ? error ? <Empty title="Waiting for your local workspace">The dashboard will reconnect automatically. Stored history stays on your machine.<button className="button retry-empty" onClick={invalidate}>Try again</button></Empty> : <Loading /> : <>
      <section className="metrics" aria-label="Selected window summary"><button className="metric" onClick={() => { setFilter('status', ''); navigate('requests'); }}><span className="metric-label">Requests<ArrowSquareOut size={13} /></span><span className="metric-value">{number(summary!.request_count)}</span><span className="metric-foot">{number(summary!.answer_count)} answers captured{summary!.action_count ? ` · ${number(summary!.action_count)} actions` : ''}</span></button><button className={`metric ${summary!.error_count ? 'metric-error' : ''}`} onClick={() => setFilter('status', filters.status ? '' : 'error')} aria-pressed={filters.status === 'error'}><span className="metric-label">Failures<WarningCircle size={14} /></span><span className="metric-value">{number(summary!.error_count)}</span><span className="metric-foot">{summary!.request_count ? percent(summary!.error_count / summary!.request_count) : '—'} of recorded requests</span></button><button className="metric" onClick={() => setMetric('latency')}><span className="metric-label">Request latency<Clock size={14} /></span><span className="metric-value">{ms(summary!.p50_ms)}<small>p50</small></span><span className="metric-foot">{ms(summary!.p95_ms)} p95 · observed round trip</span></button><div className="metric"><span className="metric-label">Reported tokens<Pulse size={14} /></span><span className="metric-value">{summary!.input_tokens == null || summary!.output_tokens == null ? 'Unknown' : compact(summary!.input_tokens + summary!.output_tokens)}</span><span className="metric-foot">{compact(summary!.input_tokens)} in / {compact(summary!.output_tokens)} out</span></div><button className="metric" onClick={() => setMetric('cost')}><span className="metric-label">{summary!.cost_known_requests < summary!.request_count ? 'Known cost' : 'Request cost'}<Database size={14} /></span><span className="metric-value">{money(summary!.cost_usd)}</span><span className="metric-foot">{sample ? 'Synthetic · ' : includesSamples ? 'Includes samples · ' : ''}{number(summary!.cost_known_requests)} / {number(summary!.request_count)} requests covered</span></button></section>

      <div className="activity-grid"><section className="panel activity-panel"><SectionHeading title="Request activity" description={metric === 'requests' ? 'Recorded requests and failures over time' : metric === 'latency' ? 'Mean observed latency in each time bucket' : 'Known request cost in each time bucket'} action={<div className="segmented" aria-label="Timeline metric">{(['requests', 'latency', 'cost'] as const).map(item => <button key={item} aria-pressed={metric === item} className={metric === item ? 'selected' : ''} onClick={() => setMetric(item)}>{item === 'requests' ? 'Volume' : item === 'latency' ? 'Latency' : 'Cost'}</button>)}</div>} /><Timeline data={data.timeline} meta={data.timeline_meta} metric={metric} /><div className="chart-footer"><div className="chart-legend"><span><i />{metric === 'requests' ? 'Requests' : metric === 'latency' ? 'Mean latency · ms' : 'Known cost · USD'}</span>{metric === 'requests' && <span><i className="failure-key" />Failures</span>}</div><span>Captured history · {filters.from || filters.to ? 'custom date range' : filters.window === 'all' ? data.timeline_meta?.truncated === false ? 'full retained time range' : 'displayed time buckets' : `last ${filters.window}`}</span></div></section>
      <section className={`panel health-panel ${gap ? 'has-gap' : ''}`}><div className="health-heading"><span className={`health-symbol ${gap ? 'warning' : ''}`}>{gap ? <WarningCircle size={21} weight="duotone" /> : <ShieldCheck size={21} weight="duotone" />}</span><div><h2>Collection health</h2><span>{gap ? 'History gaps detected' : error ? 'History unavailable' : health?.queue_depth ? 'Recording activity' : 'Ready to observe'}</span></div></div><dl className="health-fields"><div><dt>Capture queue</dt><dd>{number(health?.queue_depth ?? 0)} <span>pending</span></dd></div><div><dt>Write lag</dt><dd>{ms(health?.lag_ms)}</dd></div><div><dt>Dropped captures</dt><dd className={health?.dropped ? 'error-text' : ''}>{number(health?.dropped ?? 0)}</dd></div><div><dt>Incomplete captures</dt><dd className={health?.truncated ? 'error-text' : ''}>{number(health?.truncated ?? 0)}</dd></div><div><dt>Write failures</dt><dd className={health?.write_failures ? 'error-text' : ''}>{number(health?.write_failures ?? 0)}</dd></div></dl><p className="health-note">{gap ? `Last gap ${health?.last_gap_at ? dateTime(health.last_gap_at) : 'time unknown'}. Affected history is incomplete.` : 'Current process. Calls keep moving if recording falls behind.'}</p>{health?.maintenance_healthy === false && <p className="error-text" role="status">Retention cleanup failed. Older records may remain while Observer retries.</p>}{health?.last_error && <p className="health-error-detail" role="status"><strong>Last storage issue · {dateTime(health.last_error.at)}</strong><span>{storageHint(health.last_error.category)}</span><span>Historical report from this process; recent successful captures may indicate recovery.</span></p>}<button className="text-button health-settings" onClick={() => setPanel({ type: 'settings' })}>{settings ? settings.capture_state ? 'Input capture enabled' : 'Raw input is not retained' : 'View capture settings'}<ArrowRight size={13} /></button></section></div>

      <div className="explorer-grid"><section className="panel questions-panel" id="questions"><SectionHeading title="Recurring questions" description="Retained definitions and indexed families" action={<span className="count-badge">{number(data.groups.length)}</span>} />
        <label className="group-search"><FunnelSimple size={14} /><input value={groupSearch} onChange={event => { setGroupPages([]); setGroupSearch(event.target.value); }} placeholder="Search all question groups…" aria-label="Find a question group" /></label>{groupSearch && <p className="caption group-loading">Searching groups in this scope. Request totals are unchanged.</p>}{explorerLoading && <p className="caption group-loading" role="status">Loading history…</p>}{data.groups.length > 0 ? <><div className="group-list" aria-busy={explorerLoading} role="list" aria-label="Question groups">{groups.length ? groups.map(item => <div key={item.id} role="listitem"><button className={`group-row ${group?.id === item.id ? 'selected' : ''}`} onClick={() => setSelectedGroup(item.id)} aria-pressed={group?.id === item.id}><span className={`group-type-icon kind-${item.kind}`}><Rows size={16} weight="bold" /></span><span className="group-row-main"><strong title={item.name}>{item.name}</strong><span>{item.source}{item.version_count > 1 && <span className="version-mark">{item.version_count} {item.is_family ? 'definitions' : 'versions'}</span>}</span></span><span className="group-row-count">{compact(item.answer_count)}<small>answers</small></span></button></div>) : <p className="group-search-empty">No questions match this search.</p>}</div>
        {(filters.group_cursor || data.group_next_cursor) && <div className="history-pagination" aria-label="Question group pages"><button className="text-button" disabled={explorerLoading || !filters.group_cursor} onClick={newerGroups}>Newer groups</button><span>{number(groups.length)} groups</span><button className="text-button" disabled={explorerLoading || !data.group_next_cursor} onClick={() => data.group_next_cursor && pageGroups(data.group_next_cursor)}>Older groups</button></div>}
        {group && <div className="selected-distribution"><div className="distribution-heading"><div><strong>{group.name}</strong><div className="distribution-meta"><KindBadge kind={group.kind} /><span>{number(group.valid_count)} valid / {number(group.answer_count)} answers</span></div></div><button className="icon-button" onClick={() => setPanel({ type: 'group', id: group.id })} aria-label={`Inspect group ${group.name}`}><ArrowSquareOut size={18} /></button></div><Distribution group={group} /><div className="distribution-bottom"><span>{number(group.request_count)} distinct requests</span><button className="text-button" onClick={() => setPanel({ type: 'group', id: group.id })}>Inspect group<ArrowRight size={13} /></button></div></div>}</> : <Empty title={groupSearch ? 'No questions match this search' : 'Patterns start with a question'}>{groupSearch ? 'Try another question name or clear the group search.' : 'Repeated definitions will form groups automatically. Changed criteria keep their own versions.'}{filters.group_cursor && <button className="text-button" onClick={newerGroups}>Newer groups</button>}</Empty>}
      </section>

      <section className="panel requests-panel" id="requests"><SectionHeading title="Request stream" description={`${filters.request_cursor ? 'Older' : 'Latest'} ${Math.min(data.requests.length, moreRequests ? 100 : 12)} records · totals above cover the whole window`} action={<button className={`filter-toggle ${filters.status === 'error' ? 'selected' : ''}`} onClick={() => setFilter('status', filters.status === 'error' ? '' : 'error')} aria-pressed={filters.status === 'error'}><WarningCircle size={14} />Failures</button>} />{data.requests.length ? <><div className="request-feed"><RequestTable requests={data.requests.slice(0, moreRequests ? 100 : 12)} open={id => setPanel({ type: 'request', id })} /></div><div className="requests-footer"><span><span className={`connection-dot ${frozen ? 'paused' : error ? 'offline' : ''}`} />{frozen ? 'View paused · collection continues' : error ? 'Last loaded history' : 'Live snapshots'}</span>{data.requests.length > 12 && <button className="text-button" onClick={() => setMoreRequests(!moreRequests)}>{moreRequests ? 'Show fewer' : `Show ${Math.min(100, data.requests.length)} records`}<CaretDown size={13} /></button>}</div>{(filters.request_cursor || data.feed_next_cursor) && <div className="history-pagination" aria-label="Request pages"><button className="text-button" disabled={explorerLoading || !filters.request_cursor} onClick={newerRequests}>Newer requests</button><button className="text-button" disabled={explorerLoading || !data.feed_next_cursor} onClick={() => data.feed_next_cursor && pageRequests(data.feed_next_cursor)}>Older requests</button></div>}</> : <Empty title={filtered ? 'No requests match these filters' : hasRetainedHistory && filters.window !== 'all' ? 'No requests in this window' : 'Your workspace is ready'}>{filtered ? 'Try a wider time window or clear your filters.' : hasRetainedHistory && filters.window !== 'all' ? 'Older records are available in this workspace.' : 'Connect a local application or import existing records to start exploring.'}<div className="button-row empty-actions">{filters.request_cursor && <button className="button" onClick={newerRequests}>Newer requests</button>}{filtered ? <button className="button" onClick={resetFilters}>Clear filters</button> : hasRetainedHistory && filters.window !== 'all' ? <button className="button primary" onClick={() => setFilter('window', 'all')}>Show all history</button> : <><button className="button primary" onClick={() => setPanel({ type: 'connect' })}><Plus size={15} />Connect app</button><button className="button" onClick={() => setPanel({ type: 'import' })}>Import records</button></>}</div></Empty>}</section></div>

      <footer className="workspace-footer"><span><ShieldCheck size={14} />{sample ? 'Sample workspace' : 'Local workspace'}<span className="footer-divider" />{health ? `${bytes(health.queued_bytes)} queued` : ''}</span><span>Snapshot {time(data.generated_at)}<span className="footer-divider" />Times shown locally</span></footer>
      </>}
      </main>
    </div>
    {panel && <Suspense fallback={panel ? <div className="lazy-detail-loading" role="status">Opening details…</div> : null}><Details health={health} panel={panel} setPanel={setPanel} filters={filters} changed={invalidate} historyChanged={historyChanged} filterGroup={id => { setFilter('group', id); setPanel(null); navigate('requests'); }} notify={notify} /></Suspense>}
    {toast && <div className="toast" role="status"><Info size={18} /><span>{toast}</span><button className="icon-button" aria-label="Dismiss notification" onClick={() => setToast('')}><X size={15} /></button></div>}
  </div></Dialog.Root>;
}
