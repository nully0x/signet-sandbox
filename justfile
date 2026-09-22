set dotenv-load := true

cluster := "signet"
# Container runtime: docker if present, else the rootless podman socket
# (k3d speaks the docker API; K3D_FIX_DNS works around rootless podman DNS).
sock := `if command -v docker >/dev/null 2>&1; then echo none; else echo "unix:///run/user/$(id -u)/podman/podman.sock"; fi`
export DOCKER_HOST := if sock == "none" { "" } else { sock }
export K3D_FIX_DNS := if sock == "none" { "0" } else { "1" }
compose := "docker compose -f deploy/compose/docker-compose.yml --env-file .env"

default:
    just --list

# --- local dev (compose + native cargo, fast loop) ---

# generate signer key + challenge + credentials into .env (pass --force to regenerate)
local-setup *ARGS:
    bash scripts/local-setup.sh {{ARGS}}

# start bitcoind + postgres locally
dev-up:
    {{compose}} up -d

dev-down:
    {{compose}} down

dev-ps:
    {{compose}} ps

# wipe local chain + db volumes (needed after regenerating .env)
dev-reset:
    {{compose}} down -v

# run services natively against the compose stack (reads .env)
dev-signer:
    cargo run -p signet-signer -- run

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

# --- k3s dev cluster (deploy/dev) ---

gateway_api_crds := "https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.1/standard-install.yaml"

cluster-up:
    k3d cluster create {{cluster}} \
        --port "80:80@loadbalancer" \
        --port "443:443@loadbalancer" \
        --port "50002:50002@loadbalancer" \
        --volume signet-data:/var/lib/rancher/k3s/storage@server:0 \
        --wait
    k3d kubeconfig write {{cluster}}
    kubectl apply -f {{gateway_api_crds}}
    kubectl apply -f deploy/dev/traefik-gateway-config.yaml
    kubectl -n kube-system rollout status deploy/traefik --timeout=180s
    kubectl wait --for=condition=accepted gatewayclass/traefik --timeout=120s

cluster-down:
    k3d cluster delete {{cluster}}

cluster-status:
    kubectl get nodes
    kubectl get pods -A

# keygen + secrets + image build/import + deploy to k3s + rollout wait
deploy-dev:
    bash scripts/deploy-dev.sh

# provision an isolated per-env stack: env-<id> namespace + per-env key/challenge
env-provision id:
    bash scripts/env-provision.sh {{id}}

env-destroy id:
    kubectl delete namespace env-{{id}} --ignore-not-found

deploy-dev-manifests:
    kubectl apply -k deploy/dev

undeploy-dev:
    kubectl delete -k deploy/dev --ignore-not-found

# --- production (deploy/production) ---

deploy-production:
    kubectl apply -k deploy/production

# --- verification gates ---

verify-build:
    cargo build --workspace

verify-all: verify-build test
