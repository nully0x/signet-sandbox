# Implementation Checkpoints

Aligned to **INITIAL.md v0.7**, reordered for an MVP-first build. The MVP is one
thin vertical slice that proves the core promise (§1):
**auth → create isolated env → connection bundle → fund faucet → see it in the
explorer → destroy.**

MVP decisions (locked): **k3s + kube-rs** substrate (namespace-per-environment);
components **core + faucet + explorer(+indexer)**; interface **API + CLI first**,
minimal dashboard right after; **TTL + reaper in MVP**; indexer/explorer =
**electrs + btc-rpc-explorer**.

Statuses: `[ ]` pending · `[~]` in progress · `[x]` verified (with date)

Every checkpoint has a verifiable gate command. No checkpoint is marked `[x]`
without running its gate. Re-run the whole suite with `just verify-all`.

The path is strictly linear from M3 on: take the first unchecked item
top-to-bottom; each milestone assumes only the ones before it.

## M0 — Scaffold

- [x] CP-0.1 Cargo workspace builds — `cargo build --workspace` — 2026-09-01
- [x] CP-0.2 Workspace tests pass — `cargo test --workspace` — 2026-09-01
- [x] CP-0.3 Clippy clean — `cargo clippy --workspace --all-targets -- -D warnings` — 2026-09-01
- [x] CP-0.4 Nix devShell enters cleanly — `nix develop -c cargo --version` — 2026-09-01 (cargo 1.98, k3d 5.9, kubectl 1.37, just 1.58, sqlx-cli 0.9, node 24)
- [x] CP-0.5 justfile recipes list — `just --list` — 2026-09-01

## M1 — Core signet stack: local dev harness

The compose + native-cargo loop that iterates on signer/bitcoind behaviour
fastest. (See AGENTS.md gotchas for the BIP34/Core-29/Postgres-18 quirks.)

- [x] CP-1.1 Local setup generates .env with key/challenge — `just local-setup` — 2026-09-01
- [x] CP-1.2 Compose stack healthy — `just dev-up && just dev-ps` (bitcoind + postgres healthy) — 2026-09-01
- [x] CP-1.3 bitcoind running custom signet — `bitcoin-cli -signet getblockchaininfo` shows signet chain with our challenge — 2026-09-01
- [x] CP-1.4 Signer premines 101 blocks — height reaches 101 within ~60s of `just dev-signer` — 2026-09-01
- [x] CP-1.5 Signer produces blocks at ~30s interval (`block_policy=interval_30s`) — height increases ≥2 blocks/min over a 90s window — 2026-09-01 (3 blocks / 95s)

## M2 — v0.7 type cleanup

Bring `signet-core` / `signet-api` / `signet-rpc` in line with v0.7 before
building the control plane on them.

- [x] CP-2.1 `Environment`/`ConnectionBundle` drop the pre-v0.7 `Tier` enum; bundle component fields (`indexer_url`/`explorer_url`/`faucet_url`/`lightning`) are optional with `skip_serializing_if` — `cargo test -p signet-core` — 2026-09-01
- [x] CP-2.2 API dispatch uses `environment.destroy` (not `delete`); dead `TIER_UNAVAILABLE` error code removed — `! grep -rE 'environment\.delete|\bTier\b|TIER_UNAVAILABLE' crates/` — 2026-09-01
- [x] CP-2.3 Disabled components omitted from serialized bundle; enabled ones present — `cargo test -p signet-core` (tests `disabled_components_are_omitted`, `enabled_components_are_included`) — 2026-09-01

---

# MVP

## M3 — Auth + persistence

Pure Rust, no cluster needed; lands the token model and DB schema the
orchestrator builds on.

- [x] CP-3.1 NIP-98 event verification — `cargo test -p signet-nostr` — 2026-09-02
- [x] CP-3.2 API-token issuance + verification — `cargo test -p signet-nostr` — 2026-09-02
- [x] CP-3.3 Migrations create `Environment` + `ApiToken` (`npub_owner`, nullable `workspace_id`/`ttl`/`expires_at`) — `sqlx migrate run` then schema check — 2026-09-02
- [ ] CP-3.4 Unauthenticated request rejected — missing/invalid NIP-98 returns `-32002`

## M4 — In-cluster core stack (substrate de-risk)

The k3s/kube-rs long pole. Port the single-tenant `deploy/dev` into a
**per-environment template**.

- [ ] CP-4.1 k3d cluster up — `just cluster-up && kubectl get nodes`
- [ ] CP-4.2 Per-env core template (namespace + generated `signet-secrets` + bitcoind STS + signer Deployment) applies cleanly for one env
- [ ] CP-4.3 In-cluster signer produces blocks — height increases ≥2 blocks/min over a 90s window
- [ ] CP-4.4 Per-env signer key → distinct `signet_challenge` — two envs get different challenges

## M5 — Orchestrator + create/get/destroy

Wires M3 (auth/db) + M4 (template) into the first real API methods.

- [ ] CP-5.1 `kube-rs` creates an isolated `env-<id>` namespace — `kubectl get ns env-<id>`
- [ ] CP-5.2 `environment.create` returns a real connection bundle (platform-generated challenge) — live RPC call
- [ ] CP-5.3 Bundle omits disabled components — create with `indexer:false`, assert no `indexer_url` field
- [ ] CP-5.4 `environment.get` round-trip — live RPC call
- [ ] CP-5.5 `environment.destroy` cascades the namespace — `kubectl get ns env-<id>` gone
- [ ] CP-5.6 Ownership enforced — a non-owner npub calling a mutating method is rejected

## M6 — Faucet

- [ ] CP-6.1 Faucet minter service deployed per-env (spends from the matured premine)
- [ ] CP-6.2 `environment.faucet` funds an address — tx visible in bitcoind mempool

## M7 — Indexer + explorer

- [ ] CP-7.1 electrs synced to signet tip — electrs height == bitcoind height
- [ ] CP-7.2 btc-rpc-explorer serves — HTTP 200 and shows the latest block
- [ ] CP-7.3 Ingress routes resolve — `/rpc`, `/electrs`, `/explorer` reachable via the k3d LB port
- [ ] CP-7.4 `explorer_url` present and live in the bundle after `environment.create`

## M8 — TTL + reaper

- [ ] CP-8.1 `environment.create` with `ttl` stamps an `expires-at` annotation on the namespace
- [ ] CP-8.2 Reaper (kube-rs watch/loop) deletes a namespace past `expires-at` — TTL enforcement test

## M9 — CLI

- [ ] CP-9.1 `signet up --config ./sandbox.json --ttl 20m` provisions and prints the connection bundle
- [ ] CP-9.2 `signet fund` / `signet down` work against the API

## M10 — MVP E2E

- [ ] CP-10.1 `scripts/e2e.sh`: create → fund → confirm → explorer shows tx → destroy
- [ ] CP-10.2 Ephemeral env with `ttl` is auto-reaped after expiry

---

# Post-MVP

## P1 — Minimal dashboard  [§7]

- [ ] P1.1 Svelte app builds — `npm --prefix ui run build` exits 0
- [ ] P1.2 Static bundle embedded via `rust-embed`, served by Axum fallback — `GET /` returns dashboard HTML (200)
- [ ] P1.3 Create-environment form submits `environment.create` and renders the returned bundle
- [ ] P1.4 Environment detail view renders bundle fields as copyable

## P2 — `environment.update` / `environment.reset`  [§8, §11]

- [ ] P2.1 `environment.update` extends/clears `ttl` — live RPC call
- [ ] P2.2 `environment.update` adds a component into the existing namespace — live RPC call
- [ ] P2.3 `environment.update` rejects removing a depended-on component (e.g. indexer while LN on)
- [ ] P2.4 `environment.reset` returns chain to funded baseline — live RPC call

## P3 — Lightning (LND) + scenario controls  [§10, §8]

- [ ] P3.1 LND deployed in env namespace, connected to env bitcoind, synced to tip
- [ ] P3.2 `environment.create` with `lightning:{enabled,implementation:"lnd"}` returns `lightning.rest_url` + macaroon in the bundle
- [ ] P3.3 Indexer auto-provisioned when Lightning requested (`components.indexer` forced true)
- [ ] P3.4 `environment.mine {blocks:N}` advances height by N (`block_policy=on_demand`)
- [ ] P3.5 `environment.scenario` `reorg` / `stuck_fee` / `rbf` produce the intended state
- [ ] P3.6 LN smoke test — open a channel and route a payment within one environment
- [ ] P3.7 Dashboard exposes mine/scenario actions scoped to the team's own environment

## P4 — OSS self-host packaging  [§9, §10, §11]

- [ ] P4.1 GitHub Action wraps provisioning (create → use → down) in a CI run
- [ ] P4.2 Componentized `docker-compose.yml` (profiles per component) brings up a full self-host stack
- [ ] P4.3 Helm chart installs the componentized stack on k3s
- [ ] P4.4 DigitalOcean 1-Click Packer image boots the compose stack via cloud-init/systemd

## Deferred — Snapshot/restore  [§8]

Not built in v1 — designed only (`environment.snapshot` / `environment.restore`
method shapes in §8, `Snapshot` entity in §12). Pick up when a concrete team asks
for fixture-state capture/restore. `environment.reset` (P2.4) covers the common
case until then.
