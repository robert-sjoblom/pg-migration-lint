Detects `ALTER TABLE ... RENAME COLUMN` on a pre-existing table. A column rename silently invalidates all queries, views, and application code referencing the old name.

**Example** (bad):
```sql
ALTER TABLE orders RENAME COLUMN status TO order_status;
-- All queries using 'status' will fail with 'column does not exist'
```

**Fix** (multi-step approach):
1. Add the new column.
2. Backfill data from the old column.
3. Update application code to use the new column.
4. Drop the old column.
