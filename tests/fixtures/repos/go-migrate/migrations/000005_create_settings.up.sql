CREATE TABLE settings (
    tenant_id BIGINT PRIMARY KEY,
    auto_approve BOOLEAN DEFAULT FALSE NOT NULL,
    notification_email TEXT,
    modified TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_settings_tenant ON settings (tenant_id);
