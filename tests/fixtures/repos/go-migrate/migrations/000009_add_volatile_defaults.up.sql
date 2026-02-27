-- PGM006: Volatile default on existing table column
-- now() is STABLE and does not trigger PGM006; clock_timestamp() is truly volatile
ALTER TABLE orders ADD COLUMN updated_at TIMESTAMPTZ DEFAULT clock_timestamp();
ALTER TABLE accounts ADD COLUMN tracking_id UUID DEFAULT gen_random_uuid();
