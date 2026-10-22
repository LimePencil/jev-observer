import { boxGeometry } from '../decisions';
import { percent } from '../format';

export function BoxValue({ value, found, present, compact = false }: {
  value: unknown; found?: boolean | null; present?: number | null; compact?: boolean;
}) {
  const box = boxGeometry(value);
  const presence = present == null ? null : typeof present === 'number' && Number.isFinite(present) && present >= 0 && present <= 1 ? percent(present) : 'Invalid';
  return <div className={`box-value${compact ? ' box-value-compact' : ''}`}>
    {found === false ? <><strong>No box localized (reported)</strong>{!compact && <p className="caption">The provider returned fallback coordinates. Inspect the raw answer for those values; this does not establish that the target is absent.</p>}</>
      : !box ? <span className="error-text">Invalid box geometry</span>
        : <>
          {!compact && <svg className="box-diagram" viewBox="0 0 100 100" role="img" aria-label={`Normalized box: top ${box.top}, bottom ${box.bottom}, left ${box.left}, right ${box.right}`}>
            <rect className="box-plane" x="0" y="0" width="100" height="100" />
            <rect className="box-outline" x={box.left * 100} y={box.top * 100} width={(box.right - box.left) * 100} height={(box.bottom - box.top) * 100} />
          </svg>}
          <dl className="box-coordinates">{(['top', 'bottom', 'left', 'right'] as const).map(edge => <div key={edge}><dt>{edge}</dt><dd>{String(box[edge])}</dd></div>)}</dl>
          {!compact && <p className="caption">Normalized coordinates from the top-left corner. The outline shows a unit coordinate plane, not the image or its aspect ratio.</p>}
        </>}
    {presence !== null && <p className="caption">Reported presence {presence} · provider-defined evidence, not measured accuracy</p>}
  </div>;
}
