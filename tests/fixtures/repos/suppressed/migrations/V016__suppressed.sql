-- pgm-lint:suppress-file PGM022

-- PGM022: REINDEX without CONCURRENTLY (suppressed)
REINDEX TABLE customers;

-- Setup for PGM508 violation in V017: table + leading index
CREATE TABLE IF NOT EXISTS index_test (
    id bigint PRIMARY KEY,
    customer_id bigint,
    created_at timestamptz
);

CREATE INDEX IF NOT EXISTS idx_index_test_customer ON index_test (customer_id);
