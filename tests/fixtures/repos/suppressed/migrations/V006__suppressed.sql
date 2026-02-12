-- pgm-lint:suppress-file PGM108

-- PGM108: json type instead of jsonb (suppressed)
CREATE TABLE events_v2 (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payload json NOT NULL
);
