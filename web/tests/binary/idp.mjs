// A stand-in OpenID Connect provider for faderz in oidc mode: discovery,
// an ES256 key set, an authorization endpoint that signs alice in at once
// with the groups `faderz-operator`, a token endpoint that checks the code,
// redirect URI and PKCE verifier, and an end-session endpoint.
// Usage: node idp.mjs <port>. The issuer is http://127.0.0.1:<port>/realms/test.

import http from 'node:http';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';

const port = Number(process.argv[2]);
const issuer = `http://127.0.0.1:${port}/realms/test`;
const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'k1', alg: 'ES256', use: 'sig' };
const b64 = (b) => Buffer.from(b).toString('base64url');
/** Codes handed out, by code: what the token endpoint checks and puts in the ID token. */
const codes = new Map();

function idToken(nonce) {
  const now = Math.floor(Date.now() / 1000);
  const claims = { iss: issuer, aud: 'faderz', azp: 'faderz', sub: 'u-alice', preferred_username: 'alice', groups: ['faderz-operator'], iat: now, exp: now + 300, nonce };
  const input = `${b64(JSON.stringify({ alg: 'ES256', kid: 'k1', typ: 'JWT' }))}.${b64(JSON.stringify(claims))}`;
  return `${input}.${b64(sign('sha256', Buffer.from(input), { key: privateKey, dsaEncoding: 'ieee-p1363' }))}`;
}

function json(res, status, body) {
  res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(body));
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url ?? '/', issuer);
  const q = url.searchParams;
  switch (`${req.method} ${url.pathname}`) {
    case 'GET /realms/test/.well-known/openid-configuration':
      return json(res, 200, {
        issuer,
        authorization_endpoint: `${issuer}/auth`,
        token_endpoint: `${issuer}/token`,
        jwks_uri: `${issuer}/certs`,
        end_session_endpoint: `${issuer}/logout`,
      });
    case 'GET /realms/test/certs':
      return json(res, 200, { keys: [jwk] });
    case 'GET /realms/test/auth': {
      if (q.get('client_id') !== 'faderz' || q.get('code_challenge_method') !== 'S256' || !q.get('scope')?.split(' ').includes('openid')) {
        return json(res, 400, { error: 'invalid_request' });
      }
      const code = b64(randomBytes(16));
      codes.set(code, { challenge: q.get('code_challenge'), redirect: q.get('redirect_uri'), nonce: q.get('nonce') });
      const back = new URL(q.get('redirect_uri') ?? '');
      back.searchParams.set('code', code);
      back.searchParams.set('state', q.get('state') ?? '');
      res.writeHead(302, { Location: back.toString() });
      return res.end();
    }
    case 'POST /realms/test/token': {
      let body = '';
      req.on('data', (c) => (body += c));
      req.on('end', () => {
        const form = new URLSearchParams(body);
        const grant = codes.get(form.get('code') ?? '');
        codes.delete(form.get('code') ?? '');
        const challenge = b64(createHash('sha256').update(form.get('code_verifier') ?? '').digest());
        if (!grant || grant.challenge !== challenge || grant.redirect !== form.get('redirect_uri') || form.get('client_id') !== 'faderz') {
          return json(res, 400, { error: 'invalid_grant' });
        }
        json(res, 200, { access_token: 'opaque', token_type: 'Bearer', id_token: idToken(grant.nonce) });
      });
      return;
    }
    case 'GET /realms/test/logout':
      res.writeHead(302, { Location: q.get('post_logout_redirect_uri') ?? '/' });
      return res.end();
    default:
      res.writeHead(404);
      res.end();
  }
});

server.listen(port, '127.0.0.1');
