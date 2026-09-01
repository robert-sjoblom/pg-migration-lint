Detects `ALTER TABLE ... DETACH PARTITION` on a pre-existing partitioned table without the `CONCURRENTLY` option. Plain `DETACH PARTITION` acquires ACCESS EXCLUSIVE on the parent and child, blocking all reads and writes for the duration. PostgreSQL 14+ supports `DETACH PARTITION ... CONCURRENTLY`, which takes only SHARE UPDATE EXCLUSIVE on the parent instead, so it does not block readers and writers through the parent the way the plain form does.

Two things to plan for with `CONCURRENTLY`:

1. It cannot run inside a transaction block, so the statement must be issued on its own (`runInTransaction="false"` for Liquibase, the equivalent setting elsewhere). PGM003 does not currently detect this: it only checks `CREATE`/`DROP INDEX CONCURRENTLY`.
2. It still waits for conflicting traffic on the parent, and an interrupted wait does not roll back. The first phase has already committed, so the child is left pending-detach in `pg_inherits` (`inhdetachpending`). Queries routed through the parent already skip the partition's rows while the rows remain in it, and the state does not self-resolve. A retry of the same `DETACH ... CONCURRENTLY` fails with SQLSTATE `55000` (hint: `FINALIZE`). Recovery is `DETACH PARTITION ... FINALIZE` or `DROP TABLE` on the child. While pending, that `DROP TABLE` still takes ACCESS EXCLUSIVE on the parent. It can lose to the same traffic that caused the interruption.

**Example**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023;
```

**Fix**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;
```
