# Signet Sandbox — Bruno collection

All RPC endpoints of the provisioning API, authenticated with a bearer
token minted from the terminal.

## Setup

1. Bruno → Open Collection → select this directory.
2. Pick the `local` environment. Testing from another machine? Change
   `rpc_url` and `api_base` to your dev box address, e.g.
   `http://100.86.190.9:8081` (tailscale) or the LAN IP.
3. Mint a bearer token on the dev box — one command, prints only the
   token (see also `docs/DEV.md`):

   ```bash
   SECRET=<64-hex secret key>
   HDR=$(cargo run -q -p signet-nostr --example nip98 -- \
       http://localhost:8081/v1/rpc POST "$SECRET" | tail -1)
   curl -s -X POST localhost:8081/v1/rpc \
       -H 'content-type: application/json' -H "$HDR" \
       -d '{"jsonrpc":"2.0","id":1,"method":"token.create"}' | jq -r .result.token
   ```

4. Paste the printed `sgn_...` into the `bearer` variable. Never paste
   NIP-98 headers into API tools — the signed event wraps across
   terminal lines and hand-copying corrupts the signature.
5. Run **environment.create** — its script stores `environment_id` as
   `env_id`; get/faucet/destroy target it.

## Notes

- Failures are JSON-RPC errors inside HTTP 200: check the body's `error`
  (`-32002` unauthenticated, `-32004` not owner, `-32602` invalid params).
- The pinned create needs the versioned images imported into the cluster
  (see `docs/DEV.md`); otherwise use the default create.
- The faucet body's address is a placeholder — use any address valid for
  the environment's signet chain.
