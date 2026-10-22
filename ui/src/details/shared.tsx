import * as Dialog from '@radix-ui/react-dialog';
import type { ReactNode } from 'react';

export function DetailHeading({ eyebrow, title, description, icon }: { eyebrow: string; title: string; description: string; icon?: ReactNode }) {
  return <div className="drawer-heading"><div className="eyebrow">{icon}{eyebrow}</div><Dialog.Title>{title}</Dialog.Title><Dialog.Description>{description}</Dialog.Description></div>;
}
export function Field({ label, children }: { label: string; children: ReactNode }) { return <div className="detail-field"><dt>{label}</dt><dd>{children}</dd></div>; }
