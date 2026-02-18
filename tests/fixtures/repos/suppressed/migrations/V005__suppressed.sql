-- pgm-lint:suppress-file PGM501,PGM013,PGM014,PGM015,PGM504,PGM505

ALTER TABLE customers ALTER COLUMN customer_id SET NOT NULL;

ALTER TABLE events ADD CONSTRAINT fk_events_customer
    FOREIGN KEY (event_type) REFERENCES customers(email);

ALTER TABLE customers ADD CONSTRAINT chk_email CHECK (email <> '');

ALTER TABLE accounts RENAME TO accounts_old;

ALTER TABLE addresses RENAME COLUMN address_id TO addr_id;
