-- Subscription billing periods
-- Note: no PK (PGM502) and FK without covering index on subscription_id (PGM501)

CREATE TABLE subscription_periods (
    subscription_id uuid NOT NULL REFERENCES subscriptions(id),
    period_start date NOT NULL,
    period_end date NOT NULL,
    billing_status varchar(50) NOT NULL DEFAULT 'PENDING'
);
