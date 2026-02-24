Detects `DROP INDEX` without the `CONCURRENTLY` option, where the index belongs to a pre-existing table. Without `CONCURRENTLY`, PostgreSQL acquires an ACCESS EXCLUSIVE lock on the table.

**Example** (bad):
```sql
DROP INDEX idx_orders_status;
```

**Fix**:
```sql
DROP INDEX CONCURRENTLY idx_orders_status;
```

See also [PGM003](#pgm003).
