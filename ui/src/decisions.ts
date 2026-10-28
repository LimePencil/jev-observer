import { answerValue } from './format';
import type { BoxGeometry, ChoiceValue } from './types';

export function boxGeometry(value: unknown): BoxGeometry | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const box = value as Record<string, unknown>;
  if (!['top', 'bottom', 'left', 'right'].every(key => Object.hasOwn(box, key)
    && typeof box[key] === 'number' && Number.isFinite(box[key]) && box[key] >= 0 && box[key] <= 1)) return null;
  const { top, bottom, left, right } = box as BoxGeometry;
  return top < bottom && left < right ? { top, bottom, left, right } : null;
}

export function typedChoiceLabel(value: ChoiceValue): string {
  return typeof value === 'boolean' ? `${value} (boolean)` : JSON.stringify(value);
}

// Native definitions retain their ordered choices. Canonical IDs are only a
// lookup key: never parse a key back into a potentially different typed value.
export function choiceLabeler(definition: unknown, typed = false): (key: string) => string {
  if (!typed || !definition || typeof definition !== 'object' || !('choices' in definition)
    || !Array.isArray(definition.choices)) return key => key;
  const labels = new Map<string, string>();
  for (const choice of definition.choices) {
    if (!choice || typeof choice !== 'object' || (typeof choice.value !== 'string' && typeof choice.value !== 'boolean')) continue;
    const key = `choice:v1:${JSON.stringify(choice.value)}`;
    // Preserve the first match even in retained malformed definitions.
    if (!labels.has(key)) labels.set(key, typedChoiceLabel(choice.value));
  }
  return key => labels.get(key) ?? key;
}

export function decisionValue(value: unknown, kind: string, choiceKey?: string | null): string {
  if (kind === 'choice' && choiceKey && (typeof value === 'string' || typeof value === 'boolean')) return typedChoiceLabel(value);
  if (kind === 'box') {
    const box = boxGeometry(value);
    return box ? `Top ${box.top}; bottom ${box.bottom}; left ${box.left}; right ${box.right}` : 'Invalid box geometry';
  }
  return answerValue(value, kind);
}
