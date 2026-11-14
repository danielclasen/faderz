import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { MixerStore, mixerContext } from './lib/state/mixer.svelte';
import { configureRest } from './lib/state/actions';
import { resolveEndpoints } from './lib/sync/endpoints';
import { startSync } from './lib/sync/client';
import { initTheme } from './lib/theme';
import { renderFatal } from './lib/fatal';

const root = document.getElementById('app')!;

function fail(e: unknown): void {
  console.error(e);
  renderFatal(e, root);
}

/**
 * Faces the UI draws with. Text laid out in a fallback face and redrawn in
 * the web font would move, so the app mounts once they are loaded, or
 * after FONT_WAIT_MS with whatever is there.
 */
const FACES = [
  '400 12px "IBM Plex Sans Condensed"',
  '500 12px "IBM Plex Sans Condensed"',
  '600 12px "IBM Plex Sans Condensed"',
  '400 12px "JetBrains Mono"',
  '600 12px "JetBrains Mono"',
];
const FONT_WAIT_MS = 2000;

async function fontsLoaded(): Promise<void> {
  const wait = new Promise<void>((resolve) => setTimeout(resolve, FONT_WAIT_MS));
  await Promise.race([Promise.all(FACES.map((f) => document.fonts.load(f))).then(() => undefined, () => undefined), wait]);
}

try {
  initTheme();

  const mixer = new MixerStore();
  const endpoints = resolveEndpoints(new URLSearchParams(location.search).get('endpoints'), location);
  configureRest(endpoints, mixer);
  startSync(mixer, endpoints);

  void fontsLoaded()
    .then(() => mount(App, { target: root, context: mixerContext(mixer) }))
    .catch(fail);
} catch (e) {
  fail(e);
}

if (import.meta.env.DEV) {
  // Dev-only controls of the mock server; absent from production builds.
  void import('../dev/mock/ui/mount').then((m) => m.mountMockPanel());
}
