import { useCallback, useRef, useState } from 'react';
import { api } from './api';

type ImportResult = { imported: number; duplicates: number };

// Only operation status belongs to the workspace. File contents and pasted
// text remain local to the drawer and are not retained in workspace state.
export function useImports(changed: () => void) {
  const [busy, setBusy] = useState(false);
  const [format, setFormat] = useState<string | null>(null);
  const [result, setResult] = useState<ImportResult | null>(null);
  const [error, setError] = useState('');
  const pending = useRef(false);
  const currentChanged = useRef(changed);
  currentChanged.current = changed;
  const clear = useCallback(() => {
    if (pending.current) return;
    setResult(null); setError('');
  }, []);
  const submit = useCallback(async (text: string, format: string) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true); setFormat(format); setResult(null); setError('');
    try {
      const result = await api<ImportResult>('/api/import', { method: 'POST', body: JSON.stringify({ text, format }) });
      setResult(result);
      currentChanged.current();
    } catch (error) {
      setError(error instanceof Error ? error.message : 'Import failed');
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }, []);
  return { busy, format, result, error, clear, submit };
}

export type ImportController = ReturnType<typeof useImports>;
