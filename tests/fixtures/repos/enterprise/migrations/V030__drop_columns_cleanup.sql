-- Final cleanup: drop deprecated columns
-- Triggers PGM011

ALTER TABLE orders DROP COLUMN source_system;
ALTER TABLE subscription_items DROP COLUMN item_type;
