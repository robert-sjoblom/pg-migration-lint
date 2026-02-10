-- PGM002: DROP INDEX without CONCURRENTLY on existing table's index
DROP INDEX idx_customers_email;

-- PGM005: UNIQUE NOT NULL but no PK
CREATE TABLE settings (
    key text NOT NULL,
    value text,
    UNIQUE (key)
);

-- PGM006: CONCURRENTLY inside transaction (SqlLoader sets run_in_transaction=true)
CREATE INDEX CONCURRENTLY idx_customers_customer_id ON customers (customer_id);

-- PGM012: ADD PRIMARY KEY on existing table without prior unique constraint
ALTER TABLE events ADD PRIMARY KEY (id);
