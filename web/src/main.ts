import './app.css';
import plex400 from '@fontsource/ibm-plex-sans-condensed/files/ibm-plex-sans-condensed-latin-400-normal.woff2?url';
import plex500 from '@fontsource/ibm-plex-sans-condensed/files/ibm-plex-sans-condensed-latin-500-normal.woff2?url';
import plex600 from '@fontsource/ibm-plex-sans-condensed/files/ibm-plex-sans-condensed-latin-600-normal.woff2?url';
import mono400 from '@fontsource/jetbrains-mono/files/jetbrains-mono-latin-400-normal.woff2?url';
import mono600 from '@fontsource/jetbrains-mono/files/jetbrains-mono-latin-600-normal.woff2?url';
import { mount } from 'svelte';
import App from './App.svelte';
import { MixerStore, mixerContext } from './lib/state/mixer.svelte';
import { configureRest } from './lib/state/actions';
import { resolveEndpoints, restBase } from './lib/sync/endpoints';
import { AuthStore, authContext } from './lib/auth.svelte';
import { startSync } from './lib/sync/client';
import { initTheme } from './lib/theme.svelte';
import { renderFatal } from './lib/fatal';

const root = document.getElementById('app')!;

function fail(e: unknown): void {
  console.error(e);
  renderFatal(e, root);
}

/**
 * Faces the UI draws with. They are registered with the document only
 * once loaded and before the app mounts, so no text is ever laid out in a
 * fallback face and moved by a face arriving later. A face not loaded
 * within FONT_WAIT_MS is never registered: the app then mounts with the
 * fallback faces and keeps them.
 */
const FACES: ReadonlyArray<[family: string, weight: string, url: string]> = [
  ['IBM Plex Sans Condensed', '400', plex400],
  ['IBM Plex Sans Condensed', '500', plex500],
  ['IBM Plex Sans Condensed', '600', plex600],
  ['JetBrains Mono', '400', mono400],
  ['JetBrains Mono', '600', mono600],
];
const FONT_WAIT_MS = 10_000;

async function loadFonts(): Promise<void> {
  const faces = FACES.map(([family, weight, url]) => new FontFace(family, `url(${url}) format('woff2')`, { weight }));
  const wait = new Promise<void>((resolve) => setTimeout(resolve, FONT_WAIT_MS));
  await Promise.race([Promise.allSettled(faces.map((f) => f.load())), wait]);
  for (const f of faces) if (f.status === 'loaded') document.fonts.add(f);
}

try {
  initTheme();

  const mixer = new MixerStore();
  const endpoints = resolveEndpoints(new URLSearchParams(location.search).get('endpoints'), location);
  const auth = new AuthStore(restBase(endpoints[0]!));
  // A 401 means the session ended: look again, which starts the sign-in.
  configureRest(endpoints, mixer, () => void auth.recheck());
  startSync(mixer, endpoints);

  void Promise.all([auth.refresh(), loadFonts()])
    .then(() => {
      if (auth.signInIfSignedOut()) return;
      mount(App, { target: root, context: new Map<symbol, unknown>([...mixerContext(mixer), ...authContext(auth)]) });
    })
    .catch(fail);
} catch (e) {
  fail(e);
}

if (import.meta.env.DEV) {
  // Dev-only controls of the mock server; absent from production builds.
  void import('../dev/mock/ui/mount').then((m) => m.mountMockPanel());
}
