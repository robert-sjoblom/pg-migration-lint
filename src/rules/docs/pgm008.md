Detects `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a `DEFAULT` clause on a pre-existing table. This will fail immediately if the table has any rows.

**Example** (bad):
```sql
ALTER TABLE orders ADD COLUMN status text NOT NULL;
```

**Fix** (option A — add with default):
```sql
ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';
```

**Fix** (option B — add nullable, backfill, then constrain):
```sql
ALTER TABLE orders ADD COLUMN status text;
UPDATE orders SET status = 'pending' WHERE status IS NULL;
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
```
