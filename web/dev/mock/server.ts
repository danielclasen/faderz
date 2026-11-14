// HTTP and WebSocket front of the dev-only mock: the contract's REST
// routes under /api/v1, the WebSocket at /api/v1/ws (routed to a serving
// replica, like a service endpoint) and at /__mock/replicas/<id>/api/v1/ws
// (one replica), and dev and test controls under /__mock (README.md).

import { SESSION_ID } from '../../src/lib/net/ids.ts';
import type { IncomingMessage, Server, ServerResponse } from 'node:http';
import type { Duplex } from 'node:stream';
import { WebSocketServer, type WebSocket } from 'ws';
import type { Change, CommandResult, TallyState } from '../../src/lib/contract/types.ts';
import { Cluster, CommandError } from './cluster.ts';

const API = '/api/v1';

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let data = '';
    req.setEncoding('utf8');
    req.on('data', (chunk: string) => (data += chunk));
    req.on('end', () => resolve(data));
    req.on('error', reject);
  });
}

type Handler = (body: unknown, params: string[]) => { status: number; body?: unknown };

interface Route {
  method: string;
  pattern: RegExp;
  /** Path template as written in openapi.yaml. */
  template: string;
  handler: Handler;
}

export class MockServer {
  readonly cluster: Cluster;
  private readonly wss = new WebSocketServer({ noServer: true });
  private readonly routes: Route[];

  constructor(cluster: Cluster) {
    this.cluster = cluster;
    this.routes = this.buildRoutes();
  }

  /** Attach WebSocket handling to an HTTP server. */
  attach(server: Server): void {
    server.on('upgrade', (req: IncomingMessage, socket: Duplex, head: Buffer) => this.upgrade(req, socket, head));
  }

  private upgrade(req: IncomingMessage, socket: Duplex, head: Buffer): void {
    const url = new URL(req.url ?? '/', 'http://mock');
    let replica;
    if (url.pathname === `${API}/ws`) {
      replica = this.cluster.route();
    } else {
      const m = /^\/__mock\/replicas\/([a-z0-9-]+)\/api\/v1\/ws$/.exec(url.pathname);
      if (!m) return; // not ours, e.g. Vite's HMR socket
      const r = this.cluster.replica(m[1]!);
      replica = r && r.healthy && r.frozen === 'none' ? r : null;
    }
    const session = url.searchParams.get('session');
    if (session === null || !SESSION_ID.test(session)) {
      socket.write('HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');
      socket.destroy();
      return;
    }
    if (!replica) {
      socket.write('HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');
      socket.destroy();
      return;
    }
    const target = replica;
    this.wss.handleUpgrade(req, socket, head, (ws: WebSocket) => {
      const conn = this.cluster.connect(
        target,
        session,
        (text) => {
          if (ws.readyState === ws.OPEN) ws.send(text);
        },
        () => ws.terminate(),
      );
      ws.on('message', (data, isBinary) => {
        try {
          this.cluster.receive(conn, isBinary ? '' : String(data), isBinary);
        } catch (e) {
          console.error('[faderz mock]', e);
          ws.terminate();
        }
      });
      ws.on('close', () => this.cluster.disconnect(conn));
    });
  }

  /** Connect-style middleware; calls next() for anything it does not serve. */
  middleware = (req: IncomingMessage, res: ServerResponse, next: () => void): void => {
    const url = new URL(req.url ?? '/', 'http://mock');
    // REST of one replica, the base the client derives from a per-replica endpoint.
    const one = /^\/__mock\/replicas\/([a-z0-9-]+)(\/api\/v1\/.+)$/.exec(url.pathname);
    if (one && one[2] !== `${API}/ws`) {
      const r = this.cluster.replica(one[1]!);
      if (!r || !r.healthy || r.frozen !== 'none') {
        res.statusCode = 503;
        res.end();
        return;
      }
      void this.api(req, res, one[2]!.slice(API.length));
      return;
    }
    if (url.pathname.startsWith('/__mock/')) {
      void this.dev(req, res, url);
      return;
    }
    if (!url.pathname.startsWith(API + '/') || url.pathname === `${API}/ws`) {
      next();
      return;
    }
    void this.api(req, res, url.pathname.slice(API.length));
  };

  private async api(req: IncomingMessage, res: ServerResponse, path: string): Promise<void> {
    const method = req.method ?? 'GET';
    const route = this.routes.find((r) => r.method === method && r.pattern.test(path));
    const contract = this.cluster.contract;
    const reply = (template: string | null, status: number, body?: unknown) => {
      if (template) contract.assertResponse(template, method, status, body);
      res.statusCode = status;
      if (body === undefined) {
        res.end();
        return;
      }
      res.setHeader('Content-Type', 'application/json');
      res.end(JSON.stringify(body));
    };
    try {
      if (!route) return reply(null, 404, { code: 404, message: `no route ${method} ${path}` });
      const raw = await readBody(req);
      if (method !== 'GET') {
        if (req.headers['x-samplez-request'] !== '1') {
          return reply(route.template, 403, { code: 403, message: 'X-Samplez-Request: 1 required' });
        }
        if (raw && !(req.headers['content-type'] ?? '').startsWith('application/json')) {
          return reply(route.template, 403, { code: 403, message: 'Content-Type application/json required' });
        }
      }
      let body: unknown = undefined;
      if (raw) {
        try {
          body = JSON.parse(raw);
        } catch {
          return reply(route.template, 400, { code: 400, message: 'not JSON' });
        }
      }
      const problem = contract.checkRequest(route.template, method, body);
      if (problem) return reply(route.template, 400, { code: 400, message: `invalid request: ${problem}` });
      const params = route.pattern.exec(path)!.slice(1);
      try {
        // A request sees no block left open, and its own block commits before
        // it is answered: reads and answers carry committed revisions only.
        this.cluster.flush();
        const out = route.handler(body, params);
        this.cluster.flush();
        reply(route.template, out.status, out.body);
      } catch (e) {
        if (!(e instanceof CommandError)) throw e;
        reply(route.template, e.code, {
          code: e.code,
          message: e.message,
          ...(e.conflicts.length > 0 ? { conflicts: e.conflicts } : {}),
        });
      }
    } catch (e) {
      console.error('[faderz mock]', e);
      res.statusCode = 500;
      res.end(String(e));
    }
  }

  private buildRoutes(): Route[] {
    const c = this.cluster;
    const leader = () => {
      const r = c.replicas.find((x) => x.role === 'leader') ?? c.replicas[0]!;
      return c.ref(r);
    };
    const result = (commandId: string | undefined, result: CommandResult['result'], atIndex?: number): CommandResult => ({
      ...(commandId ? { commandId } : {}),
      result,
      revision: c.pendingRevision,
      ...(atIndex !== undefined ? { atIndex } : {}),
    });
    const duplicate = (commandId: string, revision: number): CommandResult => ({ commandId, result: 'duplicate', revision });
    const sceneOr404 = (id: string) => {
      if (!c.hasScene(id)) throw new CommandError(404, `unknown scene ${id}`);
    };
    return [
      {
        method: 'GET', pattern: /^\/state$/, template: '/state',
        handler: () => ({ status: 200, body: { revision: c.revision, replica: leader(), shape: c.shape, state: c.state, owners: c.owners } }),
      },
      {
        method: 'PATCH', pattern: /^\/state$/, template: '/state',
        handler: (b) => {
          const body = b as { commandId: string; changes: Change[] };
          return { status: 200, body: c.patch(body.commandId, body.changes) };
        },
      },
      {
        method: 'GET', pattern: /^\/replicas$/, template: '/replicas',
        handler: () => ({ status: 200, body: { replica: leader(), replicas: c.replicaStatus() } }),
      },
      {
        method: 'POST', pattern: /^\/scenes$/, template: '/scenes',
        handler: (b) => {
          const body = b as { commandId: string; name: string };
          const prior = c.appliedAt(body.commandId);
          if (prior !== undefined) {
            const scene = c.state.scenes.find((s) => s.name === body.name) ?? c.state.scenes.at(-1)!;
            return { status: 201, body: { scene, result: duplicate(body.commandId, prior) } };
          }
          const scene = c.storeScene(body.name, null);
          c.markApplied(body.commandId);
          return { status: 201, body: { scene, result: result(body.commandId, 'applied') } };
        },
      },
      {
        method: 'PUT', pattern: /^\/scenes\/([a-z][a-z0-9-]{0,31})$/, template: '/scenes/{sceneId}',
        handler: (b, [id]) => {
          sceneOr404(id!);
          const body = b as { commandId: string; name?: string };
          const prior = c.appliedAt(body.commandId);
          if (prior !== undefined) {
            return { status: 200, body: { scene: c.state.scenes.find((s) => s.id === id)!, result: duplicate(body.commandId, prior) } };
          }
          const scene = c.storeScene(body.name ?? c.sceneName(id!), id!);
          c.markApplied(body.commandId);
          return { status: 200, body: { scene, result: result(body.commandId, 'applied') } };
        },
      },
      {
        method: 'DELETE', pattern: /^\/scenes\/([a-z][a-z0-9-]{0,31})$/, template: '/scenes/{sceneId}',
        handler: (_b, [id]) => {
          sceneOr404(id!);
          c.deleteScene(id!);
          return { status: 200, body: result(undefined, 'applied') };
        },
      },
      {
        method: 'POST', pattern: /^\/scenes\/([a-z][a-z0-9-]{0,31})\/recall$/, template: '/scenes/{sceneId}/recall',
        handler: (b, [id]) => {
          sceneOr404(id!);
          const body = b as { commandId: string; crossfadeMs: number };
          const prior = c.appliedAt(body.commandId);
          if (prior !== undefined) return { status: 200, body: duplicate(body.commandId, prior) };
          const r = c.recallScene(id!);
          c.markApplied(body.commandId);
          return { status: 200, body: result(body.commandId, r.changed ? 'applied' : 'noop', r.atIndex) };
        },
      },
      {
        method: 'POST', pattern: /^\/afv\/tally$/, template: '/afv/tally',
        handler: (b) => {
          const body = b as { source: string; state: TallyState; atIndex?: number };
          const atIndex = c.applyTally({ [body.source]: body.state }, body.atIndex ?? null);
          return { status: 200, body: result(undefined, atIndex === undefined ? 'noop' : 'applied', atIndex) };
        },
      },
      {
        method: 'PUT', pattern: /^\/afv\/tally$/, template: '/afv/tally',
        handler: (b) => {
          const body = b as { sources: Record<string, TallyState> };
          const atIndex = c.replaceTally(body.sources);
          return { status: 200, body: result(undefined, atIndex === undefined ? 'noop' : 'applied', atIndex) };
        },
      },
      {
        method: 'POST', pattern: /^\/loudness\/reset$/, template: '/loudness/reset',
        handler: (b) => {
          const ids = (b as { ids: string[] }).ids;
          const known = new Set([...c.shape.buses, ...c.shape.auxes, ...c.shape.mixMinus].map((e) => e.id));
          const bad = ids.find((id) => !known.has(id));
          if (bad) throw new CommandError(404, `unknown bus ${bad}`);
          c.resetLoudness(ids);
          return { status: 204 };
        },
      },
    ];
  }

  private async dev(req: IncomingMessage, res: ServerResponse, url: URL): Promise<void> {
    const c = this.cluster;
    const send = (status: number, body: unknown) => {
      res.statusCode = status;
      res.setHeader('Content-Type', 'application/json');
      res.end(JSON.stringify(body));
    };
    await readBody(req);
    const method = req.method ?? 'GET';
    const path = url.pathname;
    // The dev server listens on every interface: a page of another origin
    // must not drive the controls. Browsers send Origin with every POST.
    const origin = req.headers.origin;
    if (method !== 'GET' && origin !== undefined && (!URL.canParse(origin) || new URL(origin).host !== req.headers.host)) {
      return send(403, { error: 'mock controls answer their own origin only' });
    }
    if (method === 'GET' && path === '/__mock/replicas') {
      return send(
        200,
        c.replicas.map((r) => ({ id: r.id, role: r.role, healthy: r.healthy, frozen: r.frozen, partitioned: r.partitioned, lagMs: r.lagMs, applied: r.applied, clients: r.conns.size })),
      );
    }
    if (method === 'GET' && path === '/__mock/commands') return send(200, c.audit);
    if (method === 'GET' && path === '/__mock/audit') return send(200, this.auditReport());
    if (method === 'GET' && path === '/__mock/stats') return send(200, this.statsReport());
    const m = /^\/__mock\/replicas\/([a-z0-9-]+)\/(kill|restart|restore|freeze|unfreeze|partition|heal|lag)$/.exec(path);
    if (method === 'POST' && m) {
      const id = m[1]!;
      const mode = url.searchParams.get('mode') === 'apply' ? 'apply' : 'drop';
      const actions: Record<string, () => boolean> = {
        kill: () => c.kill(id),
        restart: () => c.restore(id),
        restore: () => c.restore(id),
        freeze: () => c.freeze(id, mode),
        unfreeze: () => c.unfreeze(id),
        partition: () => c.partition(id),
        heal: () => c.heal(id),
        lag: () => {
          const ms = Number(url.searchParams.get('ms') ?? '0');
          return Number.isFinite(ms) && ms >= 0 && c.lag(id, ms);
        },
      };
      const ok = actions[m[2]!]!();
      return send(ok ? 200 : c.replica(id) ? 409 : 404, { ok });
    }
    if (method === 'POST' && (path === '/__mock/blocks/hold' || path === '/__mock/blocks/flush')) {
      c.holdBlock(path.endsWith('/hold'));
      return send(200, { ok: true, revision: c.revision });
    }
    if (method === 'POST' && path === '/__mock/stats/reset') {
      c.resetStats();
      return send(200, { ok: true });
    }
    if (method === 'POST' && path === '/__mock/audit/reset') {
      c.resetAudit();
      return send(200, { ok: true });
    }
    if (method === 'POST' && path === '/__mock/drop-clients') {
      for (const conn of c.connections()) conn.close();
      return send(200, { ok: true });
    }
    if (method === 'POST' && path === '/__mock/rewind') {
      const by = Number(url.searchParams.get('by') ?? '1');
      if (!Number.isInteger(by) || by < 1) return send(400, { error: 'by must be a positive integer' });
      c.rewind(by);
      return send(200, { revision: c.revision });
    }
    if (method === 'POST' && path === '/__mock/skip-revision') {
      c.skipRevision();
      return send(200, { revision: c.revision });
    }
    send(404, { error: 'unknown mock control' });
  }

  private statsReport() {
    const connections = this.cluster.connections().map((conn) => ({
      connection: conn.id,
      // The handle, never the session id: the id is a credential.
      handle: conn.owner?.handle ?? null,
      kind: conn.owner?.kind ?? null,
      mode: conn.mode,
      replica: conn.replica.id,
      ...conn.stats,
      creditsOutstanding: conn.credit,
      openedAt: new Date(conn.openedAt).toISOString(),
    }));
    return { connections, totals: { connections: connections.length, ...this.cluster.totals } };
  }

  private auditReport() {
    const entries = this.cluster.applies;
    const times = new Map<string, number>();
    for (const e of entries) times.set(e.commandId, (times.get(e.commandId) ?? 0) + 1);
    const duplicates = [...times.values()].filter((n) => n > 1).length;
    return { entries, summary: { total: entries.length, duplicates } };
  }
}
