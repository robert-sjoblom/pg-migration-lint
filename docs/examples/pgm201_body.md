Detects `DROP TABLE` targeting a pre-existing table. All data in the table is permanently lost, and any queries, views, foreign keys, or application code referencing it will break.

**Table outside any partitioned hierarchy: not a downtime risk**: the DDL itself is instant. PostgreSQL does not scan the table or hold an extended lock, so the risk is data loss, not downtime.

**Table inside a partitioned hierarchy: this IS a downtime risk**: `DROP TABLE child` takes ACCESS EXCLUSIVE on the **parent** partitioned table, not just on the child, and holds it until the transaction commits.

- A reader or writer holding the parent does not make the drop fail outright: the drop waits for ACCESS EXCLUSIVE on the parent. With a migration-grade `lock_timeout` that wait ends as SQLSTATE `55P03` and the transaction rolls back; without one it waits for as long as the traffic holds the parent.
- While the drop is queued, its pending ACCESS EXCLUSIVE already blocks new readers that the current lock holder would have let through, so an unbounded wait stalls the whole parent table.
- One transaction holds ACCESS EXCLUSIVE on every parent it has touched until `COMMIT`, so a single lock it cannot get discards all the work the migration had already done.
- Traffic aimed straight at a sibling partition does **not** block the drop. It is traffic routed through the parent that does, so an idle-looking partition is no evidence that the drop is safe.

**Example**:
```sql
DROP TABLE orders;
```

**Recommended approach**:
1. Ensure no application code, views, or foreign keys reference the table.
2. Consider renaming the table first and waiting before dropping.
3. Take a backup of the table data if it may be needed later.
4. If the table is a partition, `DETACH PARTITION ... CONCURRENTLY` first, then `DROP` the now-standalone table.

`DETACH PARTITION ... CONCURRENTLY` cannot run inside a transaction block, and it still waits for traffic on the parent — it can fail with `55P03` too. An interrupted detach leaves the partition pending detach, where `DROP TABLE` still takes ACCESS EXCLUSIVE on the parent. See also [PGM004](#pgm004).
