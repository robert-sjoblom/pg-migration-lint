-- PGM007: Volatile default on existing table column
ALTER TABLE orders ADD COLUMN updated_at TIMESTAMPTZ DEFAULT now();
ALTER TABLE accounts ADD COLUMN tracking_id UUID DEFAULT gen_random_uuid();
