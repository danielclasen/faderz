// WebSocket endpoints of the replica set and the REST base of each.

/**
 * Resolve configured endpoints against the page location. Only endpoints
 * on the page's own origin are taken: the list comes from the page URL,
 * and a link to the page must not send the session id, a credential, or
 * the operator's writes to another host.
 */
export function resolveEndpoints(param: string | null, location: { protocol: string; host: string }): string[] {
  const list = (param ?? '/api/v1/ws')
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return list.map((e) => {
    if (!/^[a-z][a-z0-9+.-]*:/i.test(e)) {
      if (e.startsWith('//')) throw new Error(`endpoint ${e} is not on this origin`);
      return `${scheme}//${location.host}${e.startsWith('/') ? '' : '/'}${e}`;
    }
    const url = URL.canParse(e) ? new URL(e) : null;
    if (url?.protocol !== scheme || url.host !== location.host) throw new Error(`endpoint ${e} is not on this origin`);
    return url.toString();
  });
}

/** REST base URL served by the same replica as a WebSocket endpoint. */
export function restBase(endpoint: string): string {
  return endpoint.replace(/^ws/, 'http').replace(/\/ws$/, '');
}
