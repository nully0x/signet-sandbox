# AGENTS.md

Operational guide for coding agents working in this repo.
Product spec: `docs/INITIAL.md` (v0.7). Build plan + verification gates:
`docs/CHECKPOINTS.md`. Read both before starting work.

## Project

Signet Sandbox provisions isolated, per-team Bitcoin signet test environments
(bitcoind + block signer + optional electrs/explorer/faucet/LND) behind a
JSON-RPC provisioning API and dashboard. Rust throughout: Axum API, kube-rs
orchestrator (k3s, namespace-per-environment), custom BIP325 signer.
Svelte dashboard embedded via rust-embed (post-MVP).

## Commands

Enter the devshell first (pins cargo, k3d, kubectl, just, sqlx-cli, node):

    nix develop

Local dev loop:

    just local-setup        # generates .env + deploy/compose/bitcoin.conf
    just dev-up             # bitcoind + postgres (docker compose)
    just dev-signer         # native signer; premines 101, then 30s interval
    just dev-api            # native API
    just dev-reset          # wipe compose volumes (REQUIRED after local-setup --force)
    just test / fmt / lint / check
    just verify-all         # build + workspace tests — run before calling work done

Cluster (k3s): `just cluster-up / deploy-dev / cluster-status / logs-signer`.

Gate discipline: every checkpoint in `docs/CHECKPOINTS.md` has a gate command.
Never mark `[x]` without running its gate.

## Repo layout

- `crates/signet-signer` — BIP325 block signer (keygen, block assembly, PoW grind)
- `crates/signet-bitcoind` — bitcoind JSON-RPC client + response types
- `crates/signet-rpc` — JSON-RPC 2.0 envelope + error codes
- `crates/signet-api` — Axum server, single RPC dispatch route
- `crates/signet-core` — Environment + ConnectionBundle types
- `crates/signet-db` — sqlx PgPool + migrations
- `crates/signet-nostr` — NIP-98 auth
- `deploy/compose` — local dev stack; `deploy/dev` — k3s kustomize; `deploy/docker` — images

## Conventions

- Wire protocol: JSON-RPC 2.0 over `POST /rpc`, methods `environment.*`.
  Failures are JSON-RPC error objects (codes in `signet-rpc/src/error.rs`),
  never HTTP status semantics.
- serde `snake_case` for wire types.
- Isolated-by-default: no tiers; every environment is its own namespace + chain.
  Disabled components are OMITTED from the connection bundle
  (`skip_serializing_if`), not nulled.
- Rust edition 2024, stable toolchain. Match existing style; no comments unless asked.

## Commits

Git is owner-driven: the user performs all git operations, including commit.
Agents never stage, commit, or push — at most they suggest a one-liner
describing what was worked on.

Make incremental, atomic commits that each tell one part of the story. Every
commit is authored by the repository owner — the repo-local
user.name/user.email — never by a tool
or agent identity, and commit messages carry no AI co-author trailers.
Authorship is part of the no-tool-names rule: check `git config user.name`
before the first commit of a session and fix it rather than committing under
a default. Format: `type: imperative summary under 50 chars`, then a body in
natural prose explaining the why more than the what (no bullet-point dumps).
Types: `feat` for new functionality (`feat: NIP-98 event verification`),
`fix` for bug fixes, `refactor`, `docs`, `build`, `ci`, `test`, `chore`.
Lock files, generated files, and vendored code get their own commits.

## Gotchas — hard-won, do not re-derive

1. **BIP34 coinbase height (the signet trap).** Core compares the coinbase
   scriptSig against `CScript() << nHeight` BYTEWISE. `push_int64` emits
   `OP_1..OP_16` (single byte, e.g. `0x51`) for heights 1–16; a minimal
   CScriptNum push only from 17 up. Signet has `BIP34Height=1`, so pushing
   `0101` at height 1 is rejected `bad-cb-height`. Heights 0–16 must be
   `OP_N` + `OP_0` pad (scriptSig must be 2–100 bytes: `bad-cb-length`).
   See `bip34_coinbase_scriptsig` in `crates/signet-signer/src/signet.rs`.
2. **Custom signet genesis == default signet genesis**
   (`00000008819873e9…`). `-signetchallenge` does NOT change the genesis; it
   only changes the p2p magic bytes and per-block solution validation.
3. **Core 29 `getblocktemplate` returns `version` as an INTEGER**
   (`536870912` = `0x20000000`), not a hex string. `de_hex_i32` in
   `signet-bitcoind/src/types.rs` accepts both — keep it that way.
4. **Signet RPC settings must live in the `[signet]` section** of bitcoin.conf
   (`rpcbind`, `rpcallowip`, `rpcuser`, `rpcpassword`), or Core 29 refuses to start.
5. **bitcoind healthcheck needs `-datadir=/home/bitcoin/.bitcoin`** — the
   healthcheck runs as root; bitcoin-cli otherwise looks in `/root/.bitcoin`
   and misses the credentials.
6. **Postgres 18 mounts at `/var/lib/postgresql`** (data lives in a versioned
   subdir). The old `…/data` mount fails initdb.
7. **PoW grinding is ~10x slow in debug** (signet difficulty `0x1e0377ae` ≈
   4.8M hashes/block). `Cargo.toml` sets `opt-level=3` for
   `bitcoin`/`bitcoin_hashes`/`secp256k1`/`sha2` in the dev profile — don't
   remove; it's what makes the 101-block premine finish in <60s.
8. **`just local-setup --force` invalidates the chain** (new challenge).
   Always follow with `just dev-reset`.
9. **Env loading:** justfile sets `dotenv-load := true`; clap args read env
   vars; docker-compose needs `--env-file .env` (the compose var includes it).
