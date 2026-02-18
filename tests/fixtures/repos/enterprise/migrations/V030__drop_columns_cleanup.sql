-- Final cleanup: drop deprecated columns
-- Triggers PGM009

ALTER TABLE orders DROP COLUMN source_system;
ALTER TABLE subscription_items DROP COLUMN item_type;
