Detects `CREATE INDEX` on an existing table without the `CONCURRENTLY` option. Without `CONCURRENTLY`, PostgreSQL acquires a SHARE lock for the entire duration of the index build, blocking all writes (inserts, updates, deletes) while allowing reads.

**Example** (bad):
```sql
CREATE INDEX idx_orders_status ON orders (status);
```

**Fix**:
```sql
CREATE INDEX CONCURRENTLY idx_orders_status ON orders (status);
```

Does not fire when the table is created in the same set of changed files (locking an empty table is harmless). See also [PGM003](#pgm003).
