Detects `ALTER TABLE ... ALTER COLUMN ... TYPE ...` on pre-existing tables. Most type changes require a full table rewrite under an ACCESS EXCLUSIVE lock.

**Safe casts** (no finding):
- `varchar(N)` → `varchar(M)` where M > N
- `varchar(N)` → `text`
- `numeric(P,S)` → `numeric(P2,S)` where P2 > P and same scale
- `varbit(N)` → `varbit(M)` where M > N

**Info cast**: `timestamp` → `timestamptz` (safe in PG 15+ with UTC timezone; verify your timezone config)

**Example** (bad):
```sql
ALTER TABLE orders ALTER COLUMN amount TYPE bigint;
```

**Fix**:
```sql
-- Create a new column, backfill, and swap:
ALTER TABLE orders ADD COLUMN amount_new bigint;
UPDATE orders SET amount_new = amount;
ALTER TABLE orders DROP COLUMN amount;
ALTER TABLE orders RENAME COLUMN amount_new TO amount;
```
