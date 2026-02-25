Detects `ALTER TABLE ... ATTACH PARTITION` where the child table already exists and has no CHECK constraint that references the partition key columns. Without a pre-validated CHECK constraint that implies the partition bound, PostgreSQL performs a full table scan under ACCESS EXCLUSIVE lock to verify every row.

**Example**:
```sql
ALTER TABLE measurements ATTACH PARTITION measurements_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
```

**Recommended approach**:
1. Add a CHECK constraint mirroring the partition bound with `NOT VALID`.
2. Validate the constraint separately (`VALIDATE CONSTRAINT` — allows concurrent reads/writes).
3. Attach the partition (scan is skipped because the constraint is already validated).

```sql
ALTER TABLE measurements_2024 ADD CONSTRAINT measurements_2024_bound
    CHECK (ts >= '2024-01-01' AND ts < '2025-01-01') NOT VALID;
ALTER TABLE measurements_2024 VALIDATE CONSTRAINT measurements_2024_bound;
ALTER TABLE measurements ATTACH PARTITION measurements_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
```
