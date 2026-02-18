-- pgm-lint:suppress-file PGM101,PGM102,PGM103,PGM104,PGM105,PGM502,PGM006,PGM402

-- PGM101: timestamp without time zone (suppressed)
CREATE TABLE audit_log_v2 (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type text NOT NULL,
    created_at timestamp NOT NULL DEFAULT now()
);

-- PGM102: timestamptz(0) precision (suppressed)
ALTER TABLE audit_log_v2 ADD COLUMN updated_at timestamptz(0);

-- PGM103: char(n) type (suppressed)
ALTER TABLE audit_log_v2 ADD COLUMN country_code char(2);

-- PGM104: money type (suppressed)
ALTER TABLE audit_log_v2 ADD COLUMN fee money;

-- PGM105: serial type (suppressed)
CREATE TABLE legacy_ids (
    id serial PRIMARY KEY,
    label text
);
