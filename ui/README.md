# Dashboard

The operator view for Signet Sandbox: list environments, create one, and
inspect a single environment's connection bundle.

Svelte 5 + TypeScript + Vite. The API embeds the production build with
`rust-embed` and serves it from `/`, so the dashboard and `POST /api/rpc`
share one origin and one port.

## Develop

```bash
npm --prefix ui install
npm --prefix ui run dev      # vite dev server, proxies /api to :8081
```

Start the API separately (`just dev-api`) and the proxy in `vite.config.ts`
forwards `/api` requests to it. The dashboard is same-origin in production,
so no CORS configuration exists or is needed.

Build and embed:

```bash
just ui-build                # writes ui/dist
cargo build -p signet-api    # release builds embed the bundle
```

Debug builds read `ui/dist` from disk at request time, so a UI rebuild is
visible without rebuilding the API. A missing `ui/dist` returns a 404 that
names the build command.

## Authenticate

Every RPC call carries `Authorization: Bearer sgn_...`. The token lives in
`localStorage` under `signet.token`.

- The token gate asks for one on first load.
- A `#token=` fragment wins over the stored value and is stripped from the
  address bar. `signet up --ui` uses that to hand over a fresh token.
- A `-32002` response clears the token and returns to the gate.

## Routes

Hash routing keeps the app a single static file:

- `#/` — environment list
- `#/new` — create form
- `#/env/<name-or-id>` — detail view

The detail view polls every three seconds while an environment is
`provisioning` or `starting`, and renders copyable bundle fields, lifecycle
actions (stop, start, destroy, delete), and the faucet form when the
environment has a faucet.

## Layout

- `src/App.svelte` — routing, session, refresh
- `src/components/EnvList.svelte` — registry table
- `src/components/EnvCreate.svelte` — create form, TTL parsing, instance list
- `src/components/EnvDetail.svelte` — bundle cards, lifecycle, faucet
- `src/components/TokenGate.svelte` — token entry
- `src/components/Copyable.svelte` — field with clipboard fallback
- `src/lib/rpc.ts` — JSON-RPC client and bundle types
- `src/app.css` — design tokens and base styles

## Check

```bash
npm --prefix ui run check    # svelte-check + tsc
npm --prefix ui run build
```

Both must pass before a change lands. The API's fallback test asserts that
unknown paths serve `index.html`.
