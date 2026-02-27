CREATE TABLE customers (
    id bigint PRIMARY KEY,
    email text NOT NULL UNIQUE,
    customer_id bigint
);
CREATE TABLE products (
    id bigint PRIMARY KEY,
    name text NOT NULL,
    product_code text NOT NULL UNIQUE
);
CREATE TABLE events (
    id bigint NOT NULL,
    event_type text NOT NULL,
    payload text
);
CREATE TABLE accounts (
    account_id bigint PRIMARY KEY,
    account_name text NOT NULL
);
CREATE TABLE addresses (
    address_id bigint PRIMARY KEY,
    account_id bigint REFERENCES accounts(account_id)
);
CREATE TABLE audit_trail (
    id bigint PRIMARY KEY,
    action text NOT NULL
);
CREATE INDEX idx_addresses_account_id ON addresses (account_id);
CREATE INDEX idx_customers_email ON customers (email);

-- Partitioned table setup for PGM004/PGM005 tests
CREATE TABLE measurements (
    id bigint NOT NULL,
    ts timestamptz NOT NULL,
    value double precision
) PARTITION BY RANGE (ts);

CREATE TABLE measurements_2023 (
    id bigint NOT NULL,
    ts timestamptz NOT NULL,
    value double precision
);
ALTER TABLE measurements_2023 ADD CONSTRAINT measurements_2023_bound
    CHECK (ts >= '2023-01-01' AND ts < '2024-01-01');
ALTER TABLE measurements ATTACH PARTITION measurements_2023
    FOR VALUES FROM ('2023-01-01') TO ('2024-01-01');

CREATE TABLE measurements_2024 (
    id bigint NOT NULL,
    ts timestamptz NOT NULL,
    value double precision
);

-- Schema setup for PGM205 test
CREATE SCHEMA myschema;
CREATE TABLE myschema.orders (id bigint PRIMARY KEY, total numeric);
