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

## How it works

The provisioning API (Axum, JSON-RPC 2.0 over `POST /rpc`) authenticates
callers via NIP-98 signed Nostr events or hashed API tokens, persists
environments in Postgres, and orchestrates the cluster with kube-rs. Each
environment is a namespace containing a bitcoind StatefulSet and a block
signer deployment driven by embedded manifests; destroying an environment
deletes the namespace, cascading everything in it. Teams configure which
components they need and pin component versions (e.g. a specific bitcoind
release) per environment.

## Running it

Prerequisites: Docker, and [Nix](https://nixos.org) with flakes enabled — the
flake pins the toolchain (cargo, k3d, kubectl, just, sqlx-cli, node).

```bash
nix develop

# local loop: compose bitcoind + postgres, native signer and API
just local-setup          # generates .env + deploy/compose/bitcoin.conf
just dev-up               # bitcoind + postgres (docker compose)
just dev-signer           # native signer; premines 101, then a block every 30s
just dev-api              # provisioning API on :8080

# cluster loop: k3d + one namespace per provisioned environment
just cluster-up
```

Example — create an environment (every request needs a NIP-98 signed Nostr
event as the `Authorization` header; mint one for manual testing):

```bash
HDR=$(cargo run -q -p signet-nostr --example nip98 -- \
    http://localhost:8080/v1/rpc POST 0000...<64-hex-secret> | tail -1)

curl -s -X POST localhost:8080/v1/rpc \
    -H 'content-type: application/json' -H "$HDR" \
    -d '{"jsonrpc":"2.0","id":1,"method":"environment.create",
         "params":{"name":"my-env","versions":{"bitcoind":"29.4"}}}'
```

Failures are JSON-RPC error objects (`-32002` unauthenticated,
`-32004` not the owner, …), never HTTP status semantics. Owners can issue
long-lived API tokens (`sgn_...`) for CI pipelines where signing every
request isn't practical.

## Contributing

```bash
nix develop
just verify-all     # build + workspace tests — the bar for every change
```

Pick up an unchecked checkpoint in `docs/CHECKPOINTS.md`, implement it, and
make sure its gate command passes before opening a PR. Commits are small and
atomic with conventional prefixes (`feat:`, `fix:`, `docs:`, …). Agent
contributors have additional operational rules in `AGENTS.md`.

## Documentation

- [`docs/SIGNET_SANDBOX_SPEC.md`](docs/SIGNET_SANDBOX_SPEC.md) — product specification
- [`docs/CHECKPOINTS.md`](docs/CHECKPOINTS.md) — implementation checkpoints and verification gates
- [`AGENTS.md`](AGENTS.md) — operational guide for coding agents

## License

MIT
