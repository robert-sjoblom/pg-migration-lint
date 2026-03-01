-- SET DEFAULT with volatile function: PGM006 should fire at INFO level.
-- random() is volatile and returns float8, matching the score column type.
-- SET DEFAULT only affects future inserts, not existing rows.
ALTER TABLE orders ALTER COLUMN score SET DEFAULT random();
