-- pgm-lint:suppress-file PGM001,PGM501,PGM502,PGM006,PGM007,PGM008,PGM009,PGM010,PGM011,PGM012,PGM014,PGM402

CREATE INDEX idx_products_name ON products (name);

ALTER TABLE customers ADD CONSTRAINT fk_customers_self
    FOREIGN KEY (customer_id) REFERENCES customers(id);

CREATE TABLE audit_log (
    event_type text NOT NULL,
    payload text
);

ALTER TABLE customers ADD COLUMN token uuid DEFAULT gen_random_uuid();
ALTER TABLE customers ALTER COLUMN email TYPE varchar(255);
ALTER TABLE products ADD COLUMN sku text NOT NULL;
ALTER TABLE products DROP COLUMN name;
ALTER TABLE products DROP COLUMN product_code;
ALTER TABLE accounts DROP COLUMN account_id;
ALTER TABLE addresses DROP COLUMN account_id;
