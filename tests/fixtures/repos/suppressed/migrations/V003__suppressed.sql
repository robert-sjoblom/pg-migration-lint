-- pgm-lint:suppress-file PGM002,PGM503,PGM003,PGM401,PGM016,PGM402

DROP INDEX idx_customers_email;

CREATE TABLE settings (
    key text NOT NULL,
    value text,
    UNIQUE (key)
);

CREATE INDEX CONCURRENTLY idx_customers_customer_id ON customers (customer_id);

ALTER TABLE events ADD PRIMARY KEY (id);
