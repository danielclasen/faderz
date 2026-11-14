// Control paths of the contract and their display labels.

import type { Shape } from '../contract/types';
import { escapeToken, parsePointer, pointer } from './pointer';

export const channelPath = (id: string, ...rest: Array<string | number>) => pointer('channels', id, ...rest);
export const busPath = (id: string, ...rest: Array<string | number>) => pointer('buses', id, ...rest);
export const auxPath = (id: string, ...rest: Array<string | number>) => pointer('auxes', id, ...rest);
export const mixMinusPath = (id: string, ...rest: Array<string | number>) => pointer('mixMinus', id, ...rest);
export const vcaPath = (id: string, ...rest: Array<string | number>) => pointer('vcas', id, ...rest);
export const monitorPath = (key: string) => pointer('monitor', key);
export const afvPath = (id: string, key: string) => pointer('afv', id, key);
export const recallSafePath = (id: string) => pointer('recallSafe', id);
export { escapeToken };

/** Human-readable name of a control, e.g. "Pres 1 fader" or "PGM 5.1 mute". */
export function describePath(path: string, shape: Shape | null): string {
  const [group, id, ...rest] = parsePointer(path);
  const lists: Record<string, Array<{ id: string; name: string }> | undefined> = {
    channels: shape?.channels,
    buses: shape?.buses,
    auxes: shape?.auxes,
    mixMinus: shape?.mixMinus,
    vcas: shape?.vcas,
  };
  const element = group && id ? lists[group]?.find((e) => e.id === id) : undefined;
  if (group === 'monitor') return `Monitor ${id ?? ''}`.trim();
  const name = element?.name ?? id ?? group ?? path;
  return [name, ...rest].join(' ');
}
