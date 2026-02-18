-- PGM203 + PGM204: TRUNCATE on existing table (audit_trail from V001)
TRUNCATE TABLE audit_trail CASCADE;

-- PGM201 + PGM202 + PGM401: DROP TABLE CASCADE on existing table (audit_trail from V001)
DROP TABLE audit_trail CASCADE;
