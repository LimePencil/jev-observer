import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from './api';
import type { Label } from './types';

export type ReviewState = { pending: Label | null; error: string; savedLabel: Label | null };
type SavedHandler = (key: string, label: Label, startedRevision: number) => void;

// Writes belong to the workspace, not to a drawer that can close while saving.
export function useReviews(activeRequest: string | null, historyRevision: number, changed: () => void) {
  const [states, setStates] = useState<Map<string, Map<string, ReviewState>>>(() => new Map());
  const inFlight = useRef(new Map<string, Set<string>>());
  const current = useRef({ activeRequest, historyRevision, changed });
  current.current = { activeRequest, historyRevision, changed };
  const handler = useRef<{ id: string; saved: SavedHandler } | null>(null);
  const register = useCallback((id: string, saved: SavedHandler) => {
    const registration = { id, saved };
    handler.current = registration;
    return () => { if (handler.current === registration) handler.current = null; };
  }, []);
  useEffect(() => {
    // Keep only active feedback and unresolved writes. Completed background
    // results will be read from storage next time that request opens.
    setStates(previous => {
      const next = new Map(previous);
      for (const [id, reviews] of next) {
        if (id === activeRequest) continue;
        const pending = new Map([...reviews].filter(([, review]) => review.pending !== null));
        if (pending.size) next.set(id, pending); else next.delete(id);
      }
      return next;
    });
  }, [activeRequest]);
  const save = useCallback(async (id: string, key: string, label: Label) => {
    const pending = inFlight.current.get(id) ?? new Set<string>();
    if (pending.has(key)) return;
    pending.add(key); inFlight.current.set(id, pending);
    const startedRevision = current.current.historyRevision;
    function status(review: ReviewState) {
      setStates(previous => {
        const next = new Map(previous), reviews = new Map(next.get(id));
        if (review.pending !== null || current.current.activeRequest === id) reviews.set(key, review);
        else reviews.delete(key);
        if (reviews.size) next.set(id, reviews); else next.delete(id);
        return next;
      });
    }
    status({ pending: label, error: '', savedLabel: null });
    try {
      await api(`/api/requests/${encodeURIComponent(id)}/label`, { method: 'POST', body: JSON.stringify({ key, label }) });
      if (current.current.activeRequest === id && handler.current?.id === id) handler.current.saved(key, label, startedRevision);
      current.current.changed();
      status({ pending: null, error: '', savedLabel: label });
    } catch (error) {
      status({ pending: null, error: error instanceof Error ? error.message : 'Could not save label', savedLabel: null });
    } finally {
      pending.delete(key);
      if (!pending.size) inFlight.current.delete(id);
    }
  }, []);
  return { states, save, register };
}

export type ReviewController = ReturnType<typeof useReviews>;
