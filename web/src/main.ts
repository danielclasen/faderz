import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { MixerStore, mixerContext } from './lib/state/mixer.svelte';
import { configureRest } from './lib/state/actions';
import { Connection, resolveEndpoints } from './lib/net/connection';
import { initTheme } from './lib/theme';

initTheme();

const mixer = new MixerStore();
const endpoints = resolveEndpoints(new URLSearchParams(location.search).get('endpoints'), location);
const connection = new Connection(endpoints, {
  open: (endpoint) => mixer.opened(endpoint),
  message: (data) => mixer.receive(data),
  close: () => mixer.closed(),
});
mixer.attach(connection);
configureRest(endpoints, mixer);
connection.start();

mount(App, { target: document.getElementById('app')!, context: mixerContext(mixer) });

if (import.meta.env.DEV) {
  // Dev-only controls of the mock server; absent from production builds.
  void import('../dev/mock/ui/mount').then((m) => m.mountMockPanel());
}
