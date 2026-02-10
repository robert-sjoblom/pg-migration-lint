-- ADD COLUMN NOT NULL without default on existing table
-- Triggers PGM010

ALTER TABLE subscription_items ADD COLUMN item_type varchar(50) NOT NULL;
