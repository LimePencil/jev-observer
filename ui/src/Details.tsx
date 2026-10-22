import { X } from '@phosphor-icons/react';
import * as Dialog from '@radix-ui/react-dialog';
import { ConnectDetails } from './details/ConnectDetails';
import { GroupDetails } from './details/GroupDetails';
import { ImportDetails } from './details/ImportDetails';
import { RequestDetails } from './details/RequestDetails';
import { SettingsDetails } from './details/SettingsDetails';
import type { DetailProps } from './details/types';

export type { Panel } from './details/types';

export default function Details({ health, panel, setPanel, filters, reviews, credentials, imports, deletion, historyRevision, filterGroup }: DetailProps) {
  return <Dialog.Root open={panel !== null} onOpenChange={open => { if (!open) setPanel(null); }}><Dialog.Portal><Dialog.Overlay className="dialog-overlay" /><Dialog.Content className={`drawer ${panel?.type === 'import' || panel?.type === 'connect' ? 'drawer-small' : ''}`}>
    <Dialog.Close className="icon-button drawer-close" aria-label="Close detail panel"><X size={20} weight="bold" /></Dialog.Close>
    {panel?.type === 'request' && <RequestDetails key={panel.id} id={panel.id} setPanel={setPanel} controller={reviews} historyRevision={historyRevision} />}
    {panel?.type === 'group' && <GroupDetails id={panel.id} filters={filters} setPanel={setPanel} filterGroup={filterGroup} historyRevision={historyRevision} />}
    {panel?.type === 'settings' && <SettingsDetails controller={deletion} close={() => setPanel(null)} />}
    {panel?.type === 'import' && <ImportDetails controller={imports} initialFormat={panel.format} />}
    {panel?.type === 'connect' && <ConnectDetails health={health} controller={credentials} importCaptures={() => setPanel({ type: 'import', format: 'systemone-capture' })} />}
  </Dialog.Content></Dialog.Portal></Dialog.Root>;
}
