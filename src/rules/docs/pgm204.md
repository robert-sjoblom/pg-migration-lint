Detects `TRUNCATE TABLE ... CASCADE` targeting a pre-existing table. `CASCADE` silently extends the truncation to all tables that have foreign key references to the truncated table, and recursively to their dependents. The developer may not be aware of the full cascade chain, leading to unexpected data loss across multiple tables.

A plain `TRUNCATE` (without `CASCADE`) would fail if FK dependencies exist, which is a safer default.

**Example**:
```sql
TRUNCATE TABLE customers CASCADE;
-- If 'orders' has a FK referencing 'customers', CASCADE silently truncates 'orders' as well.
```

**Recommended approach**:
1. Identify all dependent tables before truncating.
2. Explicitly truncate each table in the correct order.
3. Use plain `TRUNCATE` (without `CASCADE`) so PostgreSQL will error if unexpected dependencies remain.
