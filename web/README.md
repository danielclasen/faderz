# faderz web UI

Svelte 5 + Vite + TypeScript client of the faderz API. The contract it
speaks is in `../api`: `openapi.yaml` for REST and `ws/*.schema.json`
for the WebSocket at `/api/v1/ws`.

## Requirements

Node 22 and npm.

```sh
npm ci
npx playwright install chromium   # once, for the e2e tests
```

## Development

```sh
npm run dev
```

serves the UI with a mock faderz server on <http://localhost:5173>.
The mock (`dev/mock/`) is dev-only: Vite loads it only under
`npm run dev`, and `npm run build` contains none of it. It simulates
three replicas sharing one state, meters, loudness, AFV tally cuts every
8 s and console activity, and validates every message it sends and
receives against the contract. `FADERZ_MOCK_AUTOMATION=0` turns off the
tally and console simulation. The striped MOCK button in the status bar
kills, hangs and restores replicas.

HTTPS for the dev server: set `FADERZ_DEV_TLS_CERT` and `FADERZ_DEV_TLS_KEY`
to PEM files, for example a self-signed pair; the UI then uses `wss://`.
Plain HTTP works too, also on LAN addresses that are not secure
contexts.

The UI connects to `/api/v1/ws` on its own origin. A list of replica
endpoints can be given instead:
`?endpoints=ws://a/api/v1/ws,ws://b/api/v1/ws`.

## Tests

```sh
npm run check      # svelte-check; type errors and warnings fail
npm test           # Vitest: units and contract tests
npm run test:e2e   # Playwright against the mock on port 5181
npm run build      # production bundle in dist/
```

The contract tests compile the schemas, resolve every `$ref` of
`openapi.yaml` and drive the mock over HTTP and WebSocket, checking
every message and response with an independent validator.

## Keyboard

Faders and pan controls are sliders: arrows step 1 dB (Shift 0.1 dB),
Page Up/Down 5 dB, Home +10 dB, End off, `0` to 0 dB. Escape closes the
channel processing panel. Matrix grids move with the arrow keys.

With a pointer: drag a fader, hold Shift (or Ctrl) for fine travel,
double-click for 0 dB.
