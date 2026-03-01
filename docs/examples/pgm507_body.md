Detects `ALTER TABLE ... ALTER COLUMN ... DROP NOT NULL` on tables that already exist. Dropping the NOT NULL constraint silently allows NULL values where application code may assume non-NULL.

**Example** (flagged):
```sql
ALTER TABLE orders ALTER COLUMN status DROP NOT NULL;
```

**Why it matters**:
- Aggregations behave differently with NULLs (`COUNT(col)` skips NULLs, `SUM` returns NULL if any input is NULL).
- Joins on nullable columns use NULL-unsafe equality (`NULL != NULL`).
- Application code that doesn't check for NULL may fail or produce incorrect results.

**Recommended approach**:
1. Verify that all application code paths handle NULLs in the column.
2. Update aggregations and joins that assume non-NULL.
3. Consider a CHECK constraint if only certain rows should allow NULL.
