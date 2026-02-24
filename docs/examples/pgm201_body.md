Detects `DROP TABLE` targeting a pre-existing table. The DDL is instant (no table scan or extended lock), so this is not a downtime risk — it is a data loss risk.

**Example**:
```sql
DROP TABLE orders;
```

**Recommended approach**:
1. Ensure no application code, views, or foreign keys reference the table.
2. Consider renaming the table first and waiting before dropping.
3. Take a backup of the table data if it may be needed later.
