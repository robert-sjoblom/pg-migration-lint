Detects `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)` on a pre-existing table without `NOT VALID`. Without `NOT VALID`, PostgreSQL acquires an ACCESS EXCLUSIVE lock and scans the entire table to verify all existing rows.

**Example** (bad):
```sql
ALTER TABLE orders ADD CONSTRAINT orders_status_check
  CHECK (status IN ('pending', 'shipped', 'delivered'));
```

**Fix** (safe two-step pattern):
```sql
-- Step 1: Add with NOT VALID (instant, no scan)
ALTER TABLE orders ADD CONSTRAINT orders_status_check
  CHECK (status IN ('pending', 'shipped', 'delivered')) NOT VALID;
-- Step 2: Validate (SHARE UPDATE EXCLUSIVE lock, concurrent reads OK)
ALTER TABLE orders VALIDATE CONSTRAINT orders_status_check;
```

See also [PGM013](#pgm013), [PGM014](#pgm014).
