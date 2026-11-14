// Loudness readout of this screen: absolute (LUFS) or relative to the
// target (LU), the two EBU Tech 3341 displays. A per-screen preference,
// kept in localStorage; the target itself is shared state.

export type LoudnessUnit = 'LUFS' | 'LU';
const KEY = 'faderz.loudnessUnit';

function stored(): LoudnessUnit {
  try {
    return localStorage.getItem(KEY) === 'LU' ? 'LU' : 'LUFS';
  } catch {
    return 'LUFS';
  }
}

export const loudnessUnit = $state<{ value: LoudnessUnit }>({ value: stored() });

export function setLoudnessUnit(u: LoudnessUnit): void {
  loudnessUnit.value = u;
  try {
    localStorage.setItem(KEY, u);
  } catch {
    // Storage blocked: the choice lasts for this page only.
  }
}
