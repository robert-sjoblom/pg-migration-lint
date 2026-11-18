-- pgm-lint:suppress-file PGM402,PGM502
-- pgm-lint:suppress PGM024,PGM026
CREATE TABLE measurements_2026 PARTITION OF measurements
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
-- pgm-lint:suppress PGM024
CREATE TABLE readings_2026 PARTITION OF readings
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
