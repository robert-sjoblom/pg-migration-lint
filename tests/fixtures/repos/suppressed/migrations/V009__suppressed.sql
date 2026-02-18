-- pgm-lint:suppress-file PGM301,PGM302,PGM303,PGM402,PGM502,PGM506

INSERT INTO products (id, sku) VALUES (100, 'SKU-TEST');

UPDATE products SET sku = 'SKU-UPDATED' WHERE id = 100;

DELETE FROM products WHERE id = 100;

CREATE UNLOGGED TABLE scratch_data (id int, payload text);
