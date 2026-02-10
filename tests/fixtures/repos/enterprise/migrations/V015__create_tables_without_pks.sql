-- Bulk table creation: many tables WITHOUT primary keys
-- This is a real-world pattern where tables are created quickly
-- and PKs are added later in V026

-- Partner reporting
CREATE TABLE partner_reports (
    account_id bigint NOT NULL,
    report_type varchar(50) NOT NULL,
    generated_at timestamp(6) NOT NULL,
    report_data jsonb,
    UNIQUE (account_id, report_type, generated_at)
);

-- Client invitations
CREATE TABLE client_invitations (
    account_id bigint NOT NULL,
    invited_email varchar(255) NOT NULL,
    invited_by bigint NOT NULL,
    status varchar(50) NOT NULL DEFAULT 'PENDING',
    created timestamp(6) NOT NULL
);

-- Access control
CREATE TABLE external_access_rights (
    account_id bigint NOT NULL,
    external_account_id bigint NOT NULL,
    permission_level varchar(50) NOT NULL,
    granted_at timestamp(6) NOT NULL,
    UNIQUE (account_id, external_account_id)
);

-- Product grouping
CREATE TABLE product_groups (
    product_id integer NOT NULL,
    group_name varchar(100) NOT NULL,
    display_order integer NOT NULL DEFAULT 0,
    UNIQUE (product_id, group_name)
);

-- Feature flags
CREATE TABLE account_feature_flags (
    account_id bigint NOT NULL,
    feature_name varchar(100) NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    UNIQUE (account_id, feature_name)
);

-- Invoice references
CREATE TABLE order_invoice_references (
    order_id bigint NOT NULL,
    invoice_id varchar(50) NOT NULL,
    invoice_date date NOT NULL
);

-- Skip actions
CREATE TABLE order_skip_actions (
    order_id bigint NOT NULL,
    skip_reason varchar(200),
    skipped_by bigint NOT NULL,
    skipped_at timestamp(6) NOT NULL
);

-- Overdue tracking
CREATE TABLE overdue_invoices (
    account_id bigint NOT NULL,
    invoice_id varchar(50) NOT NULL,
    tax_id varchar(20),
    amount_due numeric(12,2) NOT NULL,
    due_date date NOT NULL,
    days_overdue integer NOT NULL
);

-- Balance tracking
CREATE TABLE usage_event_balances (
    account_id bigint NOT NULL,
    event_type varchar(100) NOT NULL,
    balance numeric(12,2) NOT NULL DEFAULT 0,
    last_updated timestamp(6) NOT NULL,
    UNIQUE (account_id, event_type)
);

-- Account locks
CREATE TABLE account_locks (
    account_id bigint NOT NULL UNIQUE,
    locked boolean NOT NULL DEFAULT false,
    locked_at timestamp(6),
    locked_by varchar(100)
);

-- Subscription locks
CREATE TABLE subscription_locks (
    subscription_id uuid NOT NULL UNIQUE,
    locked boolean NOT NULL DEFAULT false,
    locked_at timestamp(6)
);

-- Usage event metadata
CREATE TABLE usage_event_metadata (
    event_id uuid NOT NULL,
    metadata_key varchar(100) NOT NULL,
    metadata_value text,
    UNIQUE (event_id, metadata_key)
);

-- Scheduled dates
CREATE TABLE subscription_scheduled_dates (
    subscription_item_id uuid NOT NULL,
    scheduled_date date NOT NULL,
    schedule_type varchar(50) NOT NULL
);
