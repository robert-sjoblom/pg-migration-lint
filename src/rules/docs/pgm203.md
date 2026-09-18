Detects `TRUNCATE TABLE` targeting a pre-existing table. Unlike `DELETE`, `TRUNCATE` does not fire `ON DELETE` triggers, does not log individual row deletions, and cannot be filtered with a `WHERE` clause. The operation is irreversible once committed.

**Example**:
```sql
TRUNCATE TABLE audit_trail;
```

**Recommended approach**:
1. Ensure the data is truly disposable or has been backed up.
2. Consider whether `ON DELETE` triggers need to fire — if so, use `DELETE`.
3. If truncating for a schema migration, document the intent clearly.
