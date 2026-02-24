Detects `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause. Without `IF NOT EXISTS`, the statement fails if the object already exists, causing hard failures in migration pipelines that may be re-run.

**Example** (bad):
```sql
CREATE TABLE orders (id bigint PRIMARY KEY);
CREATE INDEX idx_orders_status ON orders (status);
```

**Fix**:
```sql
CREATE TABLE IF NOT EXISTS orders (id bigint PRIMARY KEY);
CREATE INDEX IF NOT EXISTS idx_orders_status ON orders (status);
```

See also [PGM401](#pgm401).
