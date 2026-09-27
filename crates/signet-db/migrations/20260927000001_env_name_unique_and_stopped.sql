-- Environment names resolve server-side: environment-scoped methods accept
-- a name or an id, so a name must be unique per owner.
create unique index if not exists environments_owner_name_idx
    on environments (npub_owner, name);

-- 'stopped' = compute scaled to zero, storage kept (spec §9). The Reaper
-- still enforces expires_at wall-clock for stopped environments.
alter table environments drop constraint environments_status_check;
alter table environments add constraint environments_status_check
    check (status in ('provisioning', 'ready', 'stopped', 'expired', 'destroyed'));
