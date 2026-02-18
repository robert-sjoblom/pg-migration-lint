-- PGM301: INSERT INTO existing table
INSERT INTO products (id, sku) VALUES (100, 'SKU-TEST');

-- PGM302: UPDATE on existing table
UPDATE products SET sku = 'SKU-UPDATED' WHERE id = 100;

-- PGM303: DELETE FROM existing table
DELETE FROM products WHERE id = 100;

-- PGM506: CREATE UNLOGGED TABLE
CREATE UNLOGGED TABLE scratch_data (id int, payload text);
