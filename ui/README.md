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

- `#/` — the shell: registry rail plus an empty pane
- `#/new` — create form
- `#/env/<name-or-id>` — detail view

The shell polls the registry every ten seconds while mounted. The detail
view polls every three seconds while an environment is `provisioning` or
`starting`, then for a 15-second grace window after `ready` — a node
exec-checked mid-boot is omitted from the bundle, and the grace poll lets
it surface as a card without a manual reload. The detail view renders
lifecycle actions (stop, start, destroy, delete), the connection bundle,
the faucet form, and lightning node cards. Lifecycle actions and status
transitions in the detail view refresh the rail immediately, so both
surfaces stay in step without separate manual refreshes.

## Layout

The app is a two-column shell: a persistent registry rail (brand, new and
refresh actions, environment rows with live status badges, rpc footer) and
a content pane. Below 760px the rail stacks on top and its rows become
horizontally scrolling chips. Detail views render as tabs: overview,
connect, chain, lightning.

- `src/App.svelte` — shell (rail + pane), routing, session, registry polling
- `src/components/EnvRail.svelte` — registry rail rows
- `src/components/EnvCreate.svelte` — create form, TTL parsing, instance list
- `src/components/EnvDetail.svelte` — tabbed detail: overview, connect, chain, lightning
- `src/components/TokenGate.svelte` — token entry
- `src/lib/rpc.ts` — JSON-RPC client and bundle types
- `src/lib/ui/` — design-system primitives (Button, Badge, Card, Field,
  Input, Tabs, Spinner, EmptyState, Copyable) and the status-to-tone map
- `src/app.css` — design tokens and base styles; components own their styles

## Check

```bash
npm --prefix ui run check    # svelte-check + tsc
npm --prefix ui run build
```

Both must pass before a change lands. The API's fallback test asserts that
unknown paths serve `index.html`.
