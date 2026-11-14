// Owner handle of a session (model.schema.json#/$defs/OwnerHandle): what
// other clients see in place of the session id. SHA-256 runs here in plain
// code because crypto.subtle exists only in secure contexts, and the UI
// also runs on plain-HTTP LAN origins.

const primes = (n: number): number[] => {
  const out: number[] = [];
  for (let p = 2; out.length < n; p++) if (out.every((q) => p % q !== 0)) out.push(p);
  return out;
};
const frac32 = (x: number) => ((x - Math.floor(x)) * 2 ** 32) >>> 0;
/** Round constants: the first 32 bits of the fractional parts of the cube roots of the first 64 primes. */
const K = Uint32Array.from(primes(64), (p) => frac32(Math.cbrt(p)));
/** Initial hash: the same of the square roots of the first 8 primes. */
const H0 = Uint32Array.from(primes(8), (p) => frac32(Math.sqrt(p)));

const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));

/** SHA-256 (FIPS 180-4) of up to 2^29 bytes. */
export function sha256(data: Uint8Array): Uint8Array {
  const blocks = Math.ceil((data.length + 9) / 64);
  const m = new Uint8Array(blocks * 64);
  m.set(data);
  m[data.length] = 0x80;
  const view = new DataView(m.buffer);
  view.setUint32(m.length - 4, data.length * 8);
  const h = H0.slice();
  const w = new Uint32Array(64);
  for (let off = 0; off < m.length; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = view.getUint32(off + i * 4);
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15]!;
      const b = w[i - 2]!;
      w[i] = w[i - 16]! + (rotr(a, 7) ^ rotr(a, 18) ^ (a >>> 3)) + w[i - 7]! + (rotr(b, 17) ^ rotr(b, 19) ^ (b >>> 10));
    }
    let [a, b, c, d, e, f, g, k] = h as unknown as [number, number, number, number, number, number, number, number];
    for (let i = 0; i < 64; i++) {
      const t1 = (k + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i]! + w[i]!) | 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
      k = g;
      g = f;
      f = e;
      e = (d + t1) | 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) | 0;
    }
    h[0]! += a;
    h[1]! += b;
    h[2]! += c;
    h[3]! += d;
    h[4]! += e;
    h[5]! += f;
    h[6]! += g;
    h[7]! += k;
  }
  const out = new Uint8Array(32);
  const ov = new DataView(out.buffer);
  for (let i = 0; i < 8; i++) ov.setUint32(i * 4, h[i]!);
  return out;
}

/** Short display tag of an owner handle, the same for every client showing it. */
export function handleTag(handle: string): string {
  return handle.replace(/[-_]/g, '').slice(0, 6);
}

/** base64url without padding of the first 16 bytes of SHA-256 over the session id. */
export function ownerHandle(session: string): string {
  const digest = sha256(new TextEncoder().encode(session)).subarray(0, 16);
  return btoa(String.fromCharCode(...digest)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
