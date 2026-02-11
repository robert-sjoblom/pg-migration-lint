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
CREATE INDEX idx_customers_email ON customers (email);
