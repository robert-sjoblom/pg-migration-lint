-- Platform audit log with proper CONCURRENTLY indexes

CREATE TABLE platform_audit_log (
    id bigserial PRIMARY KEY,
    account_id bigint,
    entity_type varchar(50) NOT NULL,
    entity_id varchar(100) NOT NULL,
    action varchar(50) NOT NULL,
    old_value jsonb,
    new_value jsonb,
    performed_by bigint,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_audit_log_account ON platform_audit_log (account_id);
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_audit_log_entity ON platform_audit_log (entity_type, entity_id);
