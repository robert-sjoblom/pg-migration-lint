Detects `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE` on an existing table without `USING INDEX`. Without `USING INDEX`, PostgreSQL builds a new unique index under ACCESS EXCLUSIVE lock. `NOT VALID` does **not** apply to UNIQUE constraints.

**Example** (bad):
```sql
ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE (email);
```

**Fix** (safe pattern):
```sql
CREATE UNIQUE INDEX CONCURRENTLY idx_orders_email ON orders (email);
ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_orders_email;
```

See also [PGM016](#pgm016).
