-- pgm-lint:suppress-file PGM403

CREATE TABLE IF NOT EXISTS customers (
    id bigint PRIMARY KEY,
    email text NOT NULL
);
