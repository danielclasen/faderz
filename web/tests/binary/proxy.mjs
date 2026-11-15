// A stand-in for the authenticating proxy in front of faderz in proxy
// mode: drops identity headers a client sends, sets its own, and passes
// HTTP and WebSocket upgrades on. Usage: node proxy.mjs <port> <target port>.

import http from 'node:http';
import net from 'node:net';

const [port, target] = process.argv.slice(2).map(Number);
const IDENTITY = { 'x-forwarded-user': 'alice', 'x-forwarded-groups': 'operators' };

function headers(raw) {
  const out = {};
  for (const [k, v] of Object.entries(raw)) if (!(k.toLowerCase() in IDENTITY)) out[k] = v;
  return { ...out, ...IDENTITY };
}

const server = http.createServer((req, res) => {
  const up = http.request({ host: '127.0.0.1', port: target, method: req.method, path: req.url, headers: headers(req.headers) }, (r) => {
    res.writeHead(r.statusCode ?? 502, r.headers);
    r.pipe(res);
  });
  up.on('error', () => res.destroy());
  req.pipe(up);
});

server.on('upgrade', (req, socket, head) => {
  const up = net.connect(target, '127.0.0.1', () => {
    const lines = [`${req.method} ${req.url} HTTP/1.1`];
    for (const [k, v] of Object.entries(headers(req.headers))) for (const one of [v].flat()) lines.push(`${k}: ${one}`);
    up.write(lines.join('\r\n') + '\r\n\r\n');
    up.write(head);
    up.pipe(socket);
    socket.pipe(up);
  });
  up.on('error', () => socket.destroy());
  socket.on('error', () => up.destroy());
});

server.listen(port, '127.0.0.1');
