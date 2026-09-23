set dotenv-load := true

cluster := "signet"
# container runtime: docker if present, else rootful podman (see host-setup).
# an explicit DOCKER_HOST always wins.
use_docker := `command -v docker >/dev/null 2>&1 && echo yes || echo no`
runtime := if use_docker == "yes" { "docker" } else { "podman" }
export DOCKER_HOST := if env_var_or_default("DOCKER_HOST", "") != "" {
    env_var("DOCKER_HOST")
} else if use_docker == "yes" {
    ""
} else {
    "unix:///run/podman/podman.sock"
}

# --- one-time machine bootstrap (sudo once, never needed again) ---

# one-time machine bootstrap (docker machines: nothing to do).
# podman machines expose the ROOTFUL podman socket — k3s kubelet needs host
# privileges (/dev/kmsg, CAP_SYSLOG) that rootless containers cannot get.
host-setup:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{runtime}}" = "docker" ]; then
        if docker info >/dev/null 2>&1; then
            echo 'docker runtime detected: nothing to configure'
            exit 0
        fi
        if sudo docker info >/dev/null 2>&1; then
            sudo usermod -aG docker "$USER" || true
            echo 'docker works as root; added user to docker group — RE-LOGIN required'
            exit 0
        fi
        echo 'docker is not installed or not running:'
        echo '  sudo dnf install -y docker-ce docker-ce-cli containerd.io'
        echo '  sudo systemctl enable --now docker'
        exit 1
    fi
    if ! systemctl is-active --quiet podman.socket; then
        sudo systemctl enable --now podman.socket
    fi
    conf=/etc/systemd/system/podman.socket.d/override.conf
    if ! grep -q 'SocketMode=0666' "$conf" 2>/dev/null; then
        sudo mkdir -p "$(dirname "$conf")"
        {
            echo '[Socket]'
            echo 'SocketMode=0666'
            echo 'RuntimeDirectoryMode=0755'
        } | sudo tee "$conf" >/dev/null
        echo 'override: socket mode written'
    fi
    # always re-apply: a pre-existing conf may not be loaded into the running socket
    sudo systemctl daemon-reload
    sudo systemctl restart podman.socket
    if [ "$(readlink -f /var/run/docker.sock 2>/dev/null)" != "/run/podman/podman.sock" ]; then
        sudo ln -sfn /run/podman/podman.sock /var/run/docker.sock
    fi
    echo 'machine ready: cluster operations need no sudo'

# --- cluster operations (no sudo; disposable) ---

# create or start the dev cluster + gateway stack
cluster-up:
    #!/usr/bin/env bash
    set -euo pipefail
    if k3d cluster get {{cluster}} >/dev/null 2>&1; then
        echo 'cluster exists; starting'
        k3d cluster start {{cluster}}
    else
        # no --wait: podman's compat API chokes on k3d's log streaming; we
        # wait on the node directly through kubectl instead
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

# ship rootless-podman-built images into the cluster
# usage: just images-import signet-signer:dev electrs:dev signet-faucet:dev
images-import *images:
    #!/usr/bin/env bash
    set -euo pipefail
    for img in {{images}}; do
        echo "importing $img"
        rm -f /tmp/signet-image.tar
        podman save -o /tmp/signet-image.tar "$img" 2>/dev/null \
            || podman save -o /tmp/signet-image.tar "localhost/$img" 2>/dev/null \
            || { docker pull "$img" && docker save -o /tmp/signet-image.tar "$img"; }
        docker cp /tmp/signet-image.tar k3d-{{cluster}}-server-0:/tmp/signet-image.tar
        docker exec k3d-{{cluster}}-server-0 sh -c \
            'ctr -n k8s.io -a /run/k3s/containerd/containerd.sock images import /tmp/signet-image.tar >/dev/null && rm /tmp/signet-image.tar'
    done
    rm -f /tmp/signet-image.tar
    echo 'images imported'

# --- image-side services (rootless podman; independent of the cluster) ---

# postgres for the API (rootful runtime — rootless podman cannot run it
# reliably). The image is staged from the rootless store; no registry pull
# happens as root.
db-up:
    #!/usr/bin/env bash
    set -euo pipefail
    image="docker.io/library/postgres:18"
    if [ "{{runtime}}" = "docker" ]; then
        root="docker"
        docker image inspect "$image" >/dev/null 2>&1 || docker pull "$image"
    else
        root="podman --url unix:///run/podman/podman.sock"
        podman image exists "$image" || podman pull "$image"
        rm -f /tmp/signet-pg.tar
        podman save "$image" -o /tmp/signet-pg.tar
        $root load -i /tmp/signet-pg.tar
    fi
    $root rm -f signet-postgres 2>/dev/null || true
    $root run -d --name signet-postgres \
        -e POSTGRES_USER={{env_var_or_default("POSTGRES_USER", "signet")}} \
        -e POSTGRES_DB={{env_var_or_default("POSTGRES_DB", "signet")}} \
        -e POSTGRES_PASSWORD={{env_var_or_default("POSTGRES_PASSWORD", "signet")}} \
        -p "{{env_var_or_default("POSTGRES_HOST_PORT", "55432")}}:5432" \
        -v signet-postgres-data:/var/lib/postgresql \
        "$image"
    echo 'postgres up'

db-down:
    podman rm -f signet-postgres

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
    check() { if eval "$2" >/dev/null 2>&1; then echo "  ok: $1"; else echo "  MISSING: $1 -- run: just host-setup"; fail=1; fi; }
    echo "machine (one-time bootstrap, runtime: {{runtime}}):"
    if [ "{{runtime}}" = "docker" ]; then
        check 'docker daemon reachable' 'docker info'
    else
        check 'rootful podman socket answers' \
            'podman --url unix:///run/podman/podman.sock info -f json | grep -q "\"Rootless\":false"'
        check '/var/run/docker.sock -> rootful podman socket' \
            '[ "$(readlink -f /var/run/docker.sock 2>/dev/null)" = "/run/podman/podman.sock" ]'
    fi
    echo "cluster:"
    check 'k3d cluster running' 'k3d cluster get {{cluster}}'
    exit $fail
