// One mechanism for every key that writes the state: {@attach
// pendingWrite(mixer, path)} marks the key with data-pending while its
// write is not committed because the serving replica cannot commit
// (MixerStore.pendingKey). app.css draws the mark, which changes no size.
// A key that writes several controls passes them all; any one pending
// marks it. tests/unit/pendingKeys.test.ts checks that every key that
// writes carries it.

import type { Attachment } from 'svelte/attachments';
import type { MixerStore } from './state/mixer.svelte';

export function pendingWrite(mixer: MixerStore, path: string | readonly string[]): Attachment<HTMLElement> {
  const paths = typeof path === 'string' ? [path] : path;
  return (node) => {
    node.toggleAttribute('data-pending', paths.some((p) => mixer.pendingKey(p)));
  };
}
