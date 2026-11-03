-- Explorer URLs are composed live from the env host: the stored
-- explorer_endpoint column is replaced by the per-env explorer port
-- (20600-20999), exposed by the env's LoadBalancer service. The indexer
-- endpoint composes live too (electrum_port + env host), so its stored
-- column goes as well.
alter table environments drop column explorer_endpoint;
alter table environments drop column indexer_endpoint;
alter table environments add column explorer_port integer;
