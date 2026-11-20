// What a loudness readout says beside its integrated loudness and LRA
// about the measurement behind them, from the serving replica's counters
// message (model.schema.json LoudnessHealth): the 10 ms slices left out
// as gaps, and when the measurement started where it did not start at
// the last reset this replica applied, while the state counts one. Integrated loudness keeps
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
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** A count in at most four characters: exact below 1000, else whole thousands. */
function count(n: number): string {
  return n < 1000 ? String(n) : `${Math.floor(n / 1000)}k`;
}

/** How long a number of slices covers, in words. */
function span(slices: number): string {
  const ms = slices * SLICE_MS;
  if (ms < 1000) return 'under 1 s';
  const s = Math.round(ms / 1000);
  return s < 120 ? `about ${s} s` : `about ${Math.round(s / 60)} min`;
}

/** `at` in local time: HH:MM today, else with its day and month. */
function when(at: Date, now: Date): string {
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
  return at.toDateString() === now.toDateString() ? time : `${at.getDate()} ${MONTHS[at.getMonth()]} ${time}`;
}

export function loudnessNote(h: LoudnessHealth | undefined, now = new Date()): LoudnessNote {
  if (!h) return { gaps: null, since: null, tip: '' };
  const n = h.gaps + h.lost;
  const tips: string[] = [];
  let gaps: string | null = null;
  if (n > 0) {
    const word = n === 1 ? 'gap' : 'gaps';
    gaps = `${count(n)} ${word}`;
    const lost = h.lost > 0 ? ` ${h.lost} of them on this mixer instance only.` : '';
    tips.push(
      `${n} ${word} of 10 ms in this measurement, ${span(n)}: an input was down, or this mixer instance could not measure the audio, ` +
        `for example while the server was overloaded. Integrated loudness and LRA carry on without them.${lost} ` +
        `Gaps on this instance only can make its reading differ slightly from the other instances'.`,
    );
  }
  let since: string | null = null;
  if (h.restarted && h.since !== null) {
    const at = new Date(h.since);
    since = `since ${when(at, now)}`;
    tips.push(`This mixer instance's measurement restarted at ${when(at, now)}; earlier programme loudness isn't included.`);
  }
  return { gaps, since, tip: tips.join(' ') };
}
