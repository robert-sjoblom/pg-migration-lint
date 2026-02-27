-- Drop billing.users — must NOT affect auth.users
DROP TABLE billing.users;

-- Prove auth.users still exists (fires rules for pre-existing table)
ALTER TABLE auth.users ADD COLUMN phone text;

-- New unqualified table — normalizes to public.users, a third distinct entry
CREATE TABLE users (
    id integer PRIMARY KEY,
    username text NOT NULL
);
