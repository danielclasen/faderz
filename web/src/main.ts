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

try {
  initTheme();

  const mixer = new MixerStore();
  const endpoints = resolveEndpoints(new URLSearchParams(location.search).get('endpoints'), location);
  configureRest(endpoints, mixer);
  startSync(mixer, endpoints);

  mount(App, { target: root, context: mixerContext(mixer) });
} catch (e) {
  fail(e);
}

if (import.meta.env.DEV) {
  // Dev-only controls of the mock server; absent from production builds.
  void import('../dev/mock/ui/mount').then((m) => m.mountMockPanel());
}
