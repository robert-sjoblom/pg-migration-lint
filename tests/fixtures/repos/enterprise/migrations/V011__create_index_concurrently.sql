-- Correct pattern: CONCURRENTLY indexes on existing tables

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_orders_created ON orders (created);
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_orders_user_id ON orders (user_id);
