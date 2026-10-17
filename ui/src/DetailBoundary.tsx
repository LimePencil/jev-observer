import { Component } from 'react';
import type { ReactNode } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { X } from '@phosphor-icons/react';
import { Loading } from './components';

export function DetailFallback({ close, failed = false }: { close: () => void; failed?: boolean }) {
  return <Dialog.Root open onOpenChange={open => { if (!open) close(); }}><Dialog.Portal>
    <Dialog.Overlay className="dialog-overlay" />
    <Dialog.Content className="drawer drawer-small" onCloseAutoFocus={event => event.preventDefault()}>
      <Dialog.Close className="icon-button drawer-close" aria-label="Close detail panel"><X size={20} weight="bold" /></Dialog.Close>
      <div className="drawer-heading">
        <Dialog.Title>{failed ? 'Details unavailable' : 'Opening details…'}</Dialog.Title>
        <Dialog.Description>{failed ? 'The detail panel could not be loaded. Reload this page to try again, or close this panel to continue using the dashboard.' : 'Loading the detail panel. You can close it while you wait.'}</Dialog.Description>
      </div>
      <div className="drawer-body">{failed ? <div className="button-row"><button className="button" onClick={close}>Close panel</button><button className="button primary" onClick={() => window.location.reload()}>Reload page</button></div> : <Loading label="Loading details" />}</div>
    </Dialog.Content>
  </Dialog.Portal></Dialog.Root>;
}

export class DetailBoundary extends Component<{ children: ReactNode; close: () => void }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() {
    // React.lazy caches failed imports. A full reload creates a fresh attempt;
    // reopening this drawer can only show the same failure and recovery controls.
    return this.state.failed ? <DetailFallback close={this.props.close} failed /> : this.props.children;
  }
}
