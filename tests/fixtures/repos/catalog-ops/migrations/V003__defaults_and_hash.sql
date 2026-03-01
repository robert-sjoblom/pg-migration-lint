-- SET DEFAULT: orders.status gets a default value
ALTER TABLE orders ALTER COLUMN status SET DEFAULT 'pending';

-- DROP DEFAULT: remove it again
ALTER TABLE orders ALTER COLUMN status DROP DEFAULT;

-- Add a float column for V004 volatile-default testing
ALTER TABLE orders ADD COLUMN score float8;

-- Re-add FK (NOT VALID) so PGM501 can evaluate non-btree index coverage
ALTER TABLE orders ADD CONSTRAINT fk_customer2
    FOREIGN KEY (customer_id) REFERENCES customers(id) NOT VALID;

-- Non-btree index: hash index on orders.customer_id
-- Hash indexes work on scalar types but cannot serve FK lookups.
CREATE INDEX idx_orders_customer_hash ON orders USING hash (customer_id);
