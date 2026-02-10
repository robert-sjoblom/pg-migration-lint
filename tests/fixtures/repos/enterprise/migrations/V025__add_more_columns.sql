-- Add columns to various subscription and usage tables

ALTER TABLE subscriptions ADD COLUMN cancelled_at timestamp(6);
ALTER TABLE subscriptions ADD COLUMN cancelled_by bigint;
ALTER TABLE subscription_items ADD COLUMN discount_percent numeric(5,2);
ALTER TABLE usage_events ADD COLUMN source_system varchar(100);
