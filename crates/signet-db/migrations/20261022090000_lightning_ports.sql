-- Per-instance LND TCP ports: REST listens on `port`, gRPC on `port + 1`,
-- both allocated from the lightning pool and exposed by the instance's
-- LoadBalancer service.
alter table environment_lightning add column port integer;
