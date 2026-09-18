Detects `ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE (...)` on a table that already exists. Adding an EXCLUDE constraint acquires an ACCESS EXCLUSIVE lock and scans all existing rows to verify the exclusion condition. Unlike CHECK and FOREIGN KEY constraints, PostgreSQL does not support `NOT VALID` for EXCLUDE constraints — there is no safe online path.

**Example** (bad):
```sql
ALTER TABLE reservations
  ADD CONSTRAINT excl_overlap
  EXCLUDE USING gist (room WITH =, period WITH &&);
```

**Recommended approach**:
1. Schedule the migration during a maintenance window when downtime is acceptable.
2. For new tables, adding EXCLUDE constraints in `CREATE TABLE` is fine — this rule only fires on existing tables.
