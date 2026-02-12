-- PGM016: SET NOT NULL on existing column
ALTER TABLE customers ALTER COLUMN customer_id SET NOT NULL;

-- PGM017: ADD FK without NOT VALID on existing table
ALTER TABLE events ADD CONSTRAINT fk_events_customer
    FOREIGN KEY (event_type) REFERENCES customers(email);
-- (also triggers PGM003: FK without covering index on events.event_type)

-- PGM018: ADD CHECK without NOT VALID on existing table
ALTER TABLE customers ADD CONSTRAINT chk_email CHECK (email <> '');

-- PGM019: RENAME TABLE on existing table
ALTER TABLE accounts RENAME TO accounts_old;

-- PGM020: RENAME COLUMN on existing table
ALTER TABLE addresses RENAME COLUMN address_id TO addr_id;
