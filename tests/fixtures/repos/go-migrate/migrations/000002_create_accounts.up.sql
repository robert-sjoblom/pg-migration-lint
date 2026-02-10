CREATE TABLE accounts (
    id UUID PRIMARY KEY,
    tenant_id BIGINT NOT NULL,
    name TEXT NOT NULL,
    status INT DEFAULT 0,
    owner_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_accounts_tenant ON accounts (tenant_id);
CREATE INDEX idx_accounts_owner ON accounts (owner_id);
