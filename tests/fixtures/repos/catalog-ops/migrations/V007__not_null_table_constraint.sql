CREATE TABLE ledger (
    entry_id bigint,
    CONSTRAINT uq_ledger_entry UNIQUE (entry_id),
    NOT NULL entry_id
);

ALTER TABLE orders ADD CONSTRAINT orders_status_nn NOT NULL status;

ALTER TABLE settings ADD CONSTRAINT settings_key_nn NOT NULL key NOT VALID;
