-- Subscription management tables
-- Note: subscription_invoices intentionally has NO primary key (fixed later in V026)

CREATE TABLE subscriptions (
    id uuid PRIMARY KEY,
    account_id bigint NOT NULL,
    customer_number varchar(50),
    status varchar(50) NOT NULL DEFAULT 'ACTIVE',
    payer_account_id bigint,
    created timestamp(6) NOT NULL
);

CREATE TABLE subscription_items (
    id uuid PRIMARY KEY,
    subscription_id uuid NOT NULL REFERENCES subscriptions(id),
    product_id bigint,
    status varchar(50) NOT NULL DEFAULT 'ACTIVE',
    quantity integer NOT NULL DEFAULT 1,
    created timestamp(6) NOT NULL,
    cancel_on date
);

-- Intentionally no PK (real-world pattern -- fixed later in V026)
CREATE TABLE subscription_invoices (
    subscription_id uuid NOT NULL REFERENCES subscriptions(id),
    payer_subscription_id uuid REFERENCES subscriptions(id),
    invoiced_from date NOT NULL,
    invoiced_until date NOT NULL,
    invoice_id varchar(19) NOT NULL,
    invoice_total numeric(12,2) NOT NULL
);

CREATE INDEX idx_subscription_items_subscription ON subscription_items (subscription_id);
