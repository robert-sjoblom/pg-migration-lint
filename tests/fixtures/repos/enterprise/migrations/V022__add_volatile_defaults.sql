-- Add columns with volatile defaults on existing tables
-- Triggers PGM006: gen_random_uuid() and now() are volatile

ALTER TABLE orders ADD COLUMN tracking_id uuid DEFAULT gen_random_uuid();
ALTER TABLE subscription_items ADD COLUMN last_billed_at timestamptz DEFAULT now();
