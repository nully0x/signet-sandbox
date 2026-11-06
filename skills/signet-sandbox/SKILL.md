---
name: signet-sandbox
description: Set up and connect a fresh bitcoin application or service to a personal signet sandbox — one command provisions an isolated bitcoin network (bitcoind, electrs explorer, faucet, LND lightning nodes) behind a single API. Use when the user wants to create a signet environment, get bitcoin RPC / electrum / LND REST / LND gRPC endpoints with macaroon and TLS certificate, fund addresses with signet coin, or wire a wallet, exchange backend, or lightning service to it. Covers endpoint URLs, authentication, funding, and environment lifecycle.
---

# signet-sandbox

A personal bitcoin sandbox: one command provisions an isolated signet
network — bitcoind, electrs, a faucet, and LND lightning nodes — behind
a single API. Your application gets real bitcoin semantics (real proof of
work, real chain sync) with worthless coin, isolated per environment.

Install the `signet` CLI, then:

```bash
signet bootstrap    # one-time: docker/k3d cluster + images + api + identity
signet up --name my-app --ttl 2h
```

`up` provisions the network and waits until it is ready — ~20 s warm,
up to a couple of minutes on the first run (the chain premines 101
blocks).

## Endpoints

`signet get my-app --json` returns the connection bundle. Everything your
application needs is in it:

| Service | Endpoint | Auth |
|---|---|---|
| bitcoin RPC | `rpc_url` (JSON-RPC) | basic auth, `rpc_auth` = `user:pass` |
| electrs (electrum) | `indexer_endpoint`, `tcp://localhost:<port>` | none |
| explorer | `explorer_url` (browser) | none |
| faucet | `faucet_url` — or `signet fund my-app --address <addr> --amount <sats>` | api auth |

Lightning nodes live under `lightning`, a map keyed by node name. Each
entry carries `node_id` (hex identity key), `node_uri`
(`node_id@host:port`, p2p), `rest_url`, `grpc_url`, `tls_cert` (hex of
the PEM CA), and `auth.value` (admin macaroon, hex; `auth.type` is
`macaroon`). Read one node with
`jq '.lightning["<name>"].rest_url'` and friends.

Disabled components are omitted from the bundle entirely — no nulls.
With `faucet: false` there is no `faucet_url`, and `signet fund` is
unavailable too, because it calls the faucet.

TLS: each lightning entry's `tls_cert` is the node's self-signed CA. Your
client must trust it (PEM) — pin it; do not disable verification in
production-like testing.

Authentication: every LND call needs the admin macaroon from the bundle
(REST: the `Grpc-Metadata-macaroon` header; gRPC: load the hex as a
macaroon). Never send it anywhere except your own sandbox.

## Where the endpoints resolve

Endpoints are `localhost` + port by default — everything on the machine
running the sandbox, no DNS involved.

For clients on other machines, set `SIGNET_ENV_HOST` on the API to the
sandbox host's IP or domain before starting it, and restart. The CLI
mirrors it: `signet up --host <ip-or-domain>` seeds the value when the
stack gets bootstrapped; on an already running stack the flag is a no-op
(the CLI prints a note — use `signet bootstrap --host …` instead):

```bash
SIGNET_ENV_HOST=http://192.168.100.2 signet up --name my-app
# endpoints become https://192.168.100.2:<port> — LND's certificate
# includes that IP, so clients verify normally
```

Hosted deployments do the same with a real domain (`nodes.example.com`)
plus a wildcard DNS record to the cluster ingress.

## Connecting an application

1. `signet up --name <env>` — copy the bundle values you need.
2. Point your service at the endpoints; configure the macaroon (and pin
   the TLS cert for LND).
3. Fund an address: `signet fund <env> --address <addr> --amount 1000000`
   (the faucet mints real signet coin from the premine; the chain mines a
   block every 30 s, so the balance shows confirmed after about one
   block).
4. Watch progress: `signet get <env>` (or the dashboard on port 8081).

## Multi-node and channels

A `lightning` array in the environment config provisions several nodes.
All nodes in one environment are peered with each other automatically at
boot — do not connect them manually; go straight to opening channels.
Bootstrap a channel in two steps: fund one node, then open a channel
from it to a peer's `node_id`. The funding transaction confirms in about
a block (~30 s). Then invoice on one side and pay from the other.
Channels stay manual on purpose — liquidity is usually the thing under
test.

## Lifecycle

```bash
signet status         # cluster, api, and identity health — check this first
signet down my-env     # tear the network down (stops all compute)
signet delete my-env   # remove the record and free the ports
signet ls              # list environments
```

Lightning is the one component you can add to a live environment: call
`environment.update` with the complete desired `lightning` array —
existing names included, because an omitted name counts as a removal
and is rejected. Add-only in v1: removals, ttl, and every other
component still mean `down` + `delete`, then `up` again.

Re-creating an environment under the same name always works — dead
records are cleaned up automatically. Port capacity is finite (100
lightning instances, 600 electrum): delete environments you no longer
use.

## LND REST quirks

Wiring a raw REST client instead of `lncli`? These cost real retries on
LND 0.21:

- `/v1/newaddress` rejects short aliases (`p2wkh`). Send the enum name:
  `?type=WITNESS_PUBKEY_HASH`.
- `POST /v1/channels` (open channel) is server-streaming: use
  `curl -N`. Send `node_pubkey_string` (hex) — or `node_pubkey` as
  base64 bytes — plus `local_funding_amount`.
- Peer lists key on `pub_key`, not `pubkey`. 64-bit fields arrive as
  JSON strings: `"capacity": "2000000"`.
- The REST payment endpoints do not serve on this build:
  `/v1/channels/transactions` answers 404 and `/v1/payments` answers
  501. Pay with `lncli` in the environment, or over gRPC.

## Troubleshooting

- **`rpc error -32002: unknown api token` on every command**: the API's
  database was recreated and the stored token is stale. Run
  `signet token` and write the printed value into `token = "sgn_…"` in
  `~/.signet/config.toml`.

- **Lightning endpoints missing from the bundle**: the wallet appears a
  few seconds after the environment goes ready — fetch the bundle again.
- **Everything 404s on the API port after upgrading signet**: the running
  stack predates the upgrade — `signet teardown && signet bootstrap`.
- **`signet up` fails with "port pool exhausted"**: old environments hold
  the ports — `signet delete` the ones you no longer use.
- **LND rejects calls with "wallet locked"**: wait a few seconds and
  retry; the wallet unlocks itself right after boot.
