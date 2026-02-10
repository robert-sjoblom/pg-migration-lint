-- Batch index creation with CONCURRENTLY (correct pattern)

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_connector_requests_account ON connector_requests (account_id);
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_connector_requests_connector ON connector_requests (connector_id);
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_connector_articles_connector ON connector_articles (connector_id);
