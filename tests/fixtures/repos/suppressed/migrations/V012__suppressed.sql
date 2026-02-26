-- pgm-lint:suppress-file PGM019
ALTER TABLE customers ADD CONSTRAINT excl_customers
    EXCLUDE USING gist (email WITH =);
