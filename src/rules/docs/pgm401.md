Detects `DROP TABLE` or `DROP INDEX` without the `IF EXISTS` clause. Without `IF EXISTS`, the statement fails if the object does not exist, causing hard failures in migration pipelines that may be re-run.

**Example** (bad):
```sql
DROP TABLE orders;
DROP INDEX idx_orders_status;
```

**Fix**:
```sql
DROP TABLE IF EXISTS orders;
DROP INDEX IF EXISTS idx_orders_status;
```
