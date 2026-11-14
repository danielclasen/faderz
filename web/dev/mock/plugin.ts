// Vite plugin that serves the dev-only mock from `vite` (npm run dev).
// Set FADERZ_MOCK_AUTOMATION=0 to turn off the simulated tally cuts and
// console activity, as the e2e tests do.

import type { Plugin } from 'vite';
import { Cluster } from './cluster.ts';
import { MockServer } from './server.ts';

export function faderzMock(): Plugin {
  return {
    name: 'faderz-mock',
    apply: 'serve',
    configureServer(server) {
      if (!server.httpServer) return;
      const cluster = new Cluster({ automation: process.env['FADERZ_MOCK_AUTOMATION'] !== '0' });
      const mock = new MockServer(cluster);
      mock.attach(server.httpServer as import('node:http').Server);
      server.middlewares.use(mock.middleware);
      cluster.start();
      server.httpServer.on('close', () => cluster.stop());
    },
  };
}
