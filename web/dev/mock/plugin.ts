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

/** `host` is the configured bind host (`--host`), true for every interface. */
function serve(server: Served, host: string | boolean | undefined): void {
  if (!server.httpServer) return;
  const cluster = new Cluster({ automation: process.env['FADERZ_MOCK_AUTOMATION'] !== '0', channels: channelCount() });
  const mock = new MockServer(cluster, typeof host === 'string' ? host : undefined);
  mock.attach(server.httpServer as import('node:http').Server);
  server.middlewares.use(mock.middleware);
  cluster.start();
  server.httpServer.on('close', () => cluster.stop());
}

export function faderzMock(): Plugin {
  return {
    name: 'faderz-mock',
    apply: 'serve',
    configureServer: (server) => serve(server, server.config.server.host),
    // `vite preview` of a production build, for measuring without dev-mode overhead.
    configurePreviewServer: (server) => serve(server, server.config.preview.host),
  };
}
