create table environments (
    id uuid primary key,
    name text not null,
    npub_owner text not null,
    workspace_id uuid,
    status text not null check (status in ('provisioning', 'ready', 'expired', 'destroyed')),
    block_policy text not null,
    signet_challenge text not null,
    component_explorer boolean not null default false,
    component_indexer boolean not null default false,
    component_faucet boolean not null default false,
    component_lightning text check (component_lightning in ('lnd')),
    rpc_endpoint text not null,
    indexer_endpoint text,
    explorer_endpoint text,
    faucet_endpoint text,
    ln_endpoint text,
    ttl_secs bigint,
    created_at timestamptz not null default now(),
    expires_at timestamptz,
    current_snapshot_id uuid
);

create index environments_npub_owner_idx on environments (npub_owner);

create table api_tokens (
    id uuid primary key,
    npub_owner text not null,
    environment_id uuid references environments (id) on delete cascade,
    token_hash text not null unique,
    created_at timestamptz not null default now(),
    revoked_at timestamptz
);

create index api_tokens_npub_owner_idx on api_tokens (npub_owner);
