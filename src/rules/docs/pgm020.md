Detects `ALTER TABLE ... DISABLE TRIGGER` (specific name, ALL, or USER) on any table. Fires at MINOR on existing tables and at INFO on all other tables (new or unknown). Since re-enables are not tracked, all tables are flagged to catch cases where triggers may be left disabled.

**Example** (bad):
```sql
ALTER TABLE orders DISABLE TRIGGER ALL;
INSERT INTO orders SELECT * FROM staging;
```

**Fix** (re-enable in the same migration):
```sql
ALTER TABLE orders DISABLE TRIGGER ALL;
INSERT INTO orders SELECT * FROM staging;
ALTER TABLE orders ENABLE TRIGGER ALL;
```

**Recommended approach**:
1. Avoid disabling triggers in migrations entirely.
2. If you must disable triggers for bulk data loading, ensure the DISABLE and ENABLE are in the same migration and wrapped in a transaction.
3. On tables that are not pre-existing (new or unknown), this rule fires at INFO severity.
