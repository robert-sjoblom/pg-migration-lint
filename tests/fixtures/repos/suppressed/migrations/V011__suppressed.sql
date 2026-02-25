-- pgm-lint:suppress-file PGM004,PGM005
ALTER TABLE measurements DETACH PARTITION measurements_2023;
ALTER TABLE measurements ATTACH PARTITION measurements_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
