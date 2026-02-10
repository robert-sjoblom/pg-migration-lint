-- Add columns across various tables

ALTER TABLE subscriptions ADD COLUMN referral_code varchar(50);
ALTER TABLE subscriptions ADD COLUMN payment_method varchar(50);
ALTER TABLE orders ADD COLUMN channel varchar(50);
ALTER TABLE orders ADD COLUMN source_system varchar(100);
