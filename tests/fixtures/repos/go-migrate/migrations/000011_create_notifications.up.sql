-- PGM003: FK without index (on notification_preferences.user_id)
-- PGM007: Volatile default (on notifications.id)
CREATE TABLE notifications (
    id UUID DEFAULT gen_random_uuid(),
    tenant_id BIGINT NOT NULL,
    user_id UUID NOT NULL,
    title TEXT NOT NULL,
    body TEXT,
    read BOOLEAN DEFAULT FALSE,
    created TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id)
);

CREATE TABLE notification_preferences (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel TEXT NOT NULL DEFAULT 'email',
    enabled BOOLEAN DEFAULT TRUE NOT NULL
);
