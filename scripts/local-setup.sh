#!/usr/bin/env bash
set -euo pipefail

if [[ -f .env && "${1:-}" != "--force" ]]; then
    echo ".env already exists (use --force to regenerate; this invalidates the existing chain)"
    exit 0
fi

echo "==> generating signer key and signet challenge"
KEYGEN_JSON="$(cargo run -q -p signet-signer -- keygen)"
WIF="$(echo "$KEYGEN_JSON" | jq -r .signer_privkey_wif)"
CHALLENGE="$(echo "$KEYGEN_JSON" | jq -r .signet_challenge)"
PUBKEY="$(echo "$KEYGEN_JSON" | jq -r .signer_pubkey)"

RPC_PASSWORD="$(openssl rand -hex 16)"
PG_PASSWORD="$(openssl rand -hex 16)"

cat > .env <<EOF
SIGNET_CHALLENGE=$CHALLENGE
SIGNER_KEY_WIF=$WIF
SIGNER_PUBKEY=$PUBKEY
BITCOIN_RPC_USER=signet
BITCOIN_RPC_PASSWORD=$RPC_PASSWORD
BITCOIN_RPC_URL=http://localhost:38332
SIGNER_INTERVAL_SECS=30
SIGNER_PREMINE_BLOCKS=101
SIGNER_WALLET=faucet
POSTGRES_USER=signet
POSTGRES_DB=signet
POSTGRES_PASSWORD=$PG_PASSWORD
DATABASE_URL=postgres://signet:$PG_PASSWORD@localhost:5432/signet
EOF

cat > deploy/compose/bitcoin.conf <<CONF
signet=1
signetchallenge=$CHALLENGE
txindex=1
server=1
fallbackfee=0.00001
maxconnections=16

[signet]
rpcuser=signet
rpcpassword=$RPC_PASSWORD
rpcbind=0.0.0.0
rpcallowip=0.0.0.0/0
CONF

chmod 644 deploy/compose/bitcoin.conf

echo "==> wrote .env and deploy/compose/bitcoin.conf"
echo "    pubkey:    $PUBKEY"
echo "    challenge: $CHALLENGE"
echo "next: just dev-up && just dev-signer"
