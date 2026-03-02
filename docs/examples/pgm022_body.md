Detects `REINDEX TABLE`, `REINDEX INDEX`, `REINDEX SCHEMA`, `REINDEX DATABASE`, or `REINDEX SYSTEM` without the `CONCURRENTLY` option. Without `CONCURRENTLY`, `REINDEX` acquires an ACCESS EXCLUSIVE lock on the target table (or parent table for `REINDEX INDEX`), blocking all reads and writes for the duration of the rebuild.

**Example** (bad):
```sql
REINDEX TABLE orders;
REINDEX INDEX idx_orders_status;
```

**Fix**:
```sql
REINDEX TABLE CONCURRENTLY orders;
REINDEX INDEX CONCURRENTLY idx_orders_status;
```

The `CONCURRENTLY` option (PostgreSQL 12+) rebuilds the index without holding an exclusive lock for the entire operation. It takes longer but allows normal reads and writes to continue.

See also [PGM003](#pgm003).
