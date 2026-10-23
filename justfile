set dotenv-load := true

cluster := "signet"


# this starts the shell
shell:
  nix develop

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
            --api-port 34611 \
            --port "80:80@loadbalancer" \
            --port "443:443@loadbalancer" \
            --port "20000-20599:20000-20599@loadbalancer" \
            --port "21000-21199:21000-21199@loadbalancer" \
            --volume signet-data:/var/lib/rancher/k3s/storage@server:0
        k3d kubeconfig write {{cluster}}
        k3d kubeconfig merge {{cluster}} --kubeconfig-merge-default -u
        kubectl config use-context k3d-{{cluster}}
        just images-import signet-signer:dev signet-faucet:dev
        echo 'waiting for node Ready...'
        for i in $(seq 1 90); do
            ready=$(kubectl get nodes -o jsonpath='{range .items[*]}{.status.conditions[?(@.type=="Ready")].status}{end}' 2>/dev/null)
            [ "$ready" = "True" ] && break
            [ "$i" = 90 ] && { echo 'node never became ready'; exit 1; }
            sleep 2
        done
        kubectl apply --server-side --force-conflicts -f https://github.com/envoyproxy/gateway/releases/download/v1.2.4/install.yaml
        kubectl -n envoy-gateway-system rollout status deploy/envoy-gateway --timeout=240s
        kubectl apply -f deploy/dev/gateway-eg.yaml
        kubectl wait --for=condition=accepted gatewayclass/signet-eg --timeout=120s
        # gateway-api CRDs come from the Envoy Gateway manifest; the
        # standard install is redundant here (repo gotcha 16).
        echo 'cluster up: envoy gateway stack accepted'
    fi

cluster-down:
    k3d cluster delete {{cluster}}

# ship locally built images into the cluster
# usage: just images-import signet-signer:dev electrs:dev signet-faucet:dev
# images that are not built locally are skipped, not fatal
images-import *images:
    #!/usr/bin/env bash
    set -euo pipefail
    for img in {{images}}; do
        if ! docker image inspect "$img" >/dev/null 2>&1; then
            echo "skipping $img — not built locally; pods using it will stay Pending"
            continue
        fi
        echo "importing $img"
        rm -f /tmp/signet-image.tar
        docker save -o /tmp/signet-image.tar "$img"
        docker cp /tmp/signet-image.tar k3d-{{cluster}}-server-0:/tmp/signet-image.tar
        docker exec k3d-{{cluster}}-server-0 sh -c \
            'ctr -n k8s.io -a /run/k3s/containerd/containerd.sock images import /tmp/signet-image.tar >/dev/null && rm /tmp/signet-image.tar'
    done
    rm -f /tmp/signet-image.tar
    echo 'images imported'

# full lifecycle smoke test against a scratch api on :8082
# exercises create, readiness (electrs + faucet + lnd wallet), destroy,
# delete, and same-name reuse. rebuild the api first: cargo build -p signet-api
# usage: just e2e [name] [api]
e2e name="smoke-e2e" api="http://localhost:8082":
    #!/usr/bin/env bash
    set -euo pipefail
    cfg=$(mktemp)
    cat > "$cfg" <<'JSON'
    {
      "ttl": "60m",
      "components": {
        "indexer": true,
        "faucet": true,
        "explorer": false,
        "lightning": [{ "name": "ln-smoke", "implementation": "lnd" }]
      }
    }
    JSON
    SIGNET_API={{api}} target/debug/signet up --config "$cfg" --name "{{name}}" --timeout-secs 420
    SIGNET_API={{api}} target/debug/signet down "{{name}}"
    SIGNET_API={{api}} target/debug/signet delete "{{name}}"
    SIGNET_API={{api}} target/debug/signet up --config "$cfg" --name "{{name}}" --timeout-secs 420
    bundle=$(SIGNET_API={{api}} target/debug/signet get "{{name}}" --json)
    rest_url=$(jq -r '.lightning["ln-smoke"].rest_url' <<<"$bundle")
    grpc_url=$(jq -r '.lightning["ln-smoke"].grpc_url' <<<"$bundle")
    macaroon=$(jq -r '.lightning["ln-smoke"].auth.value' <<<"$bundle")
    echo "rest url: $rest_url"
    echo "grpc url: $grpc_url"
    # the wallet files land a moment before the RPC accepts calls; retry
    rest_ok=1
    for attempt in 1 2 3 4 5 6 7 8 9 10; do
        if curl -sk --max-time 10 "$rest_url/v1/getinfo" \
            -H "Grpc-Metadata-macaroon: $macaroon" | jq -e '.identity_pubkey' >/dev/null; then
            rest_ok=0
            echo "lnd REST reachable: $rest_url"
            break
        fi
        [ "$attempt" = 10 ] && continue
        sleep 3
    done
    [ "$rest_ok" = 0 ] || { echo "lnd REST not reachable: $rest_url"; exit 1; }
    grpc_host=${grpc_url#https://}
    cert=$(echo | timeout 10 openssl s_client -connect "$grpc_host" -servername "${grpc_host%%:*}" 2>/dev/null \
        | openssl x509 -noout -subject 2>/dev/null)
    if [ -n "$cert" ]; then
        echo "lnd gRPC reachable: $grpc_url ($cert)"
    else
        echo "lnd gRPC not reachable: $grpc_url"
        exit 1
    fi
    SIGNET_API={{api}} target/debug/signet down "{{name}}"
    SIGNET_API={{api}} target/debug/signet delete "{{name}}"
    echo "e2e ok: {{name}}"

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

# build the dashboard into ui/dist (the api embeds it; a stub is
# generated at compile time when dist is absent)
ui-build:
    npm --prefix ui run build

# run the API natively (reads .env)
dev-api:
    cargo run -p signet-api

# bitcoind + postgres via docker compose (reads .env); waits for healthchecks
dev-up:
    docker compose -f deploy/compose/docker-compose.yml --env-file .env up -d --wait

# stop the compose stack (volumes kept)
dev-down:
    docker compose -f deploy/compose/docker-compose.yml --env-file .env down

# wipe compose volumes (chain + db) — required after `local-setup --force`
dev-reset:
    docker compose -f deploy/compose/docker-compose.yml --env-file .env down -v

# compose stack status
dev-ps:
    docker compose -f deploy/compose/docker-compose.yml --env-file .env ps

# run the dev signer natively (premines, then a block every interval; reads .env)
dev-signer *ARGS:
    cargo run -p signet-signer -- {{ARGS}}

# mint a bearer API token from the dev NIP-98 key (prints the raw token)
get-token:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${SIGNET_DEV_NSEC:-}" ]; then
        echo 'SIGNET_DEV_NSEC missing in .env — add a stable dev key once:'
        echo '  echo "SIGNET_DEV_NSEC=$(openssl rand -hex 32)" >> .env'
        exit 1
    fi
    cargo run -q -p signet-cli -- token --nsec "$SIGNET_DEV_NSEC"

# build and install the signet cli into ~/.local/bin (on PATH after a rehash)
install-cli:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release -p signet-cli
    mkdir -p "$HOME/.local/bin"
    install -m 755 target/release/signet "$HOME/.local/bin/signet"
    if [[ ":$PATH:" == *":$HOME/.local/bin:"* ]]; then
        signet --version
    else
        echo 'installed, but ~/.local/bin is not on your PATH — add it:'
        echo '  export PATH="$HOME/.local/bin:$PATH"'
    fi

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
