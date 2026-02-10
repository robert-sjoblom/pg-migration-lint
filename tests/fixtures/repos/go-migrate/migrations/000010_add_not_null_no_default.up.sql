-- PGM010: ADD COLUMN NOT NULL without default on existing table
ALTER TABLE users ADD COLUMN role TEXT NOT NULL;
