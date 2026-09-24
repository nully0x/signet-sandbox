set dotenv-load := true

cluster := "signet"

# --- cluster operations (no sudo; disposable) ---

# create or start the dev cluster + gateway stack
cluster-up:
    #!/usr/bin/env bash
    set -euo pipefail
    if k3d cluster get {{cluster}} >/dev/null 2>&1; then
        echo 'cluster exists; starting'
        k3d cluster start {{cluster}}
    else
        k3d cluster create {{cluster}} \
            --port "80:80@loadbalancer" \
            --port "443:443@loadbalancer" \
            --port "50002-50011:50002-50011@loadbalancer" \
            --volume signet-data:/var/lib/rancher/k3s/storage@server:0
        k3d kubeconfig write {{cluster}}
        kubectl config use-context k3d-{{cluster}}
        echo 'waiting for node Ready...'
        for i in $(seq 1 90); do
            ready=$(kubectl get nodes -o jsonpath='{range .items[*]}{.status.conditions[?(@.type=="Ready")].status}{end}' 2>/dev/null)
            [ "$ready" = "True" ] && break
            [ "$i" = 90 ] && { echo 'node never became ready'; exit 1; }
            sleep 2
        done
        kubectl apply --server-side -f https://github.com/envoyproxy/gateway/releases/download/v1.2.4/install.yaml
        kubectl -n envoy-gateway-system rollout status deploy/envoy-gateway --timeout=240s
        kubectl apply -f deploy/dev/gateway-eg.yaml
        kubectl wait --for=condition=accepted gatewayclass/signet-eg --timeout=120s
        kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.1/standard-install.yaml
        echo 'cluster up: envoy gateway stack accepted'
    fi

cluster-down:
    k3d cluster delete {{cluster}}

# ship locally built images into the cluster
# usage: just images-import signet-signer:dev electrs:dev signet-faucet:dev
images-import *images:
    #!/usr/bin/env bash
    set -euo pipefail
    for img in {{images}}; do
        echo "importing $img"
        rm -f /tmp/signet-image.tar
        docker image inspect "$img" >/dev/null 2>&1 || docker pull "$img"
        docker save -o /tmp/signet-image.tar "$img"
        docker cp /tmp/signet-image.tar k3d-{{cluster}}-server-0:/tmp/signet-image.tar
        docker exec k3d-{{cluster}}-server-0 sh -c \
            'ctr -n k8s.io -a /run/k3s/containerd/containerd.sock images import /tmp/signet-image.tar >/dev/null && rm /tmp/signet-image.tar'
    done
    rm -f /tmp/signet-image.tar
    echo 'images imported'

# --- image-side services (independent of the cluster) ---

# postgres for the API
db-up:
    #!/usr/bin/env bash
    set -euo pipefail
    image="docker.io/library/postgres:18"
    docker image inspect "$image" >/dev/null 2>&1 || docker pull "$image"
    docker rm -f signet-postgres 2>/dev/null || true
    docker run -d --name signet-postgres \
        -e POSTGRES_USER={{env_var_or_default("POSTGRES_USER", "signet")}} \
        -e POSTGRES_DB={{env_var_or_default("POSTGRES_DB", "signet")}} \
        -e POSTGRES_PASSWORD={{env_var_or_default("POSTGRES_PASSWORD", "signet")}} \
        -p "{{env_var_or_default("POSTGRES_HOST_PORT", "55432")}}:5432" \
        -v signet-postgres-data:/var/lib/postgresql \
        "$image"
    echo 'postgres up'

db-down:
    docker rm -f signet-postgres

# --- local dev ---

# generate signer key + challenge + credentials into .env (pass --force to regenerate)
local-setup *ARGS:
    bash scripts/local-setup.sh {{ARGS}}

# run the API natively (reads .env)
dev-api:
    cargo run -p signet-api

test:
    cargo test --workspace

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets -- -D warnings

check:
    cargo build --workspace

verify-build:
    cargo build --workspace

verify-all: verify-build test

# verify machine + cluster (read-only, no sudo)
doctor:
    #!/usr/bin/env bash
    set -uo pipefail
    fail=0
    check() { if eval "$2" >/dev/null 2>&1; then echo "  ok: $1"; else echo "  MISSING: $1 -- $3"; fail=1; fi; }
    echo "docker:"
    check 'daemon reachable' 'docker info' 'install docker for your platform; start the daemon'
    echo "cluster:"
    check 'k3d cluster running' 'k3d cluster get {{cluster}}' 'just cluster-up'
    exit $fail
