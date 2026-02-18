-- PGM016: ADD PRIMARY KEY on existing table without prior unique constraint
ALTER TABLE audit_log ADD COLUMN id UUID NOT NULL DEFAULT gen_random_uuid();
ALTER TABLE audit_log ADD PRIMARY KEY (id);
