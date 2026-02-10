-- Add reseller order support
-- Note: reseller_orders has no PK (PGM004 violation)

CREATE TABLE reseller_orders (
    order_id bigint NOT NULL REFERENCES orders(id),
    reseller_account_id bigint NOT NULL,
    commission_percent numeric(5,2),
    commission_amount numeric(12,2)
);

ALTER TABLE orders ADD COLUMN reseller_id bigint;
