#!/usr/bin/env bash
set -euo pipefail

ENV_ID="${1:?usage: env-provision.sh <env-id>}"
if ! [[ "$ENV_ID" =~ ^[a-z0-9]([a-z0-9-]{0,28}[a-z0-9])?$ ]]; then
    echo "env id must be a dns label: lowercase alnum and '-', 1-30 chars" >&2
    exit 1
fi

NAMESPACE="env-$ENV_ID"
SIGNER_IMAGE="${SIGNER_IMAGE:-signet-signer:dev}"
CLUSTER="${CLUSTER:-signet}"

if kubectl get namespace "$NAMESPACE" > /dev/null 2>&1; then
    echo "namespace $NAMESPACE already exists; destroy it first or pick another id" >&2
    exit 1
fi

echo "==> generating per-env signer key ($NAMESPACE)"
KEYGEN_JSON="$(cargo run -q -p signet-signer -- keygen)"
WIF="$(echo "$KEYGEN_JSON" | jq -r .signer_privkey_wif)"
CHALLENGE="$(echo "$KEYGEN_JSON" | jq -r .signet_challenge)"
PUBKEY="$(echo "$KEYGEN_JSON" | jq -r .signer_pubkey)"
echo "    challenge: $CHALLENGE"

echo "==> creating namespace and signet-secrets"
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -
kubectl -n "$NAMESPACE" create secret generic signet-secrets \
    --from-literal=SIGNER_KEY_WIF="$WIF" \
    --from-literal=SIGNET_CHALLENGE="$CHALLENGE" \
    --from-literal=SIGNER_PUBKEY="$PUBKEY" \
    --from-literal=BITCOIN_RPC_USER=signet \
    --from-literal=BITCOIN_RPC_PASSWORD="$(openssl rand -hex 16)" \
    --dry-run=client -o yaml | kubectl apply -f -

echo "==> ensuring signer image"
if ! docker image inspect "$SIGNER_IMAGE" > /dev/null 2>&1; then
    docker build -f deploy/docker/Dockerfile.signer -t "$SIGNER_IMAGE" .
fi
CONTEXT="$(kubectl config current-context)"
if [[ "$CONTEXT" == k3d-* ]] && command -v k3d > /dev/null; then
    k3d image import "$SIGNER_IMAGE" -c "${CONTEXT#k3d-}"
fi

echo "==> applying core stack"
kubectl apply -n "$NAMESPACE" -f crates/signet-orchestrator/templates/bitcoind.yaml
kubectl apply -n "$NAMESPACE" -f crates/signet-orchestrator/templates/signer.yaml

echo "==> waiting for rollout"
kubectl -n "$NAMESPACE" rollout status statefulset/bitcoind --timeout=300s
kubectl -n "$NAMESPACE" rollout status deployment/signet-signer --timeout=180s

echo "==> env $NAMESPACE provisioned"
echo "    challenge: $CHALLENGE"
echo "    signer:    kubectl -n $NAMESPACE logs deploy/signet-signer -f"
