-- Product catalog tables

CREATE TABLE products (
    id integer PRIMARY KEY,
    name varchar(1000) NOT NULL UNIQUE,
    article_number varchar(100),
    description text,
    price_structure jsonb,
    modules jsonb,
    features jsonb,
    active boolean NOT NULL DEFAULT true,
    created timestamp(6) NOT NULL
);

CREATE TABLE bundle_products (
    id integer PRIMARY KEY REFERENCES products(id),
    bundle_contents jsonb NOT NULL,
    promotion_id integer,
    finance_promotion_id integer
);
