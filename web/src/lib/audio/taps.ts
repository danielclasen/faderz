// Display names of the pick-up points of a channel (model Tap).

import type { Tap } from '../contract/types';

export const TAPS: readonly Tap[] = ['input', 'preFader', 'postFader'];

export const TAP_LABEL: Record<Tap, string> = { input: 'input', preFader: 'pre-fader', postFader: 'post-fader' };

/** Short key caption. */
export const TAP_KEY: Record<Tap, string> = { input: 'IN', preFader: 'PRE', postFader: 'POST' };
