Detects `ALTER TABLE ... DETACH PARTITION` on a pre-existing partitioned table without the `CONCURRENTLY` option. Plain `DETACH PARTITION` acquires ACCESS EXCLUSIVE on the parent and child, blocking all reads and writes for the duration. PostgreSQL 14+ supports `DETACH PARTITION ... CONCURRENTLY`, which uses a weaker lock.

**Example**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023;
```

**Fix**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;
```

Note: `DETACH PARTITION CONCURRENTLY` requires PostgreSQL 14+.
