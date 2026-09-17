Detects `ALTER TABLE ... ADD COLUMN` with a volatile function call as the DEFAULT expression on an existing table. On PostgreSQL 11+, non-volatile defaults are applied lazily without rewriting the table. Volatile defaults (`random()`, `gen_random_uuid()`, `clock_timestamp()`, etc.) force a full table rewrite under an ACCESS EXCLUSIVE lock.

Note: `now()` and `current_timestamp` are **STABLE** in PostgreSQL, not volatile. They are evaluated once at ALTER TABLE time and the single value is stored in the catalog — no table rewrite occurs.

**Severity levels per finding**:
- **Minor**: Known volatile functions (`random`, `gen_random_uuid`, `uuid_generate_v4`, `clock_timestamp`, `timeofday`, `txid_current`, `nextval`)
- **Info**: Unknown function calls — developer should verify volatility
- **No finding**: Stable functions (`now`, `current_timestamp`, `statement_timestamp`, etc.) and literal defaults

**Example** (flagged):
```sql
ALTER TABLE orders ADD COLUMN token uuid DEFAULT gen_random_uuid();
```

**Fix**:
```sql
ALTER TABLE orders ADD COLUMN token uuid;
-- Then backfill:
UPDATE orders SET token = gen_random_uuid() WHERE token IS NULL;
```

Does not fire on `CREATE TABLE` (no existing rows to rewrite).
