Detects two statement shapes that both take ACCESS EXCLUSIVE on a pre-existing partitioned parent table: dropping a live partition child with `DROP TABLE`, and creating a new child directly with `CREATE TABLE ... PARTITION OF`. Both lock the parent, not just the child, and hold that lock until commit — blocking every reader and writer routed through the parent, and therefore every sibling partition, for the duration. A reader or writer already holding the parent does not make either statement fail outright: it queues behind them, and a merely queued lock request already blocks new readers the current holder would have allowed. `DROP TABLE` additionally destroys the child's data irreversibly and has no `CONCURRENTLY` variant at all.

For `DROP TABLE`, the safe alternative is `DETACH PARTITION ... CONCURRENTLY` (PostgreSQL 14+) first, then dropping the now-standalone table — see PGM004.

For `CREATE TABLE ... PARTITION OF`, there is no `CONCURRENTLY` form. Instead, create the table standalone and `ATTACH PARTITION` it: `ATTACH` takes only `SHARE UPDATE EXCLUSIVE` on the parent, plus a brief `ACCESS EXCLUSIVE` on the new table itself, which nothing is using yet and so is uncontended. See PGM005 for the `CHECK` constraint that lets the attach skip a full scan of the child.

**Example**:
```sql
DROP TABLE measurements_2023;

CREATE TABLE measurements_2025 PARTITION OF measurements
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
```

**Fix**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;
DROP TABLE measurements_2023;

CREATE TABLE measurements_2025 (LIKE measurements INCLUDING ALL);
ALTER TABLE measurements_2025 ADD CONSTRAINT measurements_2025_bound
    CHECK (ts >= '2025-01-01' AND ts < '2026-01-01') NOT VALID;
ALTER TABLE measurements_2025 VALIDATE CONSTRAINT measurements_2025_bound;
ALTER TABLE measurements ATTACH PARTITION measurements_2025
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
```
