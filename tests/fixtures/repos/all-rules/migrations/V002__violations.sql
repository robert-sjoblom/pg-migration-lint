-- PGM001: CREATE INDEX without CONCURRENTLY on existing table
CREATE INDEX idx_products_name ON products (name);

-- PGM003: FK without covering index (customer_id column exists from V001)
ALTER TABLE customers ADD CONSTRAINT fk_customers_self
    FOREIGN KEY (customer_id) REFERENCES customers(id);

-- PGM004: Table without primary key
CREATE TABLE audit_log (
    event_type text NOT NULL,
    payload text
);

-- PGM007: Volatile default on existing table
ALTER TABLE customers ADD COLUMN created_at timestamptz DEFAULT now();

-- PGM009: unsafe ALTER COLUMN TYPE on existing table
ALTER TABLE customers ALTER COLUMN email TYPE varchar(255);

-- PGM010: ADD COLUMN NOT NULL without default on existing table
ALTER TABLE products ADD COLUMN sku text NOT NULL;

-- PGM011: DROP COLUMN on existing table
ALTER TABLE products DROP COLUMN name;
