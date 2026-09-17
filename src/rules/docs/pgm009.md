Detects `ALTER TABLE ... DROP COLUMN` on a pre-existing table. The DDL is cheap (PostgreSQL marks the column as dropped without rewriting), but the risk is application-level: queries referencing the dropped column will break.

**Example**:
```sql
ALTER TABLE orders DROP COLUMN legacy_status;
```

**Recommended approach**:
1. Remove all application references to the column.
2. Deploy the application change.
3. Drop the column in a subsequent migration.
