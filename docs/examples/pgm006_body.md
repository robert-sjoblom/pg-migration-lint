Detects `ALTER TABLE ... ADD COLUMN` with a function call as the DEFAULT expression on an existing table. On PostgreSQL 11+, non-volatile defaults are applied lazily without rewriting the table. Volatile defaults (`now()`, `random()`, `gen_random_uuid()`, etc.) force a full table rewrite under an ACCESS EXCLUSIVE lock.

**Severity levels per finding**:
- **Minor**: Known volatile functions (`now`, `current_timestamp`, `random`, `gen_random_uuid`, `uuid_generate_v4`, `clock_timestamp`, `timeofday`, `txid_current`, `nextval`)
- **Info**: Unknown function calls — developer should verify volatility

**Example** (flagged):
```sql
ALTER TABLE orders ADD COLUMN created_at timestamptz DEFAULT now();
```

**Fix**:
```sql
ALTER TABLE orders ADD COLUMN created_at timestamptz;
-- Then backfill:
UPDATE orders SET created_at = now() WHERE created_at IS NULL;
```

Does not fire on `CREATE TABLE` (no existing rows to rewrite).
