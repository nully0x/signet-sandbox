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

## Quick start

Docker is the only prerequisite. The CLI is not published to a registry yet,
so build it from a checkout:

```bash
just install-cli
```

Start the stack, then provision, use, and destroy:

```bash
signet bootstrap                      # k3d cluster + platform stack + profile
signet up --name first-env --ttl 2h   # provision, wait for ready, print the bundle
signet get first-env --json
signet fund first-env tb1q... --amount 100000
signet down first-env                 # destroy the namespace
```

Pass `--host` at bootstrap when other machines must reach the endpoints. The
host is fixed when the stack is created, so it is the first decision to make.
`signet up` reads `./sandbox.json` when present, and `signet schema` prints
the JSON Schema for that file.

## How it works

The provisioning API (Axum) serves JSON-RPC 2.0 on `POST /api/rpc`, a health
check on `GET /healthz`, and the dashboard on every other path. It
authenticates callers with NIP-98 signed Nostr events or hashed bearer
tokens, persists environments in Postgres, and orchestrates the cluster with
kube-rs.

Each environment is a namespace. The platform generates a keypair per
environment and derives the signet challenge from it, so no two environments
mine the same chain. The signer premines 101 blocks and then follows the block
policy. Destroying an environment deletes the namespace. Stop and start scale
workloads to zero while the persistent volumes keep the chain, and a reaper
enforces the `ttl`.

## Documentation

- [`site/`](site/) — the public site. `site/docs.html` covers usage: install,
  bootstrap, the connection bundle, Lightning, lifecycle, and the API
- [`docs/DEV.md`](docs/DEV.md) — development and operations
- [`AGENTS.md`](AGENTS.md) — operating guide for coding agents

## Status

Shipped: NIP-98 and token auth, per-environment provisioning, faucet, electrs,
explorer behind Envoy Gateway, LND instances with peering, TTL reaper, CLI,
and the embedded dashboard.

Not built yet: scenario controls (reorgs, stuck fees, RBF), TTL updates,
snapshot and restore, a Helm chart, a CI action, and a hosted tier.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The short version: `just
verify-all` is the bar, docs move with the change, commits are small and
conventional.

## License

MIT
