// Vite plugin that serves the dev-only mock from `vite` (npm run dev) and
// from `vite preview`; production bundles contain none of it.
// FADERZ_MOCK_AUTOMATION=0 turns off the simulated tally cuts and console
// activity; FADERZ_MOCK_CHANNELS=<n> sets the input channel count.

import type { Plugin, PreviewServer, ViteDevServer } from 'vite';
import { Cluster } from './cluster.ts';
import { MockServer } from './server.ts';

function channelCount(): number | undefined {
  const raw = process.env['FADERZ_MOCK_CHANNELS'];
  if (raw === undefined || raw === '') return undefined;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 1) throw new Error(`FADERZ_MOCK_CHANNELS must be a positive integer, got ${raw}`);
  return n;
}

type Served = Pick<ViteDevServer | PreviewServer, 'httpServer' | 'middlewares'>;

function serve(server: Served): void {
  if (!server.httpServer) return;
  const cluster = new Cluster({ automation: process.env['FADERZ_MOCK_AUTOMATION'] !== '0', channels: channelCount() });
  const mock = new MockServer(cluster);
  mock.attach(server.httpServer as import('node:http').Server);
  server.middlewares.use(mock.middleware);
  cluster.start();
  server.httpServer.on('close', () => cluster.stop());
}

export function faderzMock(): Plugin {
  return {
    name: 'faderz-mock',
    apply: 'serve',
    configureServer: serve,
    // `vite preview` of a production build, for measuring without dev-mode overhead.
    configurePreviewServer: serve,
  };
}
