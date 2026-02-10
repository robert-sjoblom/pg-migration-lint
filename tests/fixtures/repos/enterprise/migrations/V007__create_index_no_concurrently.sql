-- Index creation without CONCURRENTLY on existing tables
-- Triggers PGM001 for all three indexes

CREATE INDEX idx_subscriptions_account ON subscriptions (account_id);
CREATE INDEX idx_subscriptions_status ON subscriptions (status);
CREATE INDEX idx_orders_status ON orders (status);
