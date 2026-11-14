// RFC 4122 version 4 UUIDs from crypto.getRandomValues. crypto.randomUUID
// exists only in secure contexts (HTTPS or localhost); the UI also runs on
// plain-HTTP origins such as a LAN address.

const HEX = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, '0'));

export function uuid(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const h = Array.from(b, (x) => HEX[x]!);
  return `${h.slice(0, 4).join('')}-${h.slice(4, 6).join('')}-${h.slice(6, 8).join('')}-${h.slice(8, 10).join('')}-${h.slice(10).join('')}`;
}
