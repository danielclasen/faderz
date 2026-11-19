// Dev server for the README screenshots: the dev mock with a live music
// and talk show on the example shape, its sessions named after invented
// operators, and no automation.

import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig, type Plugin } from 'vite';
import { Cluster, type Replica } from '../../dev/mock/cluster.ts';
import { MockServer } from '../../dev/mock/server.ts';
import type { PresentSession, State } from '../../src/lib/contract/types.ts';

/** Display names by channel id; the meter simulation keeps the example's source kinds. */
const NAMES: Record<string, string> = {
  ch1: 'Host', ch2: 'Co-Host', ch7: 'Lead Vox', ch8: 'Acoustic Guitar DI', ch9: 'BV 1', ch10: 'BV 2',
  ch13: 'Aud 1', ch14: 'Aud 2', ch15: 'Aud 3', ch16: 'Aud 4', ch23: 'Spare 1', ch24: 'Spare 2',
  st1: 'Band L/R', st2: 'Playback', st3: 'Stings', st4: 'SFX', sur1: 'Opener 5.1',
};
const SOURCES: Record<string, string> = { st1: 'Stage split', st2: 'Playout A', st3: 'Playout B' };
const FADERS: Record<string, number> = {
  ch1: 0, ch2: -2.5, ch3: -4, ch4: -3.5, ch5: -6, ch6: -90, ch7: -1.5, ch8: -7, ch9: -9, ch10: -9.5,
  ch11: -4, ch12: -90, ch13: -14, ch14: -16, ch15: -18, ch16: -20, ch17: -6, ch18: -8, ch19: -90,
  ch20: -90, ch21: -5, ch22: -90, ch23: -90, ch24: -90, st1: -3, st2: -10, st3: -90, st4: -12, sur1: -90,
};
/** Sessions in the order they connect; the first is the screen the screenshots are taken from. */
const OPERATORS = ['Alex', 'Sam', 'Robin', 'Kim'];

function show(state: State): void {
  for (const [id, fader] of Object.entries(FADERS)) state.channels[id]!.fader = fader;
  for (const id of ['ch6', 'ch12', 'ch23', 'ch24']) state.channels[id]!.mute = true;
  state.channels['ch11']!.solo = 'pfl';
  for (const id of ['ch7', 'ch8', 'ch9', 'ch10']) state.channels[id]!.color = 'orange';
  for (const id of ['st1', 'st2', 'st3', 'st4']) state.channels[id]!.color = 'purple';
  for (const id of ['ch11', 'ch12']) state.channels[id]!.color = 'yellow';
  const vox = state.channels['ch7']!;
  vox.eq.enabled = true;
  vox.eq.bands[0] = { type: 'lowShelf', freq: 120, gain: -3, q: 0.7 };
  vox.eq.bands[1] = { type: 'peak', freq: 450, gain: -2.5, q: 1.4 };
  vox.eq.bands[2] = { type: 'peak', freq: 3200, gain: 3, q: 1.2 };
  vox.eq.bands[3] = { type: 'highShelf', freq: 10000, gain: 2, q: 0.7 };
  vox.gate.enabled = true;
  vox.comp = { enabled: true, threshold: -20, ratio: 4, attack: 5, release: 120, makeup: 4 };
  vox.hpf.freq = 100;
  state.buses['pgm20']!.fader = -1;
  state.auxes['aux3']!.fader = -4;
  state.vcas['vca2']!.offset = -1.5;
  state.tally = { CAM1: 'program', CAM2: 'preview', CAM3: 'off', CAM4: 'off' };
  // The presenter and guest microphones dip by 6 dB while their camera is off air.
  for (const [id, cam] of [['ch1', 'CAM1'], ['ch3', 'CAM2'], ['ch4', 'CAM3'], ['ch5', 'CAM4']] as const) {
    state.afv[id] = { ...state.afv[id]!, enabled: true, source: cam, offLevel: -6, open: cam === 'CAM1' };
  }
}

class ShowCluster extends Cluster {
  private readonly operators = new Map<string, string>();
  private routed = 0;

  constructor() {
    super({ automation: false, faultyInput: null });
    for (const c of this.shape.channels) {
      c.name = NAMES[c.id] ?? c.name;
      c.source = SOURCES[c.id] ?? c.source;
    }
    for (const o of this.shape.outputs) {
      const ch = this.shape.channels.find((c) => c.id === o.source);
      if (ch) o.name = `${ch.name} out`;
    }
    this.shape.vcas = [{ id: 'vca1', name: 'HOSTS' }, { id: 'vca2', name: 'GUESTS' }];
    show(this.state);
    // The surfaces the bridge drives: no OSC tablet.
    this.state.consoles = this.state.consoles.filter((c) => c.protocol !== 'osc');
    // Engine counters hold still, so the ENGINE lamp reads the same in every shot.
    for (const r of this.replicas) {
      r.counters = new Proxy({ ...r.counters, input_late_blocks_total: 3, deadline_miss_total: 1 }, { set: () => true });
    }
    for (const s of [...this.state.scenes]) this.deleteScene(s.id);
    for (const name of ['Cold open', 'Welcome', 'Interview', 'Band live', 'Phone-in', 'Credits']) this.storeScene(name, null);
    this.state.sceneRecalled = { id: this.state.scenes[3]!.id, at: new Date().toISOString() };
  }

  /** r1, r2, r1, ...: a screen that connects its primary and then its standby has its primary on r1, as every other screen. */
  override route(): Replica | null {
    return this.replicas[this.routed++ % 2]!;
  }

  override presence(r: Replica): PresentSession[] {
    return super.presence(r).map((p) => {
      if (p.kind === 'console') return p;
      if (!this.operators.has(p.handle)) this.operators.set(p.handle, OPERATORS[this.operators.size] ?? 'Guest');
      return { ...p, user: this.operators.get(p.handle)! };
    });
  }
}

function showMock(): Plugin {
  return {
    name: 'faderz-show-mock',
    apply: 'serve',
    configureServer(server) {
      if (!server.httpServer) return;
      const cluster = new ShowCluster();
      const mock = new MockServer(cluster);
      mock.attach(server.httpServer as import('node:http').Server);
      server.middlewares.use(mock.middleware);
      cluster.start();
      server.httpServer.on('close', () => cluster.stop());
    },
  };
}

export default defineConfig({
  root: new URL('../..', import.meta.url).pathname,
  plugins: [svelte(), showMock()],
  server: { fs: { allow: ['..'] } },
});
