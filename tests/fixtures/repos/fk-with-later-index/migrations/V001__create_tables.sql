CREATE TABLE customers (
    id serial PRIMARY KEY,
    name text NOT NULL
);

CREATE TABLE orders (
    id serial PRIMARY KEY,
    customer_id integer NOT NULL,
    total numeric(12,2) NOT NULL
);
