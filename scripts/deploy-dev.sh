#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${NAMESPACE:-signet-platform}"

echo "==> creating platform namespace"
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -

echo "==> platform deploy complete (namespace only; api/pg/orchestrator run in-cluster when deployed)"
