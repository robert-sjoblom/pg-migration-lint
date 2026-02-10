-- Down migration: recreate the index without CONCURRENTLY
-- This will trigger PGM001 but should be capped to INFO (PGM008)
CREATE INDEX idx_orders_status ON orders (status);
