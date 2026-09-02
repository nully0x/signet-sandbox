#!/usr/bin/env bash
set -euo pipefail

CLUSTER="${CLUSTER:-signet}"
NAMESPACE="${NAMESPACE:-signet-platform}"
SIGNER_IMAGE="${SIGNER_IMAGE:-signet-signer:dev}"

if [[ -f .env && "${NEW_KEY:-0}" != "1" ]] && grep -q '^SIGNER_KEY_WIF=' .env; then
    echo "==> reusing signer key from .env (set NEW_KEY=1 to regenerate)"
    # shellcheck disable=SC1091
    source .env
    WIF="$SIGNER_KEY_WIF"
    CHALLENGE="$SIGNET_CHALLENGE"
    PUBKEY="${SIGNER_PUBKEY:-}"
else
    echo "==> generating signer key and signet challenge"
    KEYGEN_JSON="$(cargo run -q -p signet-signer -- keygen)"
    WIF="$(echo "$KEYGEN_JSON" | jq -r .signer_privkey_wif)"
    CHALLENGE="$(echo "$KEYGEN_JSON" | jq -r .signet_challenge)"
    PUBKEY="$(echo "$KEYGEN_JSON" | jq -r .signer_pubkey)"
fi
echo "    challenge: $CHALLENGE"

RPC_USER="signet"
RPC_PASSWORD="$(openssl rand -hex 16)"
PG_PASSWORD="$(openssl rand -hex 16)"

echo "==> creating namespace and secrets"
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -
kubectl create secret generic signet-secrets -n "$NAMESPACE" \
    --from-literal=SIGNER_KEY_WIF="$WIF" \
    --from-literal=SIGNET_CHALLENGE="$CHALLENGE" \
    --from-literal=SIGNER_PUBKEY="$PUBKEY" \
    --from-literal=BITCOIN_RPC_USER="$RPC_USER" \
    --from-literal=BITCOIN_RPC_PASSWORD="$RPC_PASSWORD" \
    --dry-run=client -o yaml | kubectl apply -f -
kubectl create secret generic postgres-secret -n "$NAMESPACE" \
    --from-literal=POSTGRES_PASSWORD="$PG_PASSWORD" \
    --from-literal=DATABASE_URL="postgres://signet:$PG_PASSWORD@postgres:5432/signet" \
    --dry-run=client -o yaml | kubectl apply -f -

echo "==> building signer image"
docker build -f deploy/docker/Dockerfile.signer -t "$SIGNER_IMAGE" .

CONTEXT="$(kubectl config current-context)"
if [[ "$CONTEXT" == k3d-* ]] && command -v k3d >/dev/null; then
    echo "==> k3d cluster detected: importing image via k3d"
    k3d image import "$SIGNER_IMAGE" -c "${CONTEXT#k3d-}"
elif command -v k3s >/dev/null; then
    echo "==> k3s host detected: importing image via k3s ctr"
    docker save "$SIGNER_IMAGE" | sudo k3s ctr images import -
else
    echo "==> no local import path detected; assuming '$SIGNER_IMAGE' is pullable from a registry"
fi

echo "==> applying manifests"
kubectl apply -k deploy/dev

echo "==> waiting for rollout"
kubectl -n "$NAMESPACE" rollout status statefulset/postgres --timeout=180s
kubectl -n "$NAMESPACE" rollout status statefulset/bitcoind --timeout=300s
kubectl -n "$NAMESPACE" rollout status deployment/signet-signer --timeout=120s

echo "==> dev deploy complete"
echo "    watch blocks: kubectl -n $NAMESPACE logs deploy/signet-signer -f"
