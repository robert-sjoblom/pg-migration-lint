Detects multiple separate `ALTER TABLE` statements targeting the same table within a single migration file, where all statements operate at the same lock level. Each separate statement acquires and releases the table lock independently, increasing the total lock contention window unnecessarily.

**Example** (bad — two lock acquisitions):
```sql
ALTER TABLE authors ALTER COLUMN name SET NOT NULL;
ALTER TABLE authors ALTER COLUMN email SET NOT NULL;
```

**Fix** (one lock acquisition):
```sql
ALTER TABLE authors
  ALTER COLUMN name SET NOT NULL,
  ALTER COLUMN email SET NOT NULL;
```

Chains are broken by any intervening statement that references the same table (e.g., `CREATE INDEX CONCURRENTLY`). Statements with different lock levels (e.g., `VALIDATE CONSTRAINT` vs `SET NOT NULL`) are tracked independently and do not trigger this rule against each other.

Tables created within the same set of changed files are exempt — lock contention for brand-new tables is harmless.
