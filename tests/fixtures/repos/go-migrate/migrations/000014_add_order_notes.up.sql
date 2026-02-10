-- Clean: nullable column with no default is fine
ALTER TABLE orders ADD COLUMN notes TEXT;
-- Clean: NOT NULL with a literal default is fine
ALTER TABLE orders ADD COLUMN priority INT NOT NULL DEFAULT 0;
