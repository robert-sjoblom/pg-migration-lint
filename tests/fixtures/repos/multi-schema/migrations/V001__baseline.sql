-- Schemas (parsed as Ignored — no catalog impact, just valid SQL)
CREATE SCHEMA auth;
CREATE SCHEMA inventory;
CREATE SCHEMA billing;

CREATE TABLE auth.users (
    id integer PRIMARY KEY,
    email text NOT NULL
);

CREATE TABLE auth.sessions (
    id integer PRIMARY KEY,
    user_id integer NOT NULL,
    token text NOT NULL
);

CREATE TABLE inventory.products (
    id integer PRIMARY KEY,
    name text NOT NULL,
    sku text NOT NULL
);

CREATE TABLE inventory.stock (
    id integer PRIMARY KEY,
    product_id integer NOT NULL,
    warehouse text NOT NULL,
    quantity integer NOT NULL DEFAULT 0
);

-- Same table name "users" in a different schema
CREATE TABLE billing.users (
    id integer PRIMARY KEY,
    account_number text NOT NULL
);

CREATE TABLE billing.invoices (
    id integer PRIMARY KEY,
    user_id integer NOT NULL,
    amount numeric(12,2) NOT NULL
);

-- Unqualified → normalizes to public.orders
CREATE TABLE orders (
    id integer PRIMARY KEY,
    product_id integer NOT NULL,
    customer_id integer NOT NULL,
    total numeric(12,2) NOT NULL
);
