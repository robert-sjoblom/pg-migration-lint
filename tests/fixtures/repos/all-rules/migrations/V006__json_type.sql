-- PGM108: json type instead of jsonb
CREATE TABLE events_v2 (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payload json NOT NULL
);
