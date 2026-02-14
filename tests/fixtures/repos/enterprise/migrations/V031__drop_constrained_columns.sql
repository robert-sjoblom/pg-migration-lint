-- Drop columns that participate in constraints.
-- Triggers PGM013 (unique), PGM014 (PK), PGM015 (FK).

-- PGM014: drop PK column from usage_events (composite PK)
ALTER TABLE usage_events DROP COLUMN kafka_offset;

-- PGM013: drop column in UNIQUE constraint on products
ALTER TABLE products DROP COLUMN name;

-- PGM013: drop column in UNIQUE constraint on account_locks
ALTER TABLE account_locks DROP COLUMN account_id;

-- PGM015: drop FK column from orders (user_id → users.id)
ALTER TABLE orders DROP COLUMN user_id;

-- PGM015: drop FK column from connector_articles (connector_id → connector_catalog.id)
ALTER TABLE connector_articles DROP COLUMN connector_id;

-- PGM015: drop FK column from subscription_periods (subscription_id → subscriptions.id)
ALTER TABLE subscription_periods DROP COLUMN subscription_id;
