import { useEffect, useState } from 'react';
import { defaultFilters, latestHealth, parentScope, query, readFilters, useDashboard, useLiveHealth, useResource } from '../api';
import { dateInput } from '../format';
import type { Dashboard, Filters, Settings } from '../types';

const defaults = defaultFilters;
type PageNavigation = { errorRequestOrder: number; scope: string; returnScope: string; filters: Filters; requestPages: string[]; groupPages: string[] };

// Keep navigation transactions and their loaded snapshots in one owner.
export function useDashboardWorkspace(panelOpen: boolean) {
  const [filters, setFilters] = useState<Filters>(readFilters);
  const [search, setSearch] = useState(() => filters.search), [groupSearch, setGroupSearch] = useState(() => filters.group_search ?? '');
  const [searchComposing, setSearchComposing] = useState(false), [groupSearchComposing, setGroupSearchComposing] = useState(false);
  const [requestPages, setRequestPages] = useState<string[]>([]), [groupPages, setGroupPages] = useState<string[]>([]);
  const [pageNavigation, setPageNavigation] = useState<PageNavigation | null>(null);
  const [pageFailure, setPageFailure] = useState<{ message: string; navigation: PageNavigation } | null>(null);
  const [datesOpen, setDatesOpen] = useState(() => Boolean(filters.from || filters.to));
  const [paused, setPaused] = useState(false);
  const [displayed, setDisplayed] = useState<{ data: Dashboard; scope: string } | null>(null);
  const [historyRevision, setHistoryRevision] = useState(0);
  const [selectedGroup, setSelectedGroup] = useState('');
  const [metric, setMetric] = useState<'requests' | 'latency' | 'cost'>('requests');
  const [moreRequests, setMoreRequests] = useState(false);
  const { snapshot, error, errorRequestOrder, slow, invalidate, reset, scope } = useDashboard(filters);
  const liveHealth = useLiveHealth();
  const { data: settings } = useResource<Settings>('/api/settings');
  const frozen = paused || panelOpen;

  useEffect(() => {
    if (snapshot && (!frozen || displayed?.scope !== snapshot.scope)) setDisplayed(snapshot);
  }, [snapshot, frozen, displayed?.scope]);
  useEffect(() => {
    if (!pageNavigation) return;
    if (pageNavigation.scope !== scope) {
      const previous = readFilters(pageNavigation.returnScope);
      const target = pageNavigation.filters;
      const requestNavigation = (target.request_cursor ?? '') !== (previous.request_cursor ?? '');
      if (requestNavigation && parentScope(pageNavigation.scope) === parentScope(scope)
        && (target.request_cursor ?? '') === (filters.request_cursor ?? '')
        && (target.group_search ?? '') !== (filters.group_search ?? '') && !filters.group_cursor) {
        // Group search changes only the group page. Carry the pending request
        // push/pop and its retry/rollback into that new scope together.
        setPageNavigation({ ...pageNavigation, scope, filters, groupPages: [],
          returnScope: query({ ...filters, request_cursor: previous.request_cursor ?? '', group_cursor: '' }),
        });
      } else setPageNavigation(null);
      return;
    }
    if (snapshot?.scope === scope) {
      setRequestPages(pageNavigation.requestPages); setGroupPages(pageNavigation.groupPages);
      setPageNavigation(null);
    } else if (error && errorRequestOrder !== pageNavigation.errorRequestOrder) {
      // A failed attempt never enters history. Restore the loaded page while
      // retaining an explicit retry, independently of subsequent live polls.
      setPageFailure({ message: error, navigation: pageNavigation });
      setPageNavigation(null);
      const previous = readFilters(pageNavigation.returnScope);
      setFilters(value => ({ ...value, request_cursor: previous.request_cursor ?? '', group_cursor: previous.group_cursor ?? '' }));
    }
  }, [pageNavigation, scope, snapshot, error, errorRequestOrder, filters]);
  useEffect(() => {
    if (pageFailure && pageFailure.navigation.returnScope !== scope) setPageFailure(null);
  }, [scope, pageFailure]);
  // IME drafts remain editable without replacing the committed scope or cursor history.
  useEffect(() => { if (searchComposing) return; const timer = setTimeout(() => setFilters(value => value.search === search ? value : ({ ...value, search, request_cursor: '', group_cursor: '' })), 250); return () => clearTimeout(timer); }, [search, searchComposing]);
  useEffect(() => { if (groupSearchComposing) return; const timer = setTimeout(() => setFilters(value => (value.group_search ?? '') === groupSearch ? value : ({ ...value, group_search: groupSearch, group_cursor: '' })), 250); return () => clearTimeout(timer); }, [groupSearch, groupSearchComposing]);
  // Draft edits can be undone before debounce; only committed searches change the page scope.
  useEffect(() => { setRequestPages(previous => previous.length ? [] : previous); setGroupPages(previous => previous.length ? [] : previous); }, [filters.search]);
  useEffect(() => { setGroupPages(previous => previous.length ? [] : previous); }, [filters.group_search]);
  useEffect(() => { window.history.replaceState(null, '', `${window.location.pathname}?${query(filters)}${window.location.hash}`); }, [filters]);
  useEffect(() => {
    const restore = () => { const next = readFilters(); setPageNavigation(null); setPageFailure(null); setFilters(next); setSearch(next.search); setGroupSearch(next.group_search ?? ''); setSearchComposing(false); setGroupSearchComposing(false); setRequestPages([]); setGroupPages([]); setDatesOpen(Boolean(next.from || next.to)); };
    window.addEventListener('popstate', restore);
    return () => window.removeEventListener('popstate', restore);
  }, []);
  function setFilter(key: keyof Filters, value: string) {
    setPageNavigation(null); setPageFailure(null);
    setRequestPages([]); setGroupPages([]);
    setFilters(previous => ({ ...previous, request_cursor: '', group_cursor: '', ...(key === 'window' ? { from: '', to: '' } : {}), ...((key === 'from' || key === 'to') ? { window: 'all' } : {}), [key]: value }));
  }
  function navigatePage(next: Filters, nextRequestPages = requestPages, nextGroupPages = groupPages) {
    setPageFailure(null);
    setPageNavigation({ errorRequestOrder, scope: query(next), returnScope: displayed?.scope ?? scope, filters: next, requestPages: nextRequestPages, groupPages: nextGroupPages });
    setFilters(next);
  }
  function pageRequests(cursor: string) { navigatePage({ ...filters, request_cursor: cursor }, [...requestPages, filters.request_cursor ?? '']); setMoreRequests(true); }
  function pageGroups(cursor: string) { navigatePage({ ...filters, group_cursor: cursor }, requestPages, [...groupPages, filters.group_cursor ?? '']); }
  function newerRequests() { navigatePage({ ...filters, request_cursor: requestPages.at(-1) ?? '' }, requestPages.slice(0, -1)); }
  function newerGroups() { navigatePage({ ...filters, group_cursor: groupPages.at(-1) ?? '' }, requestPages, groupPages.slice(0, -1)); }
  function historyChanged() { setPageNavigation(null); setPageFailure(null); setHistoryRevision(value => value + 1); setDisplayed(null); setSelectedGroup(''); setRequestPages([]); setGroupPages([]); setFilters(previous => ({ ...previous, request_cursor: '', group_cursor: '' })); reset(); }
  function resetFilters() { setPageNavigation(null); setPageFailure(null); setFilters(defaults); setSearch(''); setGroupSearch(''); setSearchComposing(false); setGroupSearchComposing(false); setRequestPages([]); setGroupPages([]); setDatesOpen(false); }
  const explorerLoading = Boolean(displayed && displayed.scope !== scope && parentScope(displayed.scope) === parentScope(scope));
  const data = displayed && (displayed.scope === scope || explorerLoading) ? displayed.data : null;
  useEffect(() => {
    if (!data && groupSearchComposing) {
      // A parent filter can unmount the group input without compositionend.
      // Discard that unfinished draft so a later input remains searchable.
      setGroupSearch(filters.group_search ?? ''); setGroupSearchComposing(false);
    }
  }, [data, groupSearchComposing, filters.group_search]);
  const displayedRequestCursor = new URLSearchParams(displayed?.scope ?? '').get('request_cursor');
  const pageLoading = explorerLoading || pageNavigation !== null;
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
  const fromInput = dateInput(filters.from), toInput = dateInput(filters.to);
  const groups = data?.groups ?? [];
  const group = groups.find(item => item.id === selectedGroup) ?? groups[0];
  const summary = data?.summary;
  const gap = Boolean(health && (health.dropped > 0 || health.write_failures > 0 || (health.truncated ?? 0) > 0));
  const sample = Boolean(settings?.demo);
  const includesSamples = Boolean(data?.sample || sample);
  const hasRetainedHistory = Boolean(catalog?.sources.length);
  const connectionText = error ? 'Connection interrupted' : snapshot ? 'Connected locally' : 'Connecting';
  return { filters, search, setSearch, setSearchComposing, groupSearch, setGroupSearch, setGroupSearchComposing, datesOpen, setDatesOpen, paused, setPaused, historyRevision, selectedGroup, setSelectedGroup, metric, setMetric, moreRequests, setMoreRequests, error, slow, invalidate, scope, settings, frozen, pageFailure, setFilter, navigatePage, pageRequests, pageGroups, newerRequests, newerGroups, historyChanged, resetFilters, explorerLoading, data, displayedRequestCursor, pageLoading, sources, models, health, pending, filtered, fromInput, toInput, groups, group, summary, gap, sample, includesSamples, hasRetainedHistory, connectionText };
}

export type DashboardWorkspace = ReturnType<typeof useDashboardWorkspace>;
