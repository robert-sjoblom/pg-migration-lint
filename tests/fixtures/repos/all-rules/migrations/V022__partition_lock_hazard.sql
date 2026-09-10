-- PGM024: DROP TABLE on a live partition child takes ACCESS EXCLUSIVE on the parent
DROP TABLE measurements_2024;

-- PGM024: CREATE TABLE ... PARTITION OF a pre-existing parent takes ACCESS EXCLUSIVE too
CREATE TABLE measurements_2025 PARTITION OF measurements
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
