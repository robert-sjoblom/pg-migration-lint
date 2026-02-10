CREATE TABLE customers (
    id bigint PRIMARY KEY,
    email text NOT NULL UNIQUE,
    customer_id bigint
);
CREATE TABLE products (
    id bigint PRIMARY KEY,
    name text NOT NULL
);
CREATE INDEX idx_customers_email ON customers (email);
