import { useCredentials } from '../credentials';
import { useHistoryDeletion } from '../deletion';
import type { Panel } from '../details/types';
import { useImports } from '../imports';
import { useReviews } from '../reviews';

// Writes belong to the workspace, never to a drawer that can close mid-request.
export function useWorkspaceControllers(panel: Panel, historyRevision: number, invalidate: () => void, historyChanged: () => void, notify: (message: string) => void) {
  const reviews = useReviews(panel?.type === 'request' ? panel.id : null, historyRevision, invalidate);
  const credentials = useCredentials(panel?.type === 'connect');
  const imports = useImports(historyChanged);
  const deletion = useHistoryDeletion(historyChanged, notify);
  return { reviews, credentials, imports, deletion };
}
