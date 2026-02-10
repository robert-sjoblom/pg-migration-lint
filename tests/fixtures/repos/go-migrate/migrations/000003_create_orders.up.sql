CREATE TABLE orders (
    id UUID PRIMARY KEY,
    tenant_id BIGINT NOT NULL,
    account_id UUID NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    amount FLOAT NOT NULL,
    currency TEXT NOT NULL DEFAULT 'USD',
    status INT DEFAULT 0,
    created TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_orders_tenant ON orders (tenant_id);
CREATE INDEX idx_orders_account ON orders (account_id);
