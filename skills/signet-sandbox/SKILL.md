---
name: signet-sandbox
description: Set up and connect a fresh bitcoin application or service to a personal signet sandbox — one command provisions an isolated bitcoin network (bitcoind, electrs explorer, faucet, LND lightning node) behind a single API. Use when the user wants to create a signet environment, get bitcoin RPC / electrum / LND REST / LND gRPC endpoints with macaroon and TLS certificate, fund addresses with signet coin, or wire a wallet, exchange backend, or lightning service to it. Covers endpoint URLs, authentication, funding, and environment lifecycle.
---

# signet-sandbox

A personal bitcoin sandbox: one command provisions an isolated signet
network — bitcoind, electrs, faucet, and an LND lightning node — behind a
single API. Your application gets real bitcoin semantics (real proof of
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
| LND REST | `rest_url`, `https://localhost:<port>` | header `Grpc-Metadata-macaroon: <macaroon hex>` |
| LND gRPC | `grpc_url`, `https://localhost:<port+1>` | macaroon + TLS cert (see below) |
| explorer | `explorer_url` (browser) | none |
| faucet | `faucet_url` — or `signet fund my-app --address <addr> --amount <sats>` | api auth |

TLS: LND serves its own self-signed certificate. Your client must trust
the bundle's `tls_cert` (PEM) — pin it; do not disable verification in
production-like testing.

Authentication: every LND call needs the admin macaroon from the bundle
(REST: the `Grpc-Metadata-macaroon` header; gRPC: load the hex as a
macaroon). Never send it anywhere except your own sandbox.

## Where the endpoints resolve

Endpoints are `localhost` + port by default — everything on the machine
running the sandbox, no DNS involved.

For clients on other machines, set `SIGNET_ENV_HOST` on the API to the
sandbox host's IP or domain before starting it, and restart:

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
   block every 30 s).
4. Watch progress: `signet get <env>` (or the dashboard on port 8081).

## Lifecycle

```bash
signet down my-env     # tear the network down (stops all compute)
signet delete my-env   # remove the record and free the ports
signet ls              # list environments
```

Re-creating an environment under the same name always works — dead
records are cleaned up automatically. Port capacity is finite (100
lightning instances, 600 electrum): delete environments you no longer
use.

## Troubleshooting

- **Lightning endpoints missing from the bundle**: the wallet appears a
  few seconds after the environment goes ready — fetch the bundle again.
- **Everything 404s on the API port after upgrading signet**: the running
  stack predates the upgrade — `signet teardown && signet bootstrap`.
- **`signet up` fails with "port pool exhausted"**: old environments hold
  the ports — `signet delete` the ones you no longer use.
- **LND rejects calls with "wallet locked"**: wait a few seconds and
  retry; the wallet unlocks itself right after boot.
