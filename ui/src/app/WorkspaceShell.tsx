import { ArrowSquareOut, ChartBar, Database, Desktop, GearSix, Info, List, Moon, Plus, Pulse, Rows, ShieldCheck, Sun, UploadSimple, X } from '@phosphor-icons/react';
import * as Dialog from '@radix-ui/react-dialog';
import { Brand } from '../components';
import { compact } from '../format';
import type { Dashboard } from '../types';

import type { ReactNode } from 'react';
import type { AppShell } from './useAppShell';

type Props = { state: AppShell; data: Dashboard | null; sample: boolean; includesSamples: boolean; error: string; connectionText: string; children: ReactNode; details: ReactNode };

export function WorkspaceShell({ state, data, sample, includesSamples, error, connectionText, children, details }: Props) {
  const { setPanel, theme, setTheme, toast, setToast, sidebarOpen, setSidebarOpen, activeNav, navigate, finishMenuNavigation, mobileMenuRef } = state;
  const sidebar = <>
      <Brand onNavigate={() => navigate('overview')} />
      <div className="workspace-switch"><div className="workspace-monogram"><Desktop size={18} weight="bold" /></div><div><strong>Local workspace</strong><span>{sample ? 'Sample environment' : 'On this machine'}</span></div><span className={`connection-dot ${error ? 'offline' : ''}`} title={connectionText} /></div>
      <div className="nav-section-label">WORKSPACE</div>
      <nav className="navigation"><button className={activeNav === 'overview' ? 'active' : ''} onClick={() => navigate('overview')}><ChartBar size={19} weight={activeNav === 'overview' ? 'fill' : 'bold'} />Overview</button><button className={activeNav === 'questions' ? 'active' : ''} onClick={() => navigate('questions')}><Rows size={19} weight="bold" />Question groups{data && <span className="nav-count">{compact(data.groups.length)}</span>}</button><button className={activeNav === 'requests' ? 'active' : ''} onClick={() => navigate('requests')}><Pulse size={19} weight="bold" />Requests</button></nav>
      <div className="nav-section-label second">WORKSPACE DATA</div>
      <nav className="navigation"><button onClick={() => setPanel({ type: 'import' })}><UploadSimple size={19} weight="bold" />Import records</button><button onClick={() => setPanel({ type: 'settings' })}><GearSix size={19} weight="bold" />Settings</button></nav>
      <div className="sidebar-bottom"><div className="local-note"><ShieldCheck size={21} weight="duotone" /><strong>Your history stays here.</strong><p>Local storage. No account.<br />No automatic uploads.</p></div><button className="sidebar-connect" title="Connect an application" onClick={() => setPanel({ type: 'connect' })}><Plus size={17} weight="bold" />Connect an application<ArrowSquareOut size={14} /></button><div className="sidebar-foot"><span className={`connection-dot ${error ? 'offline' : ''}`} />{connectionText}</div></div>
  </>;

  return <Dialog.Root open={sidebarOpen} onOpenChange={setSidebarOpen}><div className="app-shell">
    <a href="#main" className="skip-link">Skip to dashboard</a>
    <aside className="sidebar" aria-label="Workspace navigation">{sidebar}</aside>
    <Dialog.Portal><Dialog.Overlay className="mobile-shade" /><Dialog.Content className="sidebar is-open mobile-navigation" aria-describedby={undefined} onCloseAutoFocus={finishMenuNavigation}>
      <Dialog.Title className="sr-only">Workspace navigation</Dialog.Title>
      <Dialog.Close className="icon-button navigation-close" aria-label="Close navigation"><X size={18} /></Dialog.Close>
      {sidebar}
    </Dialog.Content></Dialog.Portal>

    <div className="main-shell"><header className="topbar"><div className="breadcrumb"><Dialog.Trigger asChild><button ref={mobileMenuRef} className="icon-button mobile-menu" aria-label="Open navigation"><List size={22} /></button></Dialog.Trigger><Brand className="mobile-brand" onNavigate={() => navigate('overview')} /><span className="breadcrumb-location"><span>Workspace</span><span className="breadcrumb-slash">/</span><strong>Overview</strong></span>{includesSamples && <span className="sample-badge">{sample ? 'Sample data' : 'Includes samples'}</span>}</div><div className="topbar-actions"><span className="local-only"><Database size={14} />Local storage</span><button className="icon-button" onClick={() => setTheme(theme === 'light' ? 'dark' : 'light')} aria-label={`Switch to ${theme === 'light' ? 'dark' : 'light'} theme`}>{theme === 'light' ? <Moon size={19} /> : <Sun size={19} />}</button><button className="icon-button" onClick={() => setPanel({ type: 'settings' })} aria-label="Open settings"><GearSix size={19} /></button></div></header>

      {children}
    </div>
    {details}
    {toast && <div className="toast" role="status"><Info size={18} /><span>{toast}</span><button className="icon-button" aria-label="Dismiss notification" onClick={() => setToast('')}><X size={15} /></button></div>}
  </div></Dialog.Root>;
}
