-- PGM001: CREATE INDEX without CONCURRENTLY on existing table
CREATE INDEX idx_products_name ON products (name);

-- PGM501: FK without covering index (customer_id column exists from V001)
ALTER TABLE customers ADD CONSTRAINT fk_customers_self
    FOREIGN KEY (customer_id) REFERENCES customers(id);

-- PGM502: Table without primary key
CREATE TABLE audit_log (
    event_type text NOT NULL,
    payload text
);

-- PGM006: Volatile default on existing table (clock_timestamp is truly volatile)
ALTER TABLE customers ADD COLUMN token uuid DEFAULT gen_random_uuid();

-- PGM007: unsafe ALTER COLUMN TYPE on existing table
ALTER TABLE customers ALTER COLUMN email TYPE varchar(255);

-- PGM008: ADD COLUMN NOT NULL without default on existing table
ALTER TABLE products ADD COLUMN sku text NOT NULL;

-- PGM009: DROP COLUMN on existing table
ALTER TABLE products DROP COLUMN name;

-- PGM010: DROP COLUMN silently removes unique constraint (product_code has inline UNIQUE from V001)
ALTER TABLE products DROP COLUMN product_code;

-- PGM011: DROP COLUMN silently removes primary key (account_id is the PK from V001)
ALTER TABLE accounts DROP COLUMN account_id;

-- PGM012: DROP COLUMN silently removes foreign key (account_id references accounts from V001)
ALTER TABLE addresses DROP COLUMN account_id;
