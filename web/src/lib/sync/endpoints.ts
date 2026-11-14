// WebSocket endpoints of the replica set and the REST base of each.

/** Resolve configured endpoints against the page location. */
export function resolveEndpoints(param: string | null, location: { protocol: string; host: string }): string[] {
  const list = (param ?? '/api/v1/ws')
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return list.map((e) => (/^wss?:\/\//.test(e) ? e : `${scheme}//${location.host}${e.startsWith('/') ? '' : '/'}${e}`));
}

/** REST base URL served by the same replica as a WebSocket endpoint. */
export function restBase(endpoint: string): string {
  return endpoint.replace(/^ws/, 'http').replace(/\/ws$/, '');
}
