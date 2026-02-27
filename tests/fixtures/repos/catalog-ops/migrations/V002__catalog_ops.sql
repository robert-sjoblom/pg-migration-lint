-- VALIDATE CONSTRAINT: FK becomes valid in catalog
ALTER TABLE orders VALIDATE CONSTRAINT fk_customer;

-- VALIDATE CONSTRAINT: CHECK becomes valid in catalog
ALTER TABLE orders VALIDATE CONSTRAINT chk_status;

-- DROP CONSTRAINT: remove FK from catalog
ALTER TABLE orders DROP CONSTRAINT fk_customer;

-- DROP NOT NULL: status becomes nullable in catalog
ALTER TABLE orders ALTER COLUMN status DROP NOT NULL;

-- DROP NOT NULL: key becomes nullable → settings no longer qualifies
-- for PGM503 (UNIQUE NOT NULL without PK) because key is now nullable
ALTER TABLE settings ALTER COLUMN key DROP NOT NULL;
