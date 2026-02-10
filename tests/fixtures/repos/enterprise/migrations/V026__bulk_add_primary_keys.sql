-- The big PK fix migration
-- Adds auto-increment PK columns and creates PKs for all tables that were missing them

ALTER TABLE partner_reports ADD COLUMN id bigserial;
ALTER TABLE partner_reports ADD CONSTRAINT pk_partner_reports PRIMARY KEY (id);

ALTER TABLE client_invitations ADD COLUMN id bigserial;
ALTER TABLE client_invitations ADD CONSTRAINT pk_client_invitations PRIMARY KEY (id);

ALTER TABLE external_access_rights ADD COLUMN id bigserial;
ALTER TABLE external_access_rights ADD CONSTRAINT pk_external_access_rights PRIMARY KEY (id);

ALTER TABLE product_groups ADD COLUMN id bigserial;
ALTER TABLE product_groups ADD CONSTRAINT pk_product_groups PRIMARY KEY (id);

ALTER TABLE account_feature_flags ADD COLUMN id bigserial;
ALTER TABLE account_feature_flags ADD CONSTRAINT pk_account_feature_flags PRIMARY KEY (id);

ALTER TABLE order_invoice_references ADD COLUMN id bigserial;
ALTER TABLE order_invoice_references ADD CONSTRAINT pk_order_invoice_references PRIMARY KEY (id);

ALTER TABLE order_skip_actions ADD COLUMN id bigserial;
ALTER TABLE order_skip_actions ADD CONSTRAINT pk_order_skip_actions PRIMARY KEY (id);

ALTER TABLE overdue_invoices ADD COLUMN id bigserial;
ALTER TABLE overdue_invoices ADD CONSTRAINT pk_overdue_invoices PRIMARY KEY (id);

ALTER TABLE usage_event_balances ADD COLUMN id bigserial;
ALTER TABLE usage_event_balances ADD CONSTRAINT pk_usage_event_balances PRIMARY KEY (id);

ALTER TABLE account_locks ADD COLUMN id bigserial;
ALTER TABLE account_locks ADD CONSTRAINT pk_account_locks PRIMARY KEY (id);

ALTER TABLE subscription_locks ADD COLUMN id bigserial;
ALTER TABLE subscription_locks ADD CONSTRAINT pk_subscription_locks PRIMARY KEY (id);

ALTER TABLE usage_event_metadata ADD COLUMN id bigserial;
ALTER TABLE usage_event_metadata ADD CONSTRAINT pk_usage_event_metadata PRIMARY KEY (id);

ALTER TABLE subscription_scheduled_dates ADD COLUMN id bigserial;
ALTER TABLE subscription_scheduled_dates ADD CONSTRAINT pk_subscription_scheduled_dates PRIMARY KEY (id);

ALTER TABLE subscription_invoices ADD COLUMN id bigserial;
ALTER TABLE subscription_invoices ADD CONSTRAINT pk_subscription_invoices PRIMARY KEY (id);

ALTER TABLE reseller_orders ADD COLUMN id bigserial;
ALTER TABLE reseller_orders ADD CONSTRAINT pk_reseller_orders PRIMARY KEY (id);

ALTER TABLE subscription_periods ADD COLUMN id bigserial;
ALTER TABLE subscription_periods ADD CONSTRAINT pk_subscription_periods PRIMARY KEY (id);
