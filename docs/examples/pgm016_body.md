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
