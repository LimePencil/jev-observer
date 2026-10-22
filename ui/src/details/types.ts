import type { CredentialController } from '../credentials';
import type { HistoryDeletionController } from '../deletion';
import type { ImportController } from '../imports';
import type { ReviewController } from '../reviews';
import type { Filters, Health } from '../types';

export type Panel = { type: 'request' | 'group'; id: string } | { type: 'settings' | 'import' | 'connect'; format?: string } | null;
export type DetailProps = { health?: Health; panel: Panel; setPanel: (value: Panel) => void; filters: Filters; reviews: ReviewController; credentials: CredentialController; imports: ImportController; deletion: HistoryDeletionController; historyRevision: number; filterGroup: (id: string) => void };
