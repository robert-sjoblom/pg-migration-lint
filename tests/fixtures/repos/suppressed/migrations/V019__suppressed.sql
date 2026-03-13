-- pgm-lint:suppress-file PGM023

ALTER TABLE customers ALTER COLUMN customer_id SET DEFAULT 0;
ALTER TABLE customers ALTER COLUMN id SET DEFAULT 1;
