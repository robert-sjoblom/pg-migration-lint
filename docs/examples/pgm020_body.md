Detects `ALTER TABLE ... DISABLE TRIGGER` (specific name, ALL, or USER) on a table that already exists. Disabling triggers in a migration bypasses business logic and foreign key enforcement. If the re-enable is missing or the migration fails partway, referential integrity is silently lost.

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
3. This rule does not fire on tables created in the same changeset.
