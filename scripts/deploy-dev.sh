#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${NAMESPACE:-signet-platform}"
PG_PASSWORD="$(openssl rand -hex 16)"

echo "==> creating platform namespace"
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -
kubectl -n "$NAMESPACE" create secret generic postgres-secret \
    --from-literal=POSTGRES_PASSWORD="$PG_PASSWORD" \
    --from-literal=DATABASE_URL="postgres://signet:$PG_PASSWORD@postgres:5432/signet" \
    --dry-run=client -o yaml | kubectl apply -f -

echo "==> applying platform stack (postgres)"
kubectl apply -k deploy/dev

echo "==> waiting for rollout"
kubectl -n "$NAMESPACE" rollout status statefulset/postgres --timeout=180s

echo "==> platform deploy complete"
