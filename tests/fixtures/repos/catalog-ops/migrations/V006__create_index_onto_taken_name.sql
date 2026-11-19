-- customers_id_idx belongs to customers (V005), so PostgreSQL rejects the
-- CREATE UNIQUE INDEX (PGM027) and builds nothing: invoices is left without
-- a unique key, so PGM502 fires rather than PGM503.
CREATE TABLE invoices (id bigint NOT NULL);
CREATE UNIQUE INDEX customers_id_idx ON invoices (id);
