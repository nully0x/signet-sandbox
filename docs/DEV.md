# Development Guide

How to run and operate Signet Sandbox in dev mode. Project background lives in
the [README](../README.md); the checkpoint plan lives in
[CHECKPOINTS.md](CHECKPOINTS.md).

## Prerequisites

- Docker with a running daemon (Docker Desktop, colima, or OrbStack on macOS;
  the distro package on Linux).
- [Nix](https://nixos.org) with flakes enabled — the flake pins the toolchain
  (cargo, k3d, kubectl, just, sqlx-cli, node) and provides the docker client.

Enter the devshell and check the machine:

```bash
nix develop
just doctor       # docker daemon + cluster status
```

## The two dev loops

### Compose loop — iterate on signer/bitcoind fast

```bash
just local-setup          # generates .env + deploy/compose/bitcoin.conf
just dev-up               # bitcoind + postgres (docker compose)
just dev-signer           # native signer; premines 101, then a block every 30s
just dev-api              # provisioning API on :8081
```

`just local-setup --force` generates a new chain (new challenge). Always
follow it with `just dev-reset` — the old compose volumes no longer match.

### Cluster loop — one namespace per environment

```bash
just cluster-up                     # k3d cluster + Envoy Gateway stack
just db-up                          # postgres for the API
just images-import signet-signer:dev electrs:dev   # ship images into the cluster
just dev-api                        # API, talking to k3d
```

Build a custom electrs version (default is v0.11.1):

```bash
# pre-0.11 tags must compile their vendored rocksdb (trixie's librocksdb 9.x
# headers do not match the old electrs-rocksdb fork)
docker build -f deploy/docker/Dockerfile.electrs \
    --build-arg ELECTRS_VERSION=v0.10.9 \
    --build-arg SYSTEM_ROCKSDB=0 \
    -t electrs:0.10.9 .
just images-import electrs:0.10.9
```

## Talking to the API

The API listens on `:8081` (`POST /v1/rpc`). Every request needs a NIP-98
signed Nostr event as the `Authorization` header. Mint a fresh one per
request (the signature covers the URL and method, and stale events are
rejected):

```bash
SECRET=<64-hex secret key>
HDR=$(cargo run -q -p signet-nostr --example nip98 -- \
    http://localhost:8081/v1/rpc POST "$SECRET" | tail -1)
```

Create an environment with pinned component versions and a TTL (values are
tags only; the platform owns the repo mapping):

```bash
curl -s -X POST localhost:8081/v1/rpc \
    -H 'content-type: application/json' -H "$HDR" \
    -d '{
      "jsonrpc": "2.0", "id": 1,
      "method": "environment.create",
      "params": {
        "name": "my-env",
        "components": { "indexer": true, "explorer": true, "faucet": true },
        "versions": { "bitcoind": "29.4", "electrs": "0.10.9", "explorer": "v3.3.0" },
        "ttl_secs": 1200
      }
    }'
```

The response echoes the resolved image map and `expires_at`. Failures are
JSON-RPC error objects, never HTTP status semantics: `-32002`
unauthenticated, `-32004` not the owner, `-32602` invalid params (unknown
version key, non-positive or overflowing `ttl_secs`).

### API tokens (Bruno, curl, CI)

Signing every request does not fit API clients. Mint a bearer token once
from the terminal and never paste NIP-98 headers into API tools — the
signed event wraps across terminal lines, and hand-copying it corrupts
the signature:

```bash
SECRET=<64-hex secret key>
HDR=$(cargo run -q -p signet-nostr --example nip98 -- \
    http://localhost:8081/v1/rpc POST "$SECRET" | tail -1)
curl -s -X POST localhost:8081/v1/rpc \
    -H 'content-type: application/json' -H "$HDR" \
    -d '{"jsonrpc":"2.0","id":1,"method":"token.create"}' | jq -r .result.token
```

The last line prints only the token. Copy it wherever a bearer is needed:
a Bruno collection variable, `Authorization: Bearer sgn_...` in curl, CI
secrets. The raw token is shown once; only its hash is stored. Bearer
callers cannot mint new tokens — issuance is NIP-98-only.

NIP-98 headers expire after 300 s. The mint above always uses a fresh
header, so the window rarely matters. For a long-lived pasted header in
local dev you can lift it with `SIGNET_NIP98_MAX_AGE_SECS=0` (or add it
to `.env`). Signature, URL, and method checks stay on; use the default
window anywhere untrusted. The header must still be minted for the exact
URL you call.

Provision directly against the cluster without the API (useful for
orchestrator gates):

```bash
cargo run -p signet-orchestrator --example provision -- \
    create <env-id> indexer faucet explorer --ttl 60
cargo run -p signet-orchestrator --example provision -- ready <env-id>
cargo run -p signet-orchestrator --example provision -- destroy <env-id>
```

## Reaper

The API binary runs the TTL reaper in the background: every pass it deletes
environment namespaces whose `signet.sandbox/expires-at` annotation has
passed. Configure with `SIGNET_REAPER_INTERVAL_SECS` (default 60, `0`
disables). Watch for `reaped expired environment namespace` in the API log.

## Verification

```bash
just verify-all         # build + workspace tests — the bar for every change
just lint               # clippy -D warnings
just fmt                # rustfmt
```

Checkpoint gates and their current status: [CHECKPOINTS.md](CHECKPOINTS.md).
A checkpoint is marked `[x]` only after its gate command runs.

## Troubleshooting

Hard-won quirks (BIP34 coinbase height, Core 29 RPC sections, the
k3d/electrs/signet-magic traps and more) are catalogued in
[AGENTS.md](../AGENTS.md) — check there before debugging from scratch.
