CREATE TABLE users (
    id UUID PRIMARY KEY,
    tenant_id BIGINT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_users_tenant ON users (tenant_id);
