-- pgm-lint:suppress-file PGM002,PGM005,PGM006

DROP INDEX idx_customers_email;

CREATE TABLE settings (
    key text NOT NULL,
    value text,
    UNIQUE (key)
);

CREATE INDEX CONCURRENTLY idx_customers_customer_id ON customers (customer_id);
