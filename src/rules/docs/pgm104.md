Detects columns declared as `money`. The `money` type formats output according to the `lc_monetary` locale setting, making it unreliable across environments and causing data corruption when moving data between servers.

**Example** (bad):
```sql
CREATE TABLE orders (total money NOT NULL);
```

**Fix**:
```sql
CREATE TABLE orders (total numeric(12,2) NOT NULL);
```
