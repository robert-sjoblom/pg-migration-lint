-- PGM023: multiple ALTER TABLE statements on the same existing table
ALTER TABLE customers ALTER COLUMN email SET NOT NULL;
ALTER TABLE customers ALTER COLUMN customer_id SET NOT NULL;
