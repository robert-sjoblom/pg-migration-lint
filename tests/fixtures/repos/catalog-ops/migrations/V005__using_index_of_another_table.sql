-- customers_id_idx belongs to customers, so PostgreSQL rejects the
-- ADD PRIMARY KEY (PGM027) and shipments is left without one (PGM502).
CREATE UNIQUE INDEX customers_id_idx ON customers (id);
CREATE TABLE shipments (id bigint NOT NULL);
ALTER TABLE shipments ADD PRIMARY KEY USING INDEX customers_id_idx;
