-- Per-env electrs TCP port (spec §11.2): allocated from 50002–50502 when the
-- electrs component (indexer or explorer) is provisioned.
alter table environments add column electrum_port integer;
