-- Add CHECK constraints to subscription tables

ALTER TABLE subscriptions ADD CONSTRAINT chk_subscriptions_dates
    CHECK (created IS NOT NULL);

ALTER TABLE subscription_invoices ADD CONSTRAINT chk_invoice_dates
    CHECK (invoiced_from <= invoiced_until);
