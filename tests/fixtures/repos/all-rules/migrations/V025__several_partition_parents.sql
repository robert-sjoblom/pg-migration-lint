-- PGM026: one transaction takes ACCESS EXCLUSIVE on two partitioned parents
CREATE TABLE measurements_2026 PARTITION OF measurements
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
CREATE TABLE readings_2026 PARTITION OF readings
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
