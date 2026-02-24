Detects `DELETE FROM` targeting a table that already exists in the database. On large tables, deletes hold row locks, generate WAL, fire `ON DELETE` triggers, and produce dead tuples until autovacuum runs.

**Example** (flagged):
```sql
DELETE FROM audit_log WHERE created_at < '2020-01-01';
```

**Recommended approach**:
1. Verify the row count is bounded.
2. For large deletes, batch in chunks.
3. If no triggers need to fire, consider `TRUNCATE` instead.
