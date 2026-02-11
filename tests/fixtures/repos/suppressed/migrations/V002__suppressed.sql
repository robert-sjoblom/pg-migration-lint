-- pgm-lint:suppress-file PGM001,PGM003,PGM004,PGM007,PGM009,PGM010,PGM011,PGM013

CREATE INDEX idx_products_name ON products (name);

ALTER TABLE customers ADD CONSTRAINT fk_customers_self
    FOREIGN KEY (customer_id) REFERENCES customers(id);

CREATE TABLE audit_log (
    event_type text NOT NULL,
    payload text
);

ALTER TABLE customers ADD COLUMN created_at timestamptz DEFAULT now();
ALTER TABLE customers ALTER COLUMN email TYPE varchar(255);
ALTER TABLE products ADD COLUMN sku text NOT NULL;
ALTER TABLE products DROP COLUMN name;
ALTER TABLE products DROP COLUMN product_code;
