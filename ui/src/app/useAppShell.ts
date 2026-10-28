import { useCallback, useEffect, useRef, useState } from 'react';
import type { Panel } from '../details/types';

export function useAppShell() {
  const [panel, updatePanel] = useState<Panel>(null);
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme ?? 'light');
  const [toast, setToast] = useState(''), [sidebarOpen, setSidebarOpen] = useState(false);
  const [activeNav, setActiveNav] = useState('overview');
  const [deferredNavigation, setDeferredNavigation] = useState<{ section: string; focus: boolean } | null>(null);
  const menuNavigation = useRef<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const searchRef = useRef<HTMLInputElement>(null);
  const mainRef = useRef<HTMLElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const restoreFocusFrame = useRef<number | null>(null);
  const exportRef = useRef<HTMLButtonElement>(null);
  const mobileMenuRef = useRef<HTMLButtonElement>(null);
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
  useEffect(() => {
    const cancelNavigation = () => { setDeferredNavigation(null); menuNavigation.current = null; };
    window.addEventListener('popstate', cancelNavigation);
    return () => window.removeEventListener('popstate', cancelNavigation);
  }, []);
  useEffect(() => () => { if (toastTimer.current) clearTimeout(toastTimer.current); }, []);
  useEffect(() => () => { if (restoreFocusFrame.current !== null) cancelAnimationFrame(restoreFocusFrame.current); }, []);
  useEffect(() => {
    const mobile = matchMedia('(max-width: 560px)');
    const closeOnDesktop = () => { if (!mobile.matches) setSidebarOpen(false); };
    mobile.addEventListener('change', closeOnDesktop);
    return () => mobile.removeEventListener('change', closeOnDesktop);
  }, []);

  function notify(message: string) { setToast(message); if (toastTimer.current) clearTimeout(toastTimer.current); toastTimer.current = setTimeout(() => setToast(''), 6000); }
  function setPanel(value: Panel, opener?: HTMLElement | null) {
    if (restoreFocusFrame.current !== null) cancelAnimationFrame(restoreFocusFrame.current);
    restoreFocusFrame.current = null;
    if (value && !panel) openerRef.current = opener ?? (sidebarOpen ? mobileMenuRef.current : document.activeElement instanceof HTMLElement ? document.activeElement : null);
    updatePanel(value);
    if (value) { setSidebarOpen(false); setDeferredNavigation(null); menuNavigation.current = null; }
    if (!value) restoreFocusFrame.current = requestAnimationFrame(() => {
      restoreFocusFrame.current = null;
      const opener = openerRef.current;
      if (opener?.isConnected) opener.focus();
      if (document.activeElement !== opener) mainRef.current?.focus({ preventScroll: true });
    });
  }
  function returnToWorkspace() {
    setPanel(null);
    setActiveNav('requests');
    setDeferredNavigation({ section: 'requests', focus: true });
  }
  function navigate(section: string, afterRefresh = false) {
    setActiveNav(section); setSidebarOpen(false);
    menuNavigation.current = sidebarOpen ? section : null;
    setDeferredNavigation(afterRefresh && !sidebarOpen ? { section, focus: false } : null);
    if (!afterRefresh && !sidebarOpen) scrollToSection(section);
  }
  // Complete navigation after the modal releases focus. Returning focus to its
  // trigger cancels the destination scroll in WebKit.
  function finishMenuNavigation(event: Event) {
    const section = menuNavigation.current;
    menuNavigation.current = null;
    if (panel || section) event.preventDefault();
    if (section && !panel) setDeferredNavigation({ section, focus: true });
  }
  const finishNavigation = useCallback(() => {
    if (!deferredNavigation || panel || sidebarOpen || !document.getElementById(deferredNavigation.section)) return;
    // Closing a group drawer may still have a focus-restoration frame queued.
    // Keep that frame from scrolling back to its old opener after navigation.
    if (restoreFocusFrame.current !== null) {
      cancelAnimationFrame(restoreFocusFrame.current); restoreFocusFrame.current = null;
      mainRef.current?.focus({ preventScroll: true });
    }
    scrollToSection(deferredNavigation.section, deferredNavigation.focus);
    setDeferredNavigation(null);
  }, [deferredNavigation, panel, sidebarOpen]);
  return { panel, setPanel, theme, setTheme, toast, setToast, sidebarOpen, setSidebarOpen, activeNav, navigate, returnToWorkspace, finishNavigation, finishMenuNavigation, notify, searchRef, mainRef, exportRef, mobileMenuRef };
}

export type AppShell = ReturnType<typeof useAppShell>;

function scrollToSection(section: string, focus = false) {
  const target = document.getElementById(section);
  if (focus && target) { target.tabIndex = -1; target.focus({ preventScroll: true }); }
  target?.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth', block: 'start' });
}
