-- Down migration: undo the primary key addition
-- This file will trigger PGM009 (DROP COLUMN) but severity should be capped to INFO
ALTER TABLE audit_log DROP COLUMN id;
