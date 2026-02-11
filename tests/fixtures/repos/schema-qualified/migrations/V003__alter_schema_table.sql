-- Index on existing schema-qualified table without CONCURRENTLY -> PGM001 fires
CREATE INDEX idx_customers_name ON myschema.customers (name);
