import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from './api';
import type { CredentialStatus } from './types';

type CredentialFeedback = { kind: 'registration' | 'removal' | 'error'; message: string };

// Credential writes and their returned token belong to this loaded workspace.
// Closing a drawer must not lose a result or permit a second concurrent write.
// Secrets stay in memory and disappear when the page unloads.
export function useCredentials(active: boolean) {
  const [credential, setCredential] = useState<CredentialStatus | null>(null);
  const [error, setError] = useState('');
  const [feedback, setFeedback] = useState<CredentialFeedback | null>(null);
  const [clientToken, setClientToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const pending = useRef(false);
  const generation = useRef(0);
  const statusRead = useRef<AbortController | null>(null);
  const reload = useCallback(() => setRevision(value => value + 1), []);

  useEffect(() => {
    if (!active || pending.current) return;
    const controller = new AbortController();
    const startedGeneration = generation.current;
    statusRead.current = controller;
    setError('');
    api<CredentialStatus>('/api/credentials', { signal: controller.signal }).then(value => {
      if (!controller.signal.aborted && startedGeneration === generation.current) {
        setCredential(value);
        if (value.configured === false) {
          setClientToken('');
          // A status reload after a failed write must keep that write's error.
          setFeedback(previous => previous?.kind === 'registration' ? null : previous);
        }
      }
    }).catch(error => {
      if (!controller.signal.aborted && startedGeneration === generation.current) setError(error instanceof Error ? error.message : 'Unable to load credentials');
    }).finally(() => { if (statusRead.current === controller) statusRead.current = null; });
    return () => { controller.abort(); if (statusRead.current === controller) statusRead.current = null; };
  }, [active, revision]);

  const mutate = useCallback(async (apiKey: string | null, persist = false) => {
    if (pending.current) return false;
    pending.current = true;
    generation.current += 1;
    statusRead.current?.abort();
    setBusy(true); setError(''); setFeedback(null);
    try {
      if (apiKey !== null) {
        const result = await api<{ client_token: string; storage: 'session' | 'system' }>('/api/credentials', { method: 'PUT', body: JSON.stringify({ api_key: apiKey, persist }) });
        setCredential({ configured: true, storage: result.storage });
        setClientToken(result.client_token);
        setFeedback({ kind: 'registration', message: result.storage === 'system' ? 'Key saved in your system credential store.' : 'Key available until Observer stops.' });
      } else {
        await api('/api/credentials', { method: 'DELETE' });
        setCredential({ configured: false, storage: 'none' });
        setClientToken('');
        setFeedback({ kind: 'removal', message: 'Registered provider key removed.' });
      }
      return true;
    } catch (error) {
      setFeedback({ kind: 'error', message: error instanceof Error ? error.message : apiKey === null ? 'Could not remove provider key.' : 'Could not register provider key.' });
      // A failed replacement retains the last successful token. Refresh status
      // if the first status read was interrupted by this attempted write.
      reload();
      return false;
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }, [reload]);

  return { credential, error, reload, busy, message: feedback?.message ?? '', clientToken, mutate };
}

export type CredentialController = ReturnType<typeof useCredentials>;
