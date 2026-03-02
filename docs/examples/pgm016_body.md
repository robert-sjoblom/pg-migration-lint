Detects `ALTER TABLE ... ADD PRIMARY KEY` on an existing table that doesn't use `USING INDEX`. Without `USING INDEX`, PostgreSQL builds a new index under ACCESS EXCLUSIVE lock, even if a matching unique index already exists.

Additionally, even with `USING INDEX`, if any PK columns are nullable, PostgreSQL implicitly runs `SET NOT NULL` under ACCESS EXCLUSIVE lock.

**Example** (bad):
```sql
ALTER TABLE orders ADD PRIMARY KEY (id);
```

**Fix** (safe pattern):
```sql
CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);
ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;
```

If the PK columns are nullable, `USING INDEX` alone is not enough — PostgreSQL still runs an implicit `SET NOT NULL` (full table scan under ACCESS EXCLUSIVE). Make columns NOT NULL first using the safe CHECK-constraint pattern from [PGM013](#pgm013):

```sql
-- Step 1: Make column NOT NULL safely (see PGM013)
ALTER TABLE orders ADD CONSTRAINT orders_id_nn
  CHECK (id IS NOT NULL) NOT VALID;
ALTER TABLE orders VALIDATE CONSTRAINT orders_id_nn;
ALTER TABLE orders ALTER COLUMN id SET NOT NULL;
ALTER TABLE orders DROP CONSTRAINT orders_id_nn;
-- Step 2: Now USING INDEX is truly instant
CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);
ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;
```
