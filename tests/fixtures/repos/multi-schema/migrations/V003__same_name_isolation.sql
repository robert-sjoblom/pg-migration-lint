-- Alter auth.users — must NOT be confused with billing.users
ALTER TABLE auth.users ADD COLUMN last_login timestamp;

-- Alter billing.users — must NOT be confused with auth.users
ALTER TABLE billing.users ADD COLUMN billing_address text;

-- Index on auth.users — tracked on auth.users only
CREATE INDEX idx_auth_users_email ON auth.users (email);

-- Index on billing.users — tracked on billing.users only
CREATE INDEX idx_billing_users_acct ON billing.users (account_number);
