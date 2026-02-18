-- PGM403: CREATE TABLE IF NOT EXISTS for already-existing table
CREATE TABLE IF NOT EXISTS customers (
    id bigint PRIMARY KEY,
    email text NOT NULL
);
