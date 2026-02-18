-- ALTER COLUMN TYPE on existing tables
-- Triggers PGM007: potential table rewrite for each type change

ALTER TABLE overdue_invoices ALTER COLUMN tax_id TYPE text;
ALTER TABLE overdue_invoices ALTER COLUMN invoice_id TYPE text;
