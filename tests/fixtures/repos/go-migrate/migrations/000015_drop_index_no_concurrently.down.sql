-- Down migration: recreate the index without CONCURRENTLY.
-- PGM001 and PGM402 fire, capped to INFO (PGM901). PGM027 skips down
-- migrations, so idx_orders_status, which 000006 created and replay still
-- holds, is not reported as a taken name, and PGM402 does not leave the
-- statement to PGM027.
CREATE INDEX idx_orders_status ON orders (status);
