# Signet Sandbox

Isolated, per-team Bitcoin signet test environments, provisioned through a
JSON-RPC API. Every environment gets its own chain — bitcoind in signet mode
plus a custom BIP325 block signer, with electrs, an explorer, a faucet and
optionally LND on the roadmap — running in its own Kubernetes namespace.
Teams pick what they need, get a connection bundle back, and tear it down
when they're done. The same stack publishes as an OSS self-host template.

## Why

Bitcoin companies have no easy way to stand up their own Bitcoin test
environment. Regtest is single-node and doesn't exercise real P2P/mempool
behavior. Public testnet is shared, unreliable, and faucet-starved. Standing
up a private signet correctly — node, signer, indexer, explorer, faucet,
wired together and kept running — is enough operational work that teams often
skip it and integration-test against mainnet with real funds instead.

Signet Sandbox makes that a one-call provision: configuration in, connection
bundle out, `ttl`-based teardown when you're done.

## Quick start (local)

All you need is Docker and the `signet` CLI (`just install-cli` from a
checkout).

```bash
signet local init              # k3d cluster + images + postgres + provisioning api
signet up --name first-env     # provisions an environment and prints the bundle
signet token                   # bearer token for the api (Bruno, curl, CI)
signet ls                      # list your environments
signet down first-env          # tear it down
```

`signet up` reads `./sandbox.json` when present and points at the platform
API with `--api` + `--token` for hosted use. See `docs/DEV.md` for the full
development guide.

## How it works

The provisioning API (Axum, JSON-RPC 2.0 over `POST /v1/rpc`) authenticates
callers via NIP-98 signed Nostr events or hashed API tokens, persists
environments in Postgres, and orchestrates the cluster with kube-rs. Each
environment is a namespace containing a bitcoind StatefulSet and a block
signer deployment driven by embedded manifests; destroying an environment
deletes the namespace, cascading everything in it. Teams configure which
components they need and pin component versions (e.g. a specific bitcoind
release) per environment. Environments created with a `ttl` carry an
`expires-at` annotation that a background reaper enforces.

Failures are JSON-RPC error objects (`-32002` unauthenticated, `-32004` not
the owner, …), never HTTP status semantics. Owners can issue long-lived API
tokens (`sgn_...`) for CI pipelines where signing every request isn't
practical.

## Status

The MVP path is checkpoint-driven: auth, in-cluster provisioning, faucet,
indexer + explorer behind Envoy Gateway, TTL reaper.

## Documentation

- [`docs/DEV.md`](docs/DEV.md) — running and operating in dev mode
- [`AGENTS.md`](AGENTS.md) — operational guide for coding agents

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The short version: `just
verify-all` is the bar, docs move with the change, commits are small and
conventional.

## License

MIT
