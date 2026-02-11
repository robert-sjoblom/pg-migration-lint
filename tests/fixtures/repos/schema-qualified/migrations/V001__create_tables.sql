CREATE TABLE myschema.customers (
    id integer PRIMARY KEY,
    name text NOT NULL
);

CREATE TABLE orders (
    id integer PRIMARY KEY,
    customer_id integer NOT NULL,
    total numeric(12,2) NOT NULL
);
