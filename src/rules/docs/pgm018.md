Detects a `CLUSTER` statement targeting a table that already exists in the database. `CLUSTER` rewrites the entire table and all its indexes in a new physical order, holding an ACCESS EXCLUSIVE lock for the full duration. Unlike `VACUUM FULL`, there is no online alternative. On large tables this causes complete unavailability for minutes to hours.

**Example**:
```sql
CLUSTER orders USING idx_orders_created_at;
```

**Recommended approach**:
1. Schedule `CLUSTER` during a maintenance window when downtime is acceptable.
2. Consider `pg_repack` or `pg_squeeze` for online table rewrites.
3. For new tables, `CLUSTER` is fine — this rule only fires on existing tables.
