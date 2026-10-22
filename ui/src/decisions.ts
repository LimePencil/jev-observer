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
export function choiceLabel(key: string, definition: unknown, typed = false): string {
  if (!typed || !definition || typeof definition !== 'object' || !('choices' in definition)
    || !Array.isArray(definition.choices)) return key;
  const choice = definition.choices.find(item => item && typeof item === 'object'
    && (typeof item.value === 'string' || typeof item.value === 'boolean')
    && `choice:v1:${JSON.stringify(item.value)}` === key);
  return choice ? typedChoiceLabel(choice.value) : key;
}

export function decisionValue(value: unknown, kind: string, choiceKey?: string | null): string {
  if (kind === 'choice' && choiceKey && (typeof value === 'string' || typeof value === 'boolean')) return typedChoiceLabel(value);
  if (kind === 'box') {
    const box = boxGeometry(value);
    return box ? `Top ${box.top}; bottom ${box.bottom}; left ${box.left}; right ${box.right}` : 'Invalid box geometry';
  }
  return answerValue(value, kind);
}
