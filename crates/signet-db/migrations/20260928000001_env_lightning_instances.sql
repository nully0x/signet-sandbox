-- Lightning is a list of named instances (spec §5): its own child table,
-- one row per instance, cascading with the environment. The old single
-- value column was never populated (provisioning for Lightning is new).
create table environment_lightning (
    environment_id uuid not null references environments (id) on delete cascade,
    name text not null,
    implementation text not null check (implementation in ('lnd', 'cln', 'eclair', 'ldk_node')),
    primary key (environment_id, name)
);

alter table environments drop column component_lightning;
