-- PGM003: CONCURRENTLY inside transaction (SqlLoader sets run_in_transaction=true)
CREATE INDEX idx_customers_customer_idx ON customers (customer_id);