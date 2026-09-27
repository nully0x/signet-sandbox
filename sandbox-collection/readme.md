# Signet Sandbox — Bruno collection

All RPC endpoints of the provisioning API, authenticated with a bearer
token minted from the terminal.

## Setup

1. Bruno → Open Collection → select this directory.
2. Pick the `local` environment. Testing from another machine? Change
   `rpc_url` and `api_base` to your dev box address, e.g.
   `http://100.86.190.9:8081` (tailscale) or the LAN IP.
3. Mint a bearer token on the dev box — one command, prints only the
   token:

   ```bash
   just get-token
   ```

   It signs `token.create` with the dev NIP-98 key from `.env`.
   No just? `cargo run -p signet-cli -- token` does the same. The
   manual curl flow lives in `docs/DEV.md`.

4. Paste the printed `sgn_...` into the `bearer` variable. Never paste
   NIP-98 headers into API tools — the signed event wraps across
   terminal lines and hand-copying corrupts the signature.
5. Run **environment.create** — its script stores `environment_id` as
   `env_id`; get/faucet/destroy target it.

## Notes

- Every `id` parameter accepts the environment UUID **or** the
  environment's name — names resolve server-side, scoped to the
  caller's npub.
- All environment methods are owner-only: another npub gets `-32004`,
  including `environment.get` (bundles carry live credentials).
- Lifecycle: **environment.stop** suspends compute (status `stopped`,
  storage persists); **environment.start** resumes it (status flips
  back to `ready` via `environment.get` once bitcoind passes
  readiness). Stop requires `ready`; start requires `stopped`.
- **environment.list** returns the caller's environments: id, name,
  status, created/expires timestamps.
- Failures are JSON-RPC errors inside HTTP 200: check the body's `error`
  (`-32002` unauthenticated, `-32004` not owner, `-32602` invalid
  params, `-32030` wrong lifecycle state).
- The pinned create needs the versioned images imported into the cluster
  (see `docs/DEV.md`); otherwise use the default create.
- The faucet body's address is a placeholder — use any address valid for
  the environment's signet chain.
