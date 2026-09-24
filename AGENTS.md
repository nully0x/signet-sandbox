# AGENTS.md

Operational guide for coding agents working in this repo.

## Project

Signet Sandbox provisions isolated, per-team Bitcoin signet test environments
(bitcoind + block signer + optional electrs/explorer/faucet/LND) behind a
JSON-RPC provisioning API and dashboard. Rust throughout: Axum API, kube-rs
orchestrator (k3s, namespace-per-environment), custom BIP325 signer.
Svelte dashboard embedded via rust-embed (post-MVP).

## Commands

Enter the devshell first (pins k3d, kubectl, cargo, just, sqlx-cli, node):

    nix develop

The nix devshell provides all repo tooling, including the docker client.
The docker daemon is the one per-platform prerequisite: install it yourself
(Docker Desktop, colima, or OrbStack on macOS; the distro package on Linux).
Then verify with:

    just doctor       # what is missing on this machine

Cluster + image operations need no sudo and are separate concerns:

    just cluster-up                  # k3d create + gateway stack (no sudo)
    just images-import IMG...        # docker build -> cluster
    just db-up / db-down             # postgres (docker)

Local dev loop:

    just local-setup        # generates .env + deploy/compose/bitcoin.conf
    just dev-up             # bitcoind + postgres (docker compose)
    just dev-signer         # native signer; premines 101, then 30s interval
    just dev-api            # native API
    just dev-reset          # wipe compose volumes (REQUIRED after local-setup --force)
    just test / fmt / lint / check
    just verify-all         # build + workspace tests — run before calling work done

Cluster (k3d, no sudo): `just cluster-up / cluster-down / images-import`.
Images build with docker and ship to the cluster via `images-import`.

Long-running or stateful commands (k3d/kubectl provisioning, docker builds,
image imports, dev servers) are run by the user, not the agent: agent tool
timeouts and aborts kill the whole process group mid-run and leave partial
state behind (half-provisioned namespaces, dead servers). The agent may run
quick, read-only or idempotent commands (`cargo test`, `kubectl get`, `psql`)
directly; anything that keeps running or mutates cluster/docker state goes to
the user — or, if agent-driven is unavoidable, launched via `setsid` with
output redirected to a log and polled.

## Repo layout

- `crates/signet-signer` — BIP325 block signer (keygen, block assembly, PoW grind)
- `crates/signet-bitcoind` — bitcoind JSON-RPC client + response types
- `crates/signet-rpc` — JSON-RPC 2.0 envelope + error codes
- `crates/signet-api` — Axum server, single RPC dispatch route
- `crates/signet-core` — Environment + ConnectionBundle types
- `crates/signet-db` — sqlx PgPool + migrations
- `crates/signet-nostr` — NIP-98 auth
- `deploy/compose` — local dev stack; `deploy/dev` — platform namespace only
  (dev runs API/Postgres/orchestrator outside the cluster; in-cluster when
  deployed); `deploy/docker` — images

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
describing what was worked on, and commit only when the user explicitly
instructs it.

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
10. **bitcoin/bitcoin:29 image must run as ROOT; never pass `-datadir` args.**
    The entrypoint usermods/chowns and gosu's down itself — a pod-level
    `securityContext` (uid 1000) makes it die with `usermod: cannot lock
    /etc/passwd`. Passing `-datadir=X` as an arg is also wrong: the
    entrypoint appends its own `-datadir=$BITCOIN_DATA` afterwards and the
    LAST one wins. Set `BITCOIN_DATA=/bitcoin` env instead; it mkdirs, chowns
    and uses that dir (image bitcoin user is uid 101).
11. **k8s bitcoind conf needs `[signet]` for rpcbind/rpcallowip too** — same
    rule as gotcha 4: network-scoped settings (`rpcbind`, `rpcallowip`,
    `rpcuser`, `rpcpassword`) go under an appended `[signet]` header in
    init-config; leaving them in base.conf's default section exits with
    "Config setting for -rpcbind only applied on signet network when in
    [signet] section."
12. **STS rolling updates can wedge silently** — pod keeps the old
    controller-revision-hash while updateRevision advances; rollout waits
    forever. After changing a StatefulSet pod template, always
    `kubectl delete pod <pod>` explicitly rather than trusting the auto-roll.
13. **Never split multi-doc YAML manually.** `split("\n---")` consumes the
    `\n` before the separator, so a block scalar that ends a document
    (`base.conf` in bitcoind.yaml) parsed WITHOUT its final newline. The
    init script's first `echo >> bitcoin.conf` then glued
    `signetchallenge=` onto `maxconnections=16`, Core fell back to the
    DEFAULT (global) signet challenge, and nodes silently joined the public
    signet chain — `blocks` climbed from peer sync, the signer logged
    `challenge mismatch` every interval, wallet balance stayed 0. Fixed by
    using serde_yaml's multi-document stream parser
    (`Deserializer::from_str` → one `Value` per doc); the
    `parsed_configmaps_keep_trailing_newlines` test guards it. If you ever
    touch `Orchestrator::apply_manifest`, re-run that test. Diagnose
    conf-glue with `kubectl exec … cat /bitcoin/bitcoin.conf` (and note
    `jq -r` adds a newline of its own — don't trust it for byte checks).
14. **`k3d image import` can silently no-op.** A "successful" import with
    no INFO output may have imported nothing; the pod sits in
    ErrImagePull. Verify inside the node before assuming:
    `docker exec k3d-signet-server-0 crictl img | grep <image>`, then
    `kubectl delete pod` to force a clean pull retry.
15. **electrs on a custom signet needs the per-env magic.** electrs
    publishes NO official docker image — build `electrs:dev` from
    `deploy/docker/Dockerfile.electrs` (pinned via `ELECTRS_VERSION` ARG,
    re-declared INSIDE the build stage or the build-arg is empty) and
    `k3d image import`. Upstream's Dockerfile has no ENTRYPOINT, so the
    pod needs `command: ["electrs", "--cookie-file=…"]` or the container
    exits 0 instantly (`Completed`). Signet p2p magic is derived from the
    challenge: `sha256d(compactsize(challenge) || challenge)[0..4]`
    (Core: kernel/chainparams.cpp); electrs assumes the DEFAULT signet
    magic and its p2p task dies with "receiving on an empty and
    disconnected channel" on mismatch — the orchestrator derives it into
    `signet-secrets/SIGNET_MAGIC` and the electrs STS reads
    `ELECTRS_MAGIC` from there. Auth is `--cookie-file` only (no inline
    `--cookie` in 0.11+); sync is p2p-only (`--jsonrpc-import` was
    removed; `ELECTRS_JSONRPC_IMPORT` is silently ignored).
16. **Gateway coexistence and klipper host ports.** Envoy Gateway is the
    edge (see docs/GATEWAY.md); Traefik coexists until the cluster
    recreate. Two LoadBalancer Services claiming host port 80 on one node
    cannot coexist: the second service's klipper `svclb` pod goes Pending
    with "didn't have free ports for the requested pod ports" — diagnose
    via `kubectl describe pod` on the svclb pod; service events do not
    show it. EG 1.2.x does not self-create its GatewayClass, and the
    Envoy data plane deploys into `envoy-gateway-system`, not the
    Gateway's namespace. EG ships its own gateway-api CRDs (supersede the
    manual standard install). Multi-node production requires MetalLB —
    klipper has no VIP or failover.
