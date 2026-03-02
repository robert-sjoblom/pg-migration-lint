-- pgm-lint:suppress PGM508

CREATE INDEX IF NOT EXISTS idx_index_test_customer_date ON index_test (customer_id, created_at);
