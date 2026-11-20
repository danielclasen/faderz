// What a loudness readout says beside its integrated loudness and LRA
// about the measurement behind them, from the serving replica's counters
// message (model.schema.json LoudnessHealth): the 10 ms slices left out
// as gaps, and when the measurement started where it did not start at
// the last reset this replica applied. Integrated loudness keeps
// measuring across gaps and is never blanked for them.

import type { LoudnessHealth } from './contract/types';

/** Length of a slice the counts are in, ms. */
const SLICE_MS = 10;

export interface LoudnessNote {
  /** "N gaps", or null with none. */
  gaps: string | null;
  /** "since HH:MM" in local time, or null while the measurement started at the last reset. */
  since: string | null;
  /** The explanation of both, empty with neither. */
  tip: string;
}

const pad = (n: number) => String(n).padStart(2, '0');

/** A count in at most four characters: exact below 1000, else in thousands. */
function count(n: number): string {
  if (n < 1000) return String(n);
  return n < 10_000 ? `${(Math.floor(n / 100) / 10).toFixed(1)}k` : `${Math.floor(n / 1000)}k`;
}

/** Seconds or minutes a number of slices covers, rounded. */
function span(slices: number): string {
  const s = Math.round((slices * SLICE_MS) / 1000);
  return s < 120 ? `${s} s` : `${Math.round(s / 60)} min`;
}

export function loudnessNote(h: LoudnessHealth | undefined): LoudnessNote {
  if (!h) return { gaps: null, since: null, tip: '' };
  const n = h.gaps + h.lost;
  const tips: string[] = [];
  let gaps: string | null = null;
  if (n > 0) {
    gaps = `${count(n)} ${n === 1 ? 'gap' : 'gaps'}`;
    const lost = h.lost > 0 ? ` ${h.lost} of them on this replica only.` : '';
    tips.push(
      `${n} ${n === 1 ? 'gap' : 'gaps'} of 10 ms in this measurement, about ${span(n)}: an input down, or audio this replica could not meter, ` +
        `such as when its host was short of CPU. Integrated loudness and LRA carry on without them.${lost} ` +
        `Slices lost on this replica only can make its reading differ slightly from the other replicas'.`,
    );
  }
  let since: string | null = null;
  if (h.restarted && h.since !== null) {
    const at = new Date(h.since);
    since = `since ${pad(at.getHours())}:${pad(at.getMinutes())}`;
    tips.push(`Measuring since ${at.toLocaleString()}: this replica's measurement restarted; earlier programme loudness isn't included.`);
  }
  return { gaps, since, tip: tips.join(' ') };
}
