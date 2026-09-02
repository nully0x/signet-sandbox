set dotenv-load := true

cluster := "signet"
namespace := "signet-platform"
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

cluster-up:
    k3d cluster create {{cluster}} \
        --port "80:80@loadbalancer" \
        --port "443:443@loadbalancer" \
        --volume signet-data:/var/lib/rancher/k3s/storage@server:0 \
        --wait
    k3d kubeconfig write {{cluster}}

cluster-down:
    k3d cluster delete {{cluster}}

cluster-status:
    kubectl get nodes
    kubectl get pods -A

# keygen + secrets + image build/import + deploy to k3s + rollout wait
deploy-dev:
    bash scripts/deploy-dev.sh

deploy-dev-manifests:
    kubectl apply -k deploy/dev

undeploy-dev:
    kubectl delete -k deploy/dev --ignore-not-found

# wipe bitcoind chain state in k3s (needed if challenge changed)
reset-chain:
    kubectl -n {{namespace}} delete statefulset/bitcoind --ignore-not-found
    kubectl -n {{namespace}} delete pvc data-bitcoind-0 --ignore-not-found
    kubectl apply -k deploy/dev

logs-signer:
    kubectl -n {{namespace}} logs deploy/signet-signer -f

logs-bitcoind:
    kubectl -n {{namespace}} logs sts/bitcoind -f

# --- production (deploy/production) ---

deploy-production:
    kubectl apply -k deploy/production

# --- verification gates ---

verify-build:
    cargo build --workspace

verify-all: verify-build test
