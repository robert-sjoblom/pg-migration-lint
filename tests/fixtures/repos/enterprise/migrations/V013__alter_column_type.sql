-- ALTER COLUMN TYPE on existing table
-- Triggers PGM009: potential table rewrite

ALTER TABLE partner_client_orders ALTER COLUMN partner_account_id TYPE text;
