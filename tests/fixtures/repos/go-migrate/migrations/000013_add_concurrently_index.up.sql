-- Clean: CONCURRENTLY indexes on existing tables (no PGM001)
CREATE INDEX CONCURRENTLY idx_notifications_tenant ON notifications (tenant_id);
CREATE INDEX CONCURRENTLY idx_notifications_user ON notifications (user_id);
CREATE INDEX CONCURRENTLY idx_notification_prefs_user ON notification_preferences (user_id);
