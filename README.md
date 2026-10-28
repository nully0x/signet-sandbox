# Signet Sandbox

Isolated, per-team Bitcoin signet environments, provisioned through a
JSON-RPC API. Every environment gets its own chain: bitcoind in signet mode,
a custom BIP325 block signer driven by a per-environment key, and any of
electrs, a mempool explorer, a faucet, and LND, all in their own Kubernetes
namespace. One call returns a connection bundle. A `ttl` deletes the
environment when its time is up. Self-hosted, MIT licensed.

## Why

Bitcoin teams have no easy way to stand up their own Bitcoin test
environment. Regtest is single-node and does not exercise real P2P or
mempool behavior. Public testnet is shared, unreliable, and faucet-starved.
Standing up a private signet correctly — node, signer, indexer, explorer,
faucet, wired together and kept running — is enough operational work that
teams skip it and integration-test against mainnet with real funds instead.

Signet Sandbox makes that a one-call provision: configuration in, connection
bundle out, `ttl`-based teardown when you are done.

## Quick start (local)

Docker is the only prerequisite. Install the CLI from a checkout:

```bash
just install-cli
```

Then provision, use, and destroy:

```bash
signet bootstrap                      # k3d cluster + platform stack + profile
signet up --name first-env            # provision, wait for ready, print the bundle
signet fund first-env tb1q... --amount 100000
signet ls                             # list your environments
signet down first-env                 # destroy the namespace
```

`signet up` reads `./sandbox.json` when present. `signet schema` prints the
JSON Schema for that file, so a script or an agent can author a config
without reading these docs. `signet up --ui` opens the dashboard once the
environment is ready. `signet status` reports the local platform state, and
`signet teardown` removes the cluster.

Point the CLI at a shared platform with `--api` and `--token`. See
[`docs/DEV.md`](docs/DEV.md) for the full development guide.

## Dashboard

The Svelte dashboard is embedded in the API binary and served from `/`. It
lists your environments, creates new ones, and renders one environment's
connection bundle with copy buttons, lifecycle actions, and a faucet form.
A bearer token in `localStorage` authenticates it; the token gate collects it
once, and `signet up --ui` hands it over in a `#token=` fragment.

## How it works

The provisioning API (Axum) serves JSON-RPC 2.0 on `POST /api/rpc`, a health
check on `GET /healthz`, and the dashboard on every other path. It
authenticates callers with NIP-98 signed Nostr events or hashed bearer
tokens, persists environments in Postgres, and orchestrates the cluster with
kube-rs.

Each environment is a namespace containing its own components. The platform
generates a keypair per environment and derives the signet challenge from
it, so no two environments mine the same chain. The signer premines 101
blocks and then follows the block policy (one block every 30 seconds by
default). Destroying an environment deletes the namespace, which cascades to
everything inside it. Stop and start scale the workloads to zero while the
persistent volumes keep the chain.

Component versions are pinned per environment from a catalog, so a team can
hold bitcoind at a known release. Constraints between components are
validated at create time. Environments created with a `ttl` carry an
`signet.sandbox/expires-at` annotation that a background reaper enforces.

Failures are JSON-RPC error objects (`-32002` unauthenticated, `-32004` not
the owner, `-32602` invalid params), never HTTP status semantics. Owners can
issue long-lived API tokens (`sgn_...`) for CI pipelines where signing every
request is not practical. Token issuance itself requires NIP-98.

## What runs in an environment

Disabled components are omitted from the connection bundle, not set to null.

- **bitcoind** — signet node, pinned version, RPC and ZMQ exposed in the bundle.
- **Block signer** — BIP325 signer holding the environment key.
- **electrs** — Electrum endpoint on an allocated port, `indexer_endpoint` in the bundle.
- **Explorer** — mempool stack, one host per environment behind Envoy Gateway.
- **Faucet** — minter service that spends the matured premine.
- **LND** — one or more instances, each with node id, peering URI, gRPC and
  REST URLs, the TLS certificate as hex, and the admin macaroon.

## Status

Shipped: NIP-98 and token auth, per-environment provisioning, faucet,
electrs, explorer behind Envoy Gateway, LND instances with peering, TTL
reaper, CLI, and the embedded dashboard.

Not built yet: scenario controls (reorgs, stuck fees, RBF), TTL updates,
snapshot and restore, a Helm chart, a CI action, and a hosted tier.

## Documentation

- [`docs/DEV.md`](docs/DEV.md) — running and operating in dev mode
- [`AGENTS.md`](AGENTS.md) — operational guide for coding agents

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The short version: `just
verify-all` is the bar, docs move with the change, commits are small and
conventional.

## License

MIT
