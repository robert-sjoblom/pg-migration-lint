Detects `DROP TABLE ... CASCADE` targeting a pre-existing table. `CASCADE` silently drops all dependent objects — foreign keys, views, triggers, and rules — that reference the dropped table. The developer may not be aware of all dependencies, leading to unexpected breakage.

A plain `DROP TABLE` (without `CASCADE`) would fail if dependencies exist, which is a safer default. `CASCADE` bypasses that safety net.

**Example**:
```sql
DROP TABLE customers CASCADE;
-- If 'orders' has a FK referencing 'customers', CASCADE silently drops that FK.
```

**Recommended approach**:
1. Identify all dependent objects before dropping.
2. Explicitly drop or alter dependencies in separate migration steps.
3. Use plain `DROP TABLE` (without `CASCADE`) so PostgreSQL will error if unexpected dependencies remain.
