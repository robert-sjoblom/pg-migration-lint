Detects `ALTER TABLE ... ALTER COLUMN ... SET NOT NULL` on a pre-existing table. This acquires an ACCESS EXCLUSIVE lock and performs a full table scan to verify no existing rows contain NULL.

**Example** (bad):
```sql
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
```

**Fix** (safe three-step pattern, PostgreSQL 12+):
```sql
-- Step 1: Add a CHECK constraint with NOT VALID (instant)
ALTER TABLE orders ADD CONSTRAINT orders_status_nn
  CHECK (status IS NOT NULL) NOT VALID;
-- Step 2: Validate (SHARE UPDATE EXCLUSIVE lock, concurrent reads OK)
ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;
-- Step 3: Set NOT NULL (instant since PG 12 sees the validated CHECK)
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
-- Step 4 (optional): Drop the now-redundant CHECK
ALTER TABLE orders DROP CONSTRAINT orders_status_nn;
```

See also [PGM015](#pgm015).
