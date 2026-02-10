-- V004: "Don't Do This" type anti-patterns

-- PGM101: timestamp without time zone
CREATE TABLE audit_log_v2 (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type text NOT NULL,
    created_at timestamp NOT NULL DEFAULT now()
);

-- PGM102: timestamptz(0) precision
ALTER TABLE audit_log_v2 ADD COLUMN updated_at timestamptz(0);

-- PGM103: char(n) type
ALTER TABLE audit_log_v2 ADD COLUMN country_code char(2);

-- PGM104: money type
ALTER TABLE audit_log_v2 ADD COLUMN fee money;

-- PGM105: serial type
CREATE TABLE legacy_ids (
    id serial PRIMARY KEY,
    label text
);
