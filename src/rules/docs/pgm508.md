Detects `CREATE INDEX` where the new index's columns are an exact duplicate or a leading prefix of another index on the same table. Redundant indexes waste disk space, slow writes, and add vacuum overhead.

**Example** (flagged):
```sql
-- Existing index: CREATE INDEX idx_orders_cust ON orders (customer_id);
CREATE INDEX idx_orders_cust_dup ON orders (customer_id);
-- Exact duplicate of idx_orders_cust.

CREATE INDEX idx_orders_cust_short ON orders (customer_id);
-- Redundant: idx_orders_cust_date ON (customer_id, created_at) covers this prefix.
```

**Why it matters**:
- Every INSERT, UPDATE, and DELETE must maintain all indexes on the table.
- Redundant indexes double the I/O cost for no query benefit.
- A btree index on `(a, b)` already serves lookups on `(a)` alone.

**Does NOT fire when**:
- The shorter index is UNIQUE (it enforces a constraint the longer one doesn't).
- Either index is partial (has a WHERE clause).
- Either index has expression entries.
- The indexes use different access methods (btree vs GIN vs ...).

**Fix**: Drop the redundant index:
```sql
DROP INDEX CONCURRENTLY idx_orders_cust_short;
```
