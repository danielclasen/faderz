// RFC 6901 JSON Pointer helpers over the live state.

export function escapeToken(token: string): string {
  return token.replace(/~/g, '~0').replace(/\//g, '~1');
}

export function parsePointer(path: string): string[] {
  if (path === '') return [];
  if (!path.startsWith('/')) throw new Error(`not a JSON pointer: ${path}`);
  return path
    .slice(1)
    .split('/')
    .map((t) => t.replace(/~1/g, '/').replace(/~0/g, '~'));
}

export function pointer(...tokens: Array<string | number>): string {
  return tokens.map((t) => '/' + escapeToken(String(t))).join('');
}

type Container = Record<string, unknown> | unknown[];

export function getAt(root: unknown, path: string): unknown {
  let node: unknown = root;
  for (const token of parsePointer(path)) {
    if (node === null || typeof node !== 'object') return undefined;
    node = (node as Record<string, unknown>)[token];
  }
  return node;
}

/**
 * Set the value at `path`, creating missing map entries on the way. Works
 * in place, so it also updates Svelte state proxies.
 */
export function setAt(root: Container, path: string, value: unknown): void {
  const tokens = parsePointer(path);
  if (tokens.length === 0) throw new Error('cannot replace the root');
  let node: unknown = root;
  for (let i = 0; i < tokens.length - 1; i++) {
    const t = tokens[i]!;
    const parent = node as Record<string, unknown>;
    if (parent[t] === null || typeof parent[t] !== 'object') parent[t] = {};
    node = parent[t];
  }
  (node as Record<string, unknown>)[tokens[tokens.length - 1]!] = structuredClone(value);
}
