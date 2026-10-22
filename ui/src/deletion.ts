import { useCallback, useRef, useState } from 'react';
import { api } from './api';

export function useHistoryDeletion(changed: () => void, notify: (message: string) => void) {
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const pending = useRef(false);
  const callbacks = useRef({ changed, notify });
  callbacks.current = { changed, notify };
  const remove = useCallback(async (demo: boolean) => {
    if (pending.current) return false;
    pending.current = true;
    setBusy(true); setError('');
    try {
      await api('/api/data', { method: 'DELETE' });
      callbacks.current.changed();
      callbacks.current.notify(demo ? 'Sample history deleted. It will be recreated on the next demo startup.' : 'Local history deleted. New incoming requests will still be recorded.');
      return true;
    } catch (error) {
      setError(error instanceof Error ? error.message : 'Could not delete history');
      return false;
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }, []);
  return { busy, error, remove };
}

export type HistoryDeletionController = ReturnType<typeof useHistoryDeletion>;
