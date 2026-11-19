---
layout: default
title: Rule Reference
---

# Rule Reference
{: #rule-reference}

`pg-migration-lint` ships with 56 lint rules across seven categories:

- **Unsafe DDL** (PGM001-PGM027) — detect locking, rewrites, runtime failures, and silent side effects in DDL migrations.
- **Type Anti-patterns** (PGM101-PGM109) — flag column types that should be avoided per PostgreSQL best practice.
- **Destructive Operations** (PGM201-PGM205) — flag data-loss operations.
- **DML in Migrations** (PGM301-PGM303) — flag data manipulation statements on existing tables.
- **Idempotency Guards** (PGM401-PGM403) — detect missing IF EXISTS / IF NOT EXISTS guards.
- **Schema Design** (PGM501-PGM509) — schema quality and informational findings.
- **Meta-behavior** (PGM901) — cross-cutting behavior modifiers (not standalone lint rules).

## How to use

Run `pg-migration-lint --explain PGM001` to see the full explanation for any rule from the CLI.

### Suppression

Rules can be suppressed inline with SQL comments:

```sql
-- Suppress a single statement:
-- pgm-lint:suppress PGM001
CREATE INDEX idx_foo ON bar (col);

-- Suppress an entire file (must appear before any SQL statements):
-- pgm-lint:suppress-file PGM001,PGM501
```

## Severity levels

| Severity | Meaning |
|----------|---------|
| **Critical** | Causes downtime, data corruption, or deploy failure. Must be fixed. |
| **Major** | Performance issues or schema-integrity problems. Should be fixed. |
| **Minor** | Potentially unintended behavior or style issues. Review recommended. |
| **Info** | Informational — flags intentional but noteworthy operations. |

---

## 0xx — Unsafe DDL Rules

### PGM001 — Missing CONCURRENTLY on CREATE INDEX
{: #pgm001}

**Severity**: Critical

Detects `CREATE INDEX` on an existing table without the `CONCURRENTLY` option. Without `CONCURRENTLY`, PostgreSQL acquires a SHARE lock for the entire duration of the index build, blocking all writes (inserts, updates, deletes) while allowing reads.

**Example** (bad):
```sql
CREATE INDEX idx_orders_status ON orders (status);
```

**Fix**:
```sql
CREATE INDEX CONCURRENTLY idx_orders_status ON orders (status);
```

Does not fire when the table is created in the same set of changed files (locking an empty table is harmless). It does fire on a `CREATE INDEX` whose name a table or index in the schema already has, which [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports too: with a free name, the statement still needs `CONCURRENTLY`. See also [PGM003](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm003).

---

### PGM002 — Missing CONCURRENTLY on DROP INDEX
{: #pgm002}

**Severity**: Critical

Detects `DROP INDEX` without the `CONCURRENTLY` option, where the index belongs to a pre-existing table. Without `CONCURRENTLY`, PostgreSQL acquires an ACCESS EXCLUSIVE lock on the table.

**Example** (bad):
```sql
DROP INDEX idx_orders_status;
```

**Fix**:
```sql
DROP INDEX CONCURRENTLY idx_orders_status;
```

Does not fire on a `DROP INDEX` that names the index behind a PRIMARY KEY or UNIQUE constraint, alone or with other indexes. PostgreSQL rejects the whole statement and drops nothing; [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports it.

See also [PGM003](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm003).

---

### PGM003 — CONCURRENTLY inside transaction
{: #pgm003}

**Severity**: Critical

Detects `CREATE INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY`, `ALTER TABLE ... DETACH PARTITION ... CONCURRENTLY`, or `REINDEX ... CONCURRENTLY` that runs inside a transaction block. PostgreSQL does not allow any of these CONCURRENTLY operations inside a transaction block: the command fails at runtime with SQLSTATE 25001.

A statement runs inside a transaction block when the migration tool runs the migration in a transaction (a Liquibase changeset without `runInTransaction="false"`, or a plain SQL file while `run_in_transaction` is true), or when the file opens one itself with `BEGIN` or `START TRANSACTION`.

A `COMMIT` in a migration the tool runs in a transaction does not get the statements after it out of one. It ends the tool's transaction, and the statements after it run in a new one: a JDBC runner such as Flyway or Liquibase starts one before the next statement, and golang-migrate's default mode sends the file as one query, in which PostgreSQL runs the statements after the `COMMIT` as a new implicit transaction. Give the `CONCURRENTLY` statement a migration of its own that runs outside a transaction instead.

**Example** (bad, a Liquibase changeset with the default `runInTransaction`):
```xml
<changeSet id="1" author="dev">
  <sql>CREATE INDEX CONCURRENTLY idx_foo ON bar (col);</sql>
</changeSet>
```

**Fix**:
```xml
<changeSet id="1" author="dev" runInTransaction="false">
  <sql>CREATE INDEX CONCURRENTLY idx_foo ON bar (col);</sql>
</changeSet>
```

**Example** (bad, a plain SQL file with `run_in_transaction = false` that opens a transaction block itself):
```sql
BEGIN;
CREATE INDEX CONCURRENTLY idx_foo ON bar (col);
COMMIT;
```

**Fix**: drop the `BEGIN` and `COMMIT`, so the statement runs on its own:
```sql
CREATE INDEX CONCURRENTLY idx_foo ON bar (col);
```

Does not fire on a `DROP INDEX CONCURRENTLY` that names the index behind a PRIMARY KEY or UNIQUE constraint. PostgreSQL rejects it in or out of a transaction, and [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports it; its fix, `ALTER TABLE ... DROP CONSTRAINT`, needs no `CONCURRENTLY`. It does fire on a `CREATE INDEX CONCURRENTLY` whose name is already taken, which [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports too: inside a transaction PostgreSQL fails it on the transaction first, and with a free name it still has to run outside one.

See also [PGM001](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm001), [PGM002](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm002), and [PGM022](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm022).

---

### PGM004 — DETACH PARTITION on existing table without CONCURRENTLY
{: #pgm004}

**Severity**: Critical

Detects `ALTER TABLE ... DETACH PARTITION` on a pre-existing partitioned table without the `CONCURRENTLY` option. Plain `DETACH PARTITION` acquires ACCESS EXCLUSIVE on the parent and child, blocking all reads and writes for the duration. PostgreSQL 14+ supports `DETACH PARTITION ... CONCURRENTLY`, which takes only SHARE UPDATE EXCLUSIVE on the parent instead, so it does not block readers and writers through the parent the way the plain form does.

Two things to plan for with `CONCURRENTLY`:

1. It cannot run inside a transaction block, so the statement must be issued on its own (`runInTransaction="false"` for Liquibase, the equivalent setting elsewhere). PGM003 flags this.
2. It still waits for conflicting traffic on the parent, and an interrupted wait does not roll back. The first phase has already committed, so the child is left pending-detach in `pg_inherits` (`inhdetachpending`). Queries routed through the parent already skip the partition's rows while the rows remain in it, and the state does not self-resolve. A retry of the same `DETACH ... CONCURRENTLY` fails with SQLSTATE `55000` (hint: `FINALIZE`). Recovery is `DETACH PARTITION ... FINALIZE` or `DROP TABLE` on the child. While pending, that `DROP TABLE` still takes ACCESS EXCLUSIVE on the parent. It can lose to the same traffic that caused the interruption.

When one transaction locks several partitioned parents, with plain `DETACH PARTITION` or the statements [PGM024](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm024) reports, [PGM026](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm026) reports that as well: each parent stays locked while the transaction waits for the next.

**Example**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023;
```

**Fix**:
```sql
ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;
```

---

### PGM005 — ATTACH PARTITION of existing table without pre-validated CHECK
{: #pgm005}

**Severity**: Major

Detects `ALTER TABLE ... ATTACH PARTITION` where the child table already exists and has no CHECK constraint that references the partition key columns. Without a pre-validated CHECK constraint that implies the partition bound, PostgreSQL performs a full table scan under ACCESS EXCLUSIVE lock to verify every row.

**Example**:
```sql
ALTER TABLE measurements ATTACH PARTITION measurements_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
```

**Recommended approach**:
1. Add a CHECK constraint mirroring the partition bound with `NOT VALID`.
2. Validate the constraint separately (`VALIDATE CONSTRAINT` — allows concurrent reads/writes).
3. Attach the partition (scan is skipped because the constraint is already validated).

```sql
ALTER TABLE measurements_2024 ADD CONSTRAINT measurements_2024_bound
    CHECK (ts >= '2024-01-01' AND ts < '2025-01-01') NOT VALID;
ALTER TABLE measurements_2024 VALIDATE CONSTRAINT measurements_2024_bound;
ALTER TABLE measurements ATTACH PARTITION measurements_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
```

---

### PGM006 — Volatile default on column
{: #pgm006}

**Severity**: Minor

Detects `ALTER TABLE ... ADD COLUMN` with a volatile function call as the DEFAULT expression on an existing table. On PostgreSQL 11+, non-volatile defaults are applied lazily without rewriting the table. Volatile defaults (`random()`, `gen_random_uuid()`, `clock_timestamp()`, etc.) force a full table rewrite under an ACCESS EXCLUSIVE lock.

Note: `now()` and `current_timestamp` are **STABLE** in PostgreSQL, not volatile. They are evaluated once at ALTER TABLE time and the single value is stored in the catalog — no table rewrite occurs.

**Severity levels per finding**:
- **Minor**: Known volatile functions (`random`, `gen_random_uuid`, `uuid_generate_v4`, `clock_timestamp`, `timeofday`, `txid_current`, `nextval`)
- **Info**: Unknown function calls — developer should verify volatility
- **No finding**: Stable functions (`now`, `current_timestamp`, `statement_timestamp`, etc.) and literal defaults

**Example** (flagged):
```sql
ALTER TABLE orders ADD COLUMN token uuid DEFAULT gen_random_uuid();
```

**Fix**:
```sql
ALTER TABLE orders ADD COLUMN token uuid;
-- Then backfill:
UPDATE orders SET token = gen_random_uuid() WHERE token IS NULL;
```

Does not fire on `CREATE TABLE` (no existing rows to rewrite).

---

### PGM007 — ALTER COLUMN TYPE on existing table causes table rewrite
{: #pgm007}

**Severity**: Critical

Detects `ALTER TABLE ... ALTER COLUMN ... TYPE ...` on pre-existing tables. Most type changes require a full table rewrite under an ACCESS EXCLUSIVE lock.

**Safe casts** (no finding):
- `varchar(N)` → `varchar(M)` where M > N
- `varchar(N)` → `text`
- `numeric(P,S)` → `numeric(P2,S)` where P2 > P and same scale
- `varbit(N)` → `varbit(M)` where M > N

**Info cast**: `timestamp` → `timestamptz` (safe in PG 9.2+ when timezone=UTC; verify your timezone config)

**Example** (bad):
```sql
ALTER TABLE orders ALTER COLUMN amount TYPE bigint;
```

**Fix**:
```sql
-- Create a new column, backfill, and swap:
ALTER TABLE orders ADD COLUMN amount_new bigint;
UPDATE orders SET amount_new = amount;
ALTER TABLE orders DROP COLUMN amount;
ALTER TABLE orders RENAME COLUMN amount_new TO amount;
```

---

### PGM008 — ADD COLUMN NOT NULL without DEFAULT on existing table
{: #pgm008}

**Severity**: Critical

Detects `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a `DEFAULT` clause on a pre-existing table. This will fail immediately if the table has any rows.

**Example** (bad):
```sql
ALTER TABLE orders ADD COLUMN status text NOT NULL;
```

**Fix** (option A — add with default):
```sql
ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';
```

**Fix** (option B — add nullable, backfill, then constrain):
```sql
ALTER TABLE orders ADD COLUMN status text;
UPDATE orders SET status = 'pending' WHERE status IS NULL;
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
```

---

### PGM009 — DROP COLUMN on existing table
{: #pgm009}

**Severity**: Info

Detects `ALTER TABLE ... DROP COLUMN` on a pre-existing table. The DDL is cheap (PostgreSQL marks the column as dropped without rewriting), but the risk is application-level: queries referencing the dropped column will break.

**Example**:
```sql
ALTER TABLE orders DROP COLUMN legacy_status;
```

**Recommended approach**:
1. Remove all application references to the column.
2. Deploy the application change.
3. Drop the column in a subsequent migration.

---

### PGM010 — DROP COLUMN silently removes unique constraint
{: #pgm010}

**Severity**: Minor

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in a UNIQUE constraint or unique index. PostgreSQL automatically drops dependent constraints, silently removing uniqueness guarantees.

**Example** (bad):
```sql
-- Table has UNIQUE(email)
ALTER TABLE users DROP COLUMN email;
-- The unique constraint is silently removed.
```

**Fix**: Verify that the uniqueness guarantee is no longer needed before dropping the column.

See also [PGM011](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm011), [PGM012](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm012), [PGM025](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm025).

---

### PGM011 — DROP COLUMN silently removes primary key
{: #pgm011}

**Severity**: Major

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in the table's primary key. The table loses its row identity, affecting replication, ORMs, query planning, and data integrity.

**Example** (bad):
```sql
-- Table has PRIMARY KEY (id)
ALTER TABLE orders DROP COLUMN id;
-- The primary key is silently removed.
```

**Fix**: Add a new primary key on remaining columns before or after dropping the column.

See also [PGM010](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm010), [PGM012](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm012), [PGM025](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm025).

---

### PGM012 — DROP COLUMN silently removes foreign key
{: #pgm012}

**Severity**: Minor

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in a FOREIGN KEY constraint. The referential integrity guarantee is silently lost, potentially allowing orphaned rows.

**Example** (bad):
```sql
-- Table has FOREIGN KEY (customer_id) REFERENCES customers(id)
ALTER TABLE orders DROP COLUMN customer_id;
-- The foreign key constraint is silently removed.
```

**Fix**: Verify that the referential integrity guarantee is no longer needed before dropping the column.

See also [PGM010](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm010), [PGM011](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm011), [PGM025](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm025).

---

### PGM013 — SET NOT NULL on existing table requires ACCESS EXCLUSIVE lock
{: #pgm013}

**Severity**: Critical

Detects `ALTER TABLE ... ALTER COLUMN ... SET NOT NULL` on a pre-existing table. This acquires an ACCESS EXCLUSIVE lock and performs a full table scan to verify no existing rows contain NULL. PostgreSQL 12+ skips the scan when a valid CHECK constraint already proves the column holds no NULLs. The rule stays silent only when such a CHECK is exactly `col IS NOT NULL`.

**Example** (bad):
```sql
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
```

**Fix** (safe pattern, PostgreSQL 12+):
```sql
-- Migration 1: add a CHECK constraint with NOT VALID (no scan)
ALTER TABLE orders ADD CONSTRAINT orders_status_nn
  CHECK (status IS NOT NULL) NOT VALID;
```

```sql
-- Migration 2: validate (scans under SHARE UPDATE EXCLUSIVE, concurrent reads and writes OK)
ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;
-- Set NOT NULL (no scan: the valid CHECK proves the column holds no NULLs)
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
-- Optional, as a statement of its own: drop the now-redundant CHECK
ALTER TABLE orders DROP CONSTRAINT orders_status_nn;
```

The rule checks each `SET NOT NULL` against the schema as the migration's earlier statements leave it, so the CHECK can come from an earlier migration or from earlier in the same one. The CHECK counts once it is valid (validated, or added without `NOT VALID`), and no longer once it is dropped, before the statement or by the statement itself. PostgreSQL runs every `DROP CONSTRAINT` of an `ALTER TABLE` before its `SET NOT NULL`, whatever the written order, so keep the optional `DROP CONSTRAINT` a statement of its own: written into the `SET NOT NULL` statement, it brings the scan back.

The rule recognizes only a CHECK that is exactly `status IS NOT NULL`. It does not recognize a CHECK that combines the test with other conditions, even though PostgreSQL may use it to skip the scan, and still reports the `SET NOT NULL`.

Commit the `ADD CONSTRAINT` before `VALIDATE CONSTRAINT` runs, which is why it has a migration of its own above. `ADD CONSTRAINT ... CHECK` takes an ACCESS EXCLUSIVE lock, with or without `NOT VALID`, and a transaction holds its locks until it ends. When the migration tool runs all three statements in one transaction, the validation scan therefore runs with reads and writes blocked. This rule looks at the `SET NOT NULL` only, so it does not report that.

See also [PGM015](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm015).

---

### PGM014 — ADD FOREIGN KEY on existing table without NOT VALID
{: #pgm014}

**Severity**: Critical

Detects `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY` on a pre-existing table without the `NOT VALID` modifier. Without `NOT VALID`, PostgreSQL immediately validates all existing rows under a SHARE ROW EXCLUSIVE lock on both the referencing and the referenced table.

**Example** (bad):
```sql
ALTER TABLE orders
  ADD CONSTRAINT fk_customer
  FOREIGN KEY (customer_id) REFERENCES customers (id);
```

**Fix** (safe pattern):
```sql
ALTER TABLE orders
  ADD CONSTRAINT fk_customer
  FOREIGN KEY (customer_id) REFERENCES customers (id)
  NOT VALID;
ALTER TABLE orders VALIDATE CONSTRAINT fk_customer;
```

See also [PGM015](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm015).

---

### PGM015 — ADD CHECK on existing table without NOT VALID
{: #pgm015}

**Severity**: Critical

Detects `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)` on a pre-existing table without `NOT VALID`. Without `NOT VALID`, PostgreSQL acquires a SHARE ROW EXCLUSIVE lock and scans the entire table to verify all existing rows, blocking concurrent writes for the duration.

**Example** (bad):
```sql
ALTER TABLE orders ADD CONSTRAINT orders_status_check
  CHECK (status IN ('pending', 'shipped', 'delivered'));
```

**Fix** (safe two-step pattern):
```sql
-- Step 1: Add with NOT VALID (instant, no scan)
ALTER TABLE orders ADD CONSTRAINT orders_status_check
  CHECK (status IN ('pending', 'shipped', 'delivered')) NOT VALID;
-- Step 2: Validate (SHARE UPDATE EXCLUSIVE lock, concurrent reads OK)
ALTER TABLE orders VALIDATE CONSTRAINT orders_status_check;
```

See also [PGM013](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm013), [PGM014](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm014).

---

### PGM016 — ADD PRIMARY KEY on existing table without USING INDEX
{: #pgm016}

**Severity**: Major

Detects `ALTER TABLE ... ADD PRIMARY KEY` on an existing table that doesn't use `USING INDEX`. Without `USING INDEX`, PostgreSQL builds a new index under ACCESS EXCLUSIVE lock, even if a matching unique index already exists.

Additionally, even with `USING INDEX`, if any PK columns are nullable, PostgreSQL implicitly runs `SET NOT NULL` under ACCESS EXCLUSIVE lock.

**Example** (bad):
```sql
ALTER TABLE orders ADD PRIMARY KEY (id);
```

**Fix** (safe pattern):
```sql
CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);
ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;
```

If the PK columns are nullable, `USING INDEX` alone is not enough — PostgreSQL still runs an implicit `SET NOT NULL` (full table scan under ACCESS EXCLUSIVE). Make columns NOT NULL first using the safe CHECK-constraint pattern from [PGM013](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm013):

```sql
-- Step 1: Make column NOT NULL safely (see PGM013)
ALTER TABLE orders ADD CONSTRAINT orders_id_nn
  CHECK (id IS NOT NULL) NOT VALID;
ALTER TABLE orders VALIDATE CONSTRAINT orders_id_nn;
ALTER TABLE orders ALTER COLUMN id SET NOT NULL;
ALTER TABLE orders DROP CONSTRAINT orders_id_nn;
-- Step 2: Now USING INDEX is truly instant
CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);
ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;
```

The rule checks `USING INDEX` against the schema as the migration's earlier statements leave it, so the index can be built, and the columns made NOT NULL, earlier in the same migration.

---

### PGM017 — ADD UNIQUE on existing table without USING INDEX
{: #pgm017}

**Severity**: Critical

Detects `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE` on an existing table without `USING INDEX`. Without `USING INDEX`, PostgreSQL builds a new unique index under ACCESS EXCLUSIVE lock. `NOT VALID` does **not** apply to UNIQUE constraints.

**Example** (bad):
```sql
ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE (email);
```

**Fix** (safe pattern):
```sql
CREATE UNIQUE INDEX CONCURRENTLY idx_orders_email ON orders (email);
ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_orders_email;
```

The rule checks `USING INDEX` against the schema as the migration's earlier statements leave it, so the index can be built earlier in the same migration, as above.

See also [PGM016](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm016).

---

### PGM018 — CLUSTER on existing table
{: #pgm018}

**Severity**: Critical

Detects a `CLUSTER` statement targeting a table that already exists in the database. `CLUSTER` rewrites the entire table and all its indexes in a new physical order, holding an ACCESS EXCLUSIVE lock for the full duration. Unlike `VACUUM FULL`, there is no online alternative. On large tables this causes complete unavailability for minutes to hours.

**Example**:
```sql
CLUSTER orders USING idx_orders_created_at;
```

**Recommended approach**:
1. Schedule `CLUSTER` during a maintenance window when downtime is acceptable.
2. Consider `pg_repack` or `pg_squeeze` for online table rewrites.
3. For new tables, `CLUSTER` is fine — this rule only fires on existing tables.

---

### PGM019 — ADD EXCLUDE constraint on existing table
{: #pgm019}

**Severity**: Critical

Detects `ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE (...)` on a table that already exists. Adding an EXCLUDE constraint acquires an ACCESS EXCLUSIVE lock and scans all existing rows to verify the exclusion condition. Unlike CHECK and FOREIGN KEY constraints, PostgreSQL does not support `NOT VALID` for EXCLUDE constraints — there is no safe online path.

**Example** (bad):
```sql
ALTER TABLE reservations
  ADD CONSTRAINT excl_overlap
  EXCLUDE USING gist (room WITH =, period WITH &&);
```

**Recommended approach**:
1. Schedule the migration during a maintenance window when downtime is acceptable.
2. For new tables, adding EXCLUDE constraints in `CREATE TABLE` is fine — this rule only fires on existing tables.

---

### PGM020 — DISABLE TRIGGER on table suppresses FK enforcement
{: #pgm020}

**Severity**: Minor

Detects `ALTER TABLE ... DISABLE TRIGGER` (specific name, ALL, or USER) on any table. Fires at MINOR on existing tables and at INFO on all other tables (new or unknown). Since re-enables are not tracked, all tables are flagged to catch cases where triggers may be left disabled.

**Example** (bad):
```sql
ALTER TABLE orders DISABLE TRIGGER ALL;
INSERT INTO orders SELECT * FROM staging;
```

**Fix** (re-enable in the same migration):
```sql
ALTER TABLE orders DISABLE TRIGGER ALL;
INSERT INTO orders SELECT * FROM staging;
ALTER TABLE orders ENABLE TRIGGER ALL;
```

**Recommended approach**:
1. Avoid disabling triggers in migrations entirely.
2. If you must disable triggers for bulk data loading, ensure the DISABLE and ENABLE are in the same migration and wrapped in a transaction.
3. On tables that are not pre-existing (new or unknown), this rule fires at INFO severity.

---

### PGM021 — VACUUM FULL on existing table
{: #pgm021}

**Severity**: Critical

Detects `VACUUM FULL` targeting an existing table. `VACUUM FULL` rewrites the entire table into a new data file under an ACCESS EXCLUSIVE lock, blocking all reads and writes for the duration. On large tables this means minutes to hours of downtime.

**Example** (flagged):
```sql
VACUUM FULL orders;
```

**Fix** (use an online compaction tool):
```text
pg_repack --table orders --no-superuser-check -d mydb
```

Or schedule during a maintenance window when downtime is acceptable.

---

### PGM022 — Missing CONCURRENTLY on REINDEX
{: #pgm022}

**Severity**: Critical

Detects `REINDEX TABLE`, `REINDEX INDEX`, `REINDEX SCHEMA`, `REINDEX DATABASE`, or `REINDEX SYSTEM` without the `CONCURRENTLY` option. Without `CONCURRENTLY`, `REINDEX` acquires an ACCESS EXCLUSIVE lock on the target table (or parent table for `REINDEX INDEX`), blocking all reads and writes for the duration of the rebuild.

**Example** (bad):
```sql
REINDEX TABLE orders;
REINDEX INDEX idx_orders_status;
```

**Fix**:
```sql
REINDEX TABLE CONCURRENTLY orders;
REINDEX INDEX CONCURRENTLY idx_orders_status;
```

The `CONCURRENTLY` option (PostgreSQL 12+) rebuilds the index without holding an exclusive lock for the entire operation. It takes longer but allows normal reads and writes to continue.

See also [PGM003](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm003).

---

### PGM023 — Multiple ALTER TABLE statements on the same table can be combined
{: #pgm023}

**Severity**: Minor

Detects multiple separate `ALTER TABLE` statements targeting the same table within a single migration file, where all statements operate at the same lock level. Each separate statement acquires and releases the table lock independently, increasing the total lock contention window unnecessarily.

**Example** (bad — two lock acquisitions):
```sql
ALTER TABLE authors ALTER COLUMN name SET NOT NULL;
ALTER TABLE authors ALTER COLUMN email SET NOT NULL;
```

**Fix** (one lock acquisition):
```sql
ALTER TABLE authors
  ALTER COLUMN name SET NOT NULL,
  ALTER COLUMN email SET NOT NULL;
```

Chains are broken by any intervening statement that references the same table (e.g., `CREATE INDEX CONCURRENTLY`). Statements with different lock levels (e.g., `VALIDATE CONSTRAINT` vs `SET NOT NULL`) are tracked independently and do not trigger this rule against each other.

`ATTACH PARTITION` and `DETACH PARTITION` are never reported by this rule. PostgreSQL's grammar makes `partition_cmd` occupy the entire `ALTER TABLE` statement, so these actions can never be combined with anything else, and "combine them" is not applicable advice for this pair.

Tables created within the same set of changed files are exempt — lock contention for brand-new tables is harmless.

---

### PGM024 — DROP TABLE or CREATE TABLE PARTITION OF locks the parent partitioned table
{: #pgm024}

**Severity**: Critical

Detects two statement shapes that both take ACCESS EXCLUSIVE on a pre-existing partitioned parent table: dropping a live partition child with `DROP TABLE`, and creating a new child directly with `CREATE TABLE ... PARTITION OF`. Both lock the parent, not just the child, and hold that lock until commit — blocking every reader and writer routed through the parent, and therefore every sibling partition, for the duration. A reader or writer already holding the parent does not make either statement fail outright: it queues behind them, and a merely queued lock request already blocks new readers the current holder would have allowed. `DROP TABLE` additionally destroys the child's data irreversibly and has no `CONCURRENTLY` variant at all.

For `DROP TABLE`, the safe alternative is `DETACH PARTITION ... CONCURRENTLY` (PostgreSQL 14+) first, then dropping the now-standalone table — see PGM004.

For `CREATE TABLE ... PARTITION OF`, there is no `CONCURRENTLY` form. Instead, create the table standalone and `ATTACH PARTITION` it: `ATTACH` takes only `SHARE UPDATE EXCLUSIVE` on the parent, plus a brief `ACCESS EXCLUSIVE` on the new table itself, which nothing is using yet and so is uncontended. See PGM005 for the `CHECK` constraint that lets the attach skip a full scan of the child.

When one transaction locks several parents this way, [PGM026](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm026) reports that as well: each parent stays locked while the transaction waits for the next.

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

---

### PGM025 — DROP COLUMN silently removes EXCLUDE constraint
{: #pgm025}

**Severity**: Minor

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in an EXCLUDE constraint. PostgreSQL automatically drops the whole EXCLUDE constraint, silently removing the exclusion guarantee.

**Example** (bad):
```sql
-- Table has EXCLUDE USING gist (room WITH =, period WITH &&)
ALTER TABLE bookings DROP COLUMN room;
-- The EXCLUDE constraint is silently removed.
```

**Fix**: Verify that the exclusion guarantee is no longer needed before dropping the column.

See also [PGM010](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm010), [PGM011](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm011), [PGM012](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm012).

---

### PGM026 — One transaction locks several partitioned parent tables
{: #pgm026}

**Severity**: Critical

Detects one transaction that takes ACCESS EXCLUSIVE on two or more partitioned parent tables that existed before the change. `DROP TABLE` of a partition and `CREATE TABLE ... PARTITION OF` each lock the partition's parent ([PGM024](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm024)), as does a plain `ALTER TABLE ... DETACH PARTITION` ([PGM004](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm004)), and a transaction holds every lock it takes until it ends. Once the transaction has locked the first parent, reads and writes routed through it wait, and they go on waiting while the transaction queues for the next parent's lock behind the traffic already running there. Each further parent adds its wait to the time the earlier ones stay blocked. If one of those locks cannot be taken, because a `lock_timeout` expires or the transaction deadlocks with application traffic that touches two of the parents, the transaction aborts: the work on every parent rolls back, after the earlier parents were blocked for nothing.

The rule looks at each transaction of the migration on its own: the whole file when the migration tool runs it in a transaction, a `BEGIN ... COMMIT` block, or one statement outside a transaction block. A `COMMIT` in a file the tool runs in a transaction ends the tool's transaction, and the statements after it run in a new one, which the rule counts on its own. `DROP TABLE a, b` is one statement, and so one transaction, that locks the parents of both `a` and `b`: it holds the lock on `a`'s parent while it waits for `b`'s. The rule fires once per transaction that locks two or more parents, at the first statement that locks one, and names them all. golang-migrate's default mode runs a file of several statements as one implicit transaction, which `run_in_transaction = true` models; with `false`, the rule counts each statement on its own and misses two parents locked in such a file.

`ATTACH PARTITION` and `DETACH PARTITION ... CONCURRENTLY` do not count. They take only SHARE UPDATE EXCLUSIVE on the parent, which lets reads and writes go on, and `DETACH ... CONCURRENTLY` cannot run inside a transaction block at all. Dropping a partition that the migration detached earlier locks no parent either, as the table is no longer a partition, but a plain `DETACH` before the drop has already locked the parent and counts. A parent created in the same change does not count, as nothing uses it yet.

PGM024 flags the same `DROP TABLE` and `CREATE TABLE ... PARTITION OF` statements (one finding per parent in the unit), and PGM004 each plain `DETACH PARTITION`: they say that each parent gets locked, this rule that one transaction holds several of those locks at once. Their fixes (detach the partition concurrently, then drop it; create the new table standalone, then attach it) avoid the ACCESS EXCLUSIVE lock and so clear this rule as well. Where the ACCESS EXCLUSIVE locks are acceptable, change one parent per transaction, so that each lock is held only for that parent's own work. A separate migration per parent is the safest way to get there. A `COMMIT` inside a migration the tool runs in a transaction does split it, but if a statement after the `COMMIT` fails, the migration is left half applied.

**Example** (bad):
```sql
BEGIN;
DROP TABLE measurements_2023;
CREATE TABLE readings_2026 PARTITION OF readings
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
COMMIT;
```

**Fix**: one migration per parent:
```sql
-- V010__drop_measurements_2023.sql
DROP TABLE measurements_2023;

-- V011__create_readings_2026.sql
CREATE TABLE readings_2026 PARTITION OF readings
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
```

---

### PGM027 — Statement fails against the schema from earlier migrations
{: #pgm027}

**Severity**: Major

Detects a statement that PostgreSQL will reject, given the schema built from the earlier migrations and the earlier statements of the same migration. The deploy breaks at that statement. Inside a transaction the whole migration rolls back; outside one, the statements before it stay applied and the migration is left half-done, so a rerun starts from a state nobody planned for.

Each statement is checked against the schema as it stands just before it, so a name freed or taken earlier in the same migration counts. The rule fires only on positive evidence: an object the migration history does not show may exist outside it, so a missing table, index or constraint is never reported.

Down migrations (`.down.sql`, `_down.sql`) are not checked. A down file usually sorts before its up file, so the schema it would be checked against is the one before the up migration ran, not the one it undoes. PGM002, PGM003, PGM401, PGM402, PGM501 and PGM504 report a down migration's statements as usual.

It currently detects:

- `ADD PRIMARY KEY` or `ADD UNIQUE ... USING INDEX idx` where `idx` belongs to another table. PostgreSQL looks the index up in the altered table's schema and requires it to be one of that table's indexes.
- Renaming a table, index or constraint onto a name that is taken. A table or index needs the name free among the schema's tables and indexes. A constraint needs it free among its table's constraints. Renaming a PRIMARY KEY, UNIQUE or EXCLUDE constraint renames its index too, and the other way round, so those need both.
- `DROP INDEX` of the index behind a PRIMARY KEY or UNIQUE constraint. PostgreSQL refuses with or without `CONCURRENTLY`, `IF EXISTS` or `CASCADE`; the constraint has to be dropped instead.
- `CREATE INDEX idx`, with or without `CONCURRENTLY`, where a table or index in the indexed table's schema already has the name `idx`, the index behind a PRIMARY KEY, UNIQUE or EXCLUDE constraint included. Index names share one namespace with tables, so PostgreSQL fails the statement and builds nothing. An unnamed `CREATE INDEX` gets a generated name that is free, and `CREATE INDEX IF NOT EXISTS` skips the statement instead of failing, so neither is reported. Sequences, views and the indexes PostgreSQL creates on partitions take names too, but the schema built from the migration history does not track them, so a clash with one of those goes unreported. The other way round, a drop or rename that the schema misses, such as `ALTER TABLE` used to rename an index or a `DROP INDEX` inside a `DO` block or a statement the parser cannot read, leaves the old name looking taken, so a later `CREATE INDEX` reusing it is reported though it succeeds.

**Example** (bad):
```sql
-- V001
CREATE TABLE customers (id bigint PRIMARY KEY, email text, created_at timestamptz);
CREATE UNIQUE INDEX customers_email_idx ON customers (email);
CREATE INDEX idx_created_at ON customers (created_at);
CREATE TABLE orders (id bigint PRIMARY KEY, email text, created_at timestamptz);
CREATE TABLE archive (id bigint PRIMARY KEY);

-- V002: each statement fails
ALTER TABLE orders ADD CONSTRAINT orders_email_key UNIQUE USING INDEX customers_email_idx;
ALTER TABLE archive RENAME TO orders;
DROP INDEX orders_pkey;
CREATE INDEX idx_created_at ON orders (created_at);
```

**Fix**: build the index on the table the constraint is for, pick a free name (or drop or rename the object that holds it first), and remove a constraint's index with `ALTER TABLE ... DROP CONSTRAINT`:
```sql
CREATE UNIQUE INDEX CONCURRENTLY orders_email_idx ON orders (email);
ALTER TABLE orders ADD CONSTRAINT orders_email_key UNIQUE USING INDEX orders_email_idx;
ALTER TABLE archive RENAME TO orders_archive;
ALTER TABLE orders DROP CONSTRAINT orders_pkey;
CREATE INDEX CONCURRENTLY orders_created_at_idx ON orders (created_at);
```

A statement this rule reports is not reported by [PGM002](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm002), [PGM016](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm016), [PGM017](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm017), [PGM107](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm107), [PGM401](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm401), [PGM402](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm402), [PGM501](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm501), [PGM504](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm504) or [PGM508](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm508), since it never takes effect. [PGM003](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm003) leaves a `DROP INDEX CONCURRENTLY` this rule reports alone, as its fix needs no `CONCURRENTLY`. A `CREATE INDEX` onto a taken name is still reported by [PGM001](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm001) on an existing table, and by [PGM003](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm003) with `CONCURRENTLY` inside a transaction: once it has a free name, it still needs `CONCURRENTLY` on an existing table, and `CONCURRENTLY` still cannot run inside a transaction.

---

## 1xx — Type Anti-pattern Rules

These rules flag column types that should be avoided per the PostgreSQL wiki's ["Don't Do This"](https://wiki.postgresql.org/wiki/Don't_Do_This) recommendations.

### PGM101 — Column uses timestamp without time zone
{: #pgm101}

**Severity**: Minor

Detects columns declared as `timestamp` (which PostgreSQL interprets as `timestamp without time zone`). This type stores no timezone context, making values ambiguous across environments.

**Example** (bad):
```sql
CREATE TABLE events (created_at timestamp NOT NULL);
```

**Fix**:
```sql
CREATE TABLE events (created_at timestamptz NOT NULL);
```

---

### PGM102 — Column uses timestamp or timestamptz with precision 0
{: #pgm102}

**Severity**: Minor

Detects timestamp columns with precision 0. Precision 0 causes **rounding**, not truncation — a value of `'23:59:59.9'` rounds to the next day.

**Example** (bad):
```sql
CREATE TABLE events (created_at timestamptz(0));
```

**Fix**:
```sql
CREATE TABLE events (created_at timestamptz);
```

---

### PGM103 — Column uses char(n) type
{: #pgm103}

**Severity**: Minor

Detects columns declared as `char(n)` or `character(n)`. In PostgreSQL, `char(n)` pads with trailing spaces, wastes storage, and is no faster than `text` or `varchar`.

**Example** (bad):
```sql
CREATE TABLE countries (code char(2) NOT NULL);
```

**Fix**:
```sql
CREATE TABLE countries (code text NOT NULL);
-- or: code varchar(2) NOT NULL
```

---

### PGM104 — Column uses the money type
{: #pgm104}

**Severity**: Minor

Detects columns declared as `money`. The `money` type formats output according to the `lc_monetary` locale setting, making it unreliable across environments and causing data corruption when moving data between servers.

**Example** (bad):
```sql
CREATE TABLE orders (total money NOT NULL);
```

**Fix**:
```sql
CREATE TABLE orders (total numeric(12,2) NOT NULL);
```

---

### PGM105 — Column uses serial/bigserial instead of identity column
{: #pgm105}

**Severity**: Info

Detects columns declared as `serial`, `bigserial`, or `smallserial`. Since PostgreSQL 10, identity columns (`GENERATED ALWAYS AS IDENTITY`) provide the same auto-incrementing behavior with tighter ownership, better permission handling, and SQL standard compliance.

**Example** (flagged):
```sql
CREATE TABLE orders (id serial PRIMARY KEY);
```

**Fix**:
```sql
CREATE TABLE orders (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY
);
```

---

### PGM106 — Column uses json type instead of jsonb
{: #pgm106}

**Severity**: Minor

Detects columns declared as `json`. The `json` type stores exact input text and re-parses on every operation. `jsonb` stores a decomposed binary format that is faster, smaller, indexable (GIN), and supports containment operators.

**Example** (bad):
```sql
CREATE TABLE events (payload json NOT NULL);
```

**Fix**:
```sql
CREATE TABLE events (payload jsonb NOT NULL);
```

---

### PGM107 — Primary key column uses integer or smallint instead of bigint
{: #pgm107}

**Severity**: Major

Detects primary key columns that use `integer` (`int4`) or `smallint` (`int2`) instead of `bigint` (`int8`). High-write tables routinely exhaust the ~2.1 billion (`integer`) or ~32 000 (`smallint`) limit. Migrating to `bigint` later requires an ACCESS EXCLUSIVE lock and full table rewrite.

**Example** (flagged):
```sql
CREATE TABLE orders (id integer PRIMARY KEY);
```

**Fix**:
```sql
CREATE TABLE orders (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY
);
```

`ALTER TABLE ... ADD PRIMARY KEY` reads the column types as they stand just before the statement, so a column added or retyped earlier in the same migration counts with its new type.

---

### PGM108 — Column uses varchar(n) instead of text
{: #pgm108}

**Severity**: Info

Detects columns declared as `varchar(n)` or `character varying(n)`. In PostgreSQL, `varchar(n)` has zero performance benefit over `text` — they share identical `varlena` storage. The length constraint adds an artificial limit that may require future schema changes.

**Example** (bad):
```sql
CREATE TABLE users (name varchar(100) NOT NULL);
```

**Fix**:
```sql
CREATE TABLE users (name text NOT NULL);
-- If validation is needed, use a CHECK constraint:
-- ALTER TABLE users ADD CONSTRAINT chk_name_len CHECK (length(name) <= 100) NOT VALID;
-- ALTER TABLE users VALIDATE CONSTRAINT chk_name_len;
```

---

### PGM109 — Column uses floating-point type instead of numeric
{: #pgm109}

**Severity**: Minor

Detects columns declared as `real` (`float4`), `double precision` (`float8`), or `float`. IEEE 754 floating-point types suffer from precision issues — for example, `0.1 + 0.2 ≠ 0.3`. For money, quantities, measurements, or any domain where exact decimal values matter, `numeric`/`decimal` is the correct choice.

**Example** (bad):
```sql
CREATE TABLE products (price double precision NOT NULL);
```

**Fix**:
```sql
CREATE TABLE products (price numeric(10,2) NOT NULL);
```

---

## 2xx — Destructive Operation Rules

### PGM201 — DROP TABLE on existing table
{: #pgm201}

**Severity**: Minor

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

`DETACH PARTITION ... CONCURRENTLY` cannot run inside a transaction block, and it still waits for traffic on the parent — it can fail with `55P03` too. An interrupted detach leaves the partition pending detach, where `DROP TABLE` still takes ACCESS EXCLUSIVE on the parent. See also [PGM004](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm004).

---

### PGM202 — DROP TABLE CASCADE on existing table
{: #pgm202}

**Severity**: Major

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

---

### PGM203 — TRUNCATE TABLE on existing table
{: #pgm203}

**Severity**: Minor

Detects `TRUNCATE TABLE` targeting a pre-existing table. Unlike `DELETE`, `TRUNCATE` does not fire `ON DELETE` triggers, does not log individual row deletions, and cannot be filtered with a `WHERE` clause. The operation is irreversible once committed.

**Example**:
```sql
TRUNCATE TABLE audit_trail;
```

**Recommended approach**:
1. Ensure the data is truly disposable or has been backed up.
2. Consider whether `ON DELETE` triggers need to fire — if so, use `DELETE`.
3. If truncating for a schema migration, document the intent clearly.

---

### PGM204 — TRUNCATE TABLE CASCADE on existing table
{: #pgm204}

**Severity**: Major

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

---

### PGM205 — DROP SCHEMA CASCADE
{: #pgm205}

**Severity**: Critical

Detects `DROP SCHEMA ... CASCADE`. This is the most destructive single DDL statement in PostgreSQL — it silently drops every object in the schema: tables, views, sequences, functions, types, and indexes.

Unlike `DROP TABLE CASCADE` (which only removes objects that depend on one table), `DROP SCHEMA CASCADE` destroys the entire namespace and everything in it.

**Example**:
```sql
DROP SCHEMA myschema CASCADE;
-- Silently drops every table, view, function, sequence, and type in 'myschema'.
```

**Recommended approach**:
1. Enumerate all objects in the schema before dropping.
2. Explicitly drop or migrate each object in separate migration steps.
3. Use plain `DROP SCHEMA` (without `CASCADE`) so PostgreSQL will error if the schema is non-empty.

---

## 3xx — DML in Migration Rules

### PGM301 — INSERT INTO existing table in migration
{: #pgm301}

**Severity**: Info

Detects `INSERT INTO` targeting a table that already exists in the database (not created in the same migration file). While seed data and lookup table population are valid use cases, they deserve review because large inserts can cause lock contention, WAL pressure, and timeouts.

**Example** (flagged):
```sql
INSERT INTO config (key, value) VALUES ('feature_x', 'enabled');
```

Not flagged when inserting into a table created in the same migration file.

---

### PGM302 — UPDATE on existing table in migration
{: #pgm302}

**Severity**: Minor

Detects `UPDATE` targeting a table that already exists in the database. On large tables, updates hold row locks for the full statement duration, generate WAL (spiking replication lag), and may time out under migration tool limits.

**Example** (flagged):
```sql
UPDATE orders SET status = 'pending' WHERE status IS NULL;
```

**Recommended approach**:
1. Verify the row count is bounded (small lookup table = fine).
2. For large tables, batch the update in chunks.
3. Consider running the update outside the migration transaction.

---

### PGM303 — DELETE FROM existing table in migration
{: #pgm303}

**Severity**: Minor

Detects `DELETE FROM` targeting a table that already exists in the database. On large tables, deletes hold row locks, generate WAL, fire `ON DELETE` triggers, and produce dead tuples until autovacuum runs.

**Example** (flagged):
```sql
DELETE FROM audit_log WHERE created_at < '2020-01-01';
```

**Recommended approach**:
1. Verify the row count is bounded.
2. For large deletes, batch in chunks.
3. If no triggers need to fire, consider `TRUNCATE` instead.

---

## 4xx — Idempotency Guard Rules

### PGM401 — Missing IF EXISTS on DROP TABLE / DROP INDEX
{: #pgm401}

**Severity**: Minor

Detects `DROP TABLE` or `DROP INDEX` without the `IF EXISTS` clause. Without `IF EXISTS`, the statement fails if the object does not exist, causing hard failures in migration pipelines that may be re-run.

**Example** (bad):
```sql
DROP TABLE orders;
DROP INDEX idx_orders_status;
```

**Fix**:
```sql
DROP TABLE IF EXISTS orders;
DROP INDEX IF EXISTS idx_orders_status;
```

Does not fire on a `DROP INDEX` that names the index behind a PRIMARY KEY or UNIQUE constraint. PostgreSQL rejects it with or without `IF EXISTS`; [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports it.

---

### PGM402 — Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX
{: #pgm402}

**Severity**: Minor

Detects `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause. Without `IF NOT EXISTS`, the statement fails if the object already exists, causing hard failures in migration pipelines that may be re-run.

An unnamed `CREATE INDEX` is not reported. PostgreSQL gives it a generated name that is free in the schema, so it cannot fail on an existing index, and `IF NOT EXISTS` requires a name.

Nor is a `CREATE INDEX` whose name a table or index in the schema already has. PostgreSQL rejects it, and `IF NOT EXISTS` would only turn that into a silent no-op that builds no index; [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports it.

**Example** (bad):
```sql
CREATE TABLE orders (id bigint PRIMARY KEY);
CREATE INDEX idx_orders_status ON orders (status);
```

**Fix**:
```sql
CREATE TABLE IF NOT EXISTS orders (id bigint PRIMARY KEY);
CREATE INDEX IF NOT EXISTS idx_orders_status ON orders (status);
```

See also [PGM401](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm401).

---

### PGM403 — CREATE TABLE IF NOT EXISTS for already-existing table is a misleading no-op
{: #pgm403}

**Severity**: Minor

Detects `CREATE TABLE IF NOT EXISTS` targeting a table that already exists in the migration history. `IF NOT EXISTS` makes the statement a silent no-op — if the column definitions differ from the actual table state, the migration author may believe the table has the shape described in this statement, when in reality PostgreSQL ignores it entirely.

**Example** (bad):
```sql
-- V001: original table
CREATE TABLE orders (id bigint PRIMARY KEY);
ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';

-- V010: redundant re-creation (silently ignored)
CREATE TABLE IF NOT EXISTS orders (
  id bigint PRIMARY KEY,
  status text NOT NULL DEFAULT 'pending',
  created_at timestamptz DEFAULT now()  -- this column will NOT be added
);
```

**Fix**: Remove the redundant `CREATE TABLE IF NOT EXISTS`. If the intent is to add columns, use `ALTER TABLE ... ADD COLUMN` instead.

---

## 5xx — Schema Design Rules

### PGM501 — Foreign key without covering index on referencing columns
{: #pgm501}

**Severity**: Major

Detects foreign key constraints where the referencing table has no usable index containing any of the FK columns. Without such an index, any query joining or filtering on those columns forces a sequential scan on the referencing table — and so does a referential integrity check when a referenced row is deleted or its key columns are updated.

**Example** (bad):
```sql
ALTER TABLE order_items
  ADD CONSTRAINT fk_order
  FOREIGN KEY (order_id) REFERENCES orders(id);
-- No index on order_items(order_id)
```

**Fix**:
```sql
CREATE INDEX idx_order_items_order_id ON order_items (order_id);
ALTER TABLE order_items
  ADD CONSTRAINT fk_order
  FOREIGN KEY (order_id) REFERENCES orders(id);
```

**Dropping the covering index**: the rule also fires when a migration removes the last usable index covering a foreign key that already exists and stays. That happens with `DROP INDEX` (with or without `CONCURRENTLY`), with `ALTER TABLE ... DROP CONSTRAINT` of a PRIMARY KEY or UNIQUE constraint, whose index goes with it, and with `ALTER TABLE ... DROP COLUMN` of any column of the index, even one the foreign key does not use. A partition's foreign key covered by its parent's index loses that coverage when the parent's index is dropped.

**Example** (bad):
```sql
-- order_items has FOREIGN KEY (order_id) REFERENCES orders(id),
-- covered only by idx_order_items_order_id.
DROP INDEX CONCURRENTLY idx_order_items_order_id;
```

It does not fire when the same file drops the foreign key too, when another usable index still covers it, or when the same file adds the foreign key, which the first check already reports once. Nor does it fire on a statement PostgreSQL rejects, such as a `DROP INDEX` that also names the index behind a PRIMARY KEY or UNIQUE constraint: nothing is dropped, and [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports it. When several statements in a file drop covering indexes of one foreign key, the finding goes to the last of them.

**Column matching**: FK columns `(a, b)` are covered by any usable index that contains at least one of `a` or `b`, in any position — e.g. `(a, b)`, `(b, a)`, `(a)`, or `(c, b)` all count. An index covering only some of the FK columns still avoids a sequential scan (via a Filter or Recheck), so it counts as coverage even though a fully covering index performs better. Column order does not matter here. The check uses the catalog state after the entire file is processed, so creating the index later in the same file avoids a false positive. A `CREATE INDEX` whose name a table or index in the schema already has creates nothing, as PostgreSQL rejects it ([PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027)), so it does not count.

**Known limitation**: an FK whose only indexed column is low-cardinality (e.g. 10 distinct values over 1M rows) produces no warning, even though the check still degrades to a Bitmap Heap Scan touching a large fraction of the table. Index shape alone can't tell a selective index from an unselective one — the catalog has no column statistics, and requiring the index to also be UNIQUE would not help, since the false-negative case is not unique in the referencing table either.

---

### PGM502 — Table without primary key
{: #pgm502}

**Severity**: Major

Detects `CREATE TABLE` (non-temporary) that results in a table without a primary key after the entire file is processed.

**Example** (bad):
```sql
CREATE TABLE events (event_type text, payload jsonb);
```

**Fix**:
```sql
CREATE TABLE events (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  event_type text,
  payload jsonb
);
```

Temporary tables are excluded. When [PGM503](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm503) fires (UNIQUE NOT NULL substitute detected), PGM502 does not fire for the same table.

---

### PGM503 — UNIQUE NOT NULL used instead of PRIMARY KEY
{: #pgm503}

**Severity**: Info

Detects tables that have no primary key but have at least one UNIQUE constraint where all constituent columns are NOT NULL. This is functionally equivalent to a PK but less conventional.

**Example** (flagged):
```sql
CREATE TABLE users (
  email text NOT NULL UNIQUE,
  name text
);
```

**Fix**:
```sql
CREATE TABLE users (
  email text PRIMARY KEY,
  name text
);
```

When PGM503 fires, [PGM502](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm502) does not fire for the same table.

---

### PGM504 — RENAME TABLE on existing table
{: #pgm504}

**Severity**: Info

Detects `ALTER TABLE ... RENAME TO` on a pre-existing table. Renaming breaks all queries, views, and functions referencing the old name. The rename itself is instant DDL (metadata-only), but downstream breakage can be severe.

**Example** (bad):
```sql
ALTER TABLE orders RENAME TO orders_archive;
-- All queries referencing 'orders' will fail.
```

**Fix** (backward-compatible):
```sql
ALTER TABLE orders RENAME TO orders_v2;
CREATE VIEW orders AS SELECT * FROM orders_v2;
```

Does not fire when a replacement table with the old name is created in the same migration unit (safe swap pattern).

---

### PGM505 — RENAME COLUMN on existing table
{: #pgm505}

**Severity**: Info

Detects `ALTER TABLE ... RENAME COLUMN` on a pre-existing table. A column rename silently invalidates all queries, views, and application code referencing the old name.

**Example** (bad):
```sql
ALTER TABLE orders RENAME COLUMN status TO order_status;
-- All queries using 'status' will fail with 'column does not exist'
```

**Fix** (multi-step approach):
1. Add the new column.
2. Backfill data from the old column.
3. Update application code to use the new column.
4. Drop the old column.

---

### PGM506 — CREATE UNLOGGED TABLE
{: #pgm506}

**Severity**: Info

Detects `CREATE TABLE` with the `UNLOGGED` keyword. Unlogged tables skip the write-ahead log for better write performance, but data is **truncated after a crash** and the table is **not replicated** to standby servers.

**Example** (flagged):
```sql
CREATE UNLOGGED TABLE scratch_data (id int, payload text);
```

**When unlogged tables are appropriate**:
- Ephemeral staging/import data that can be re-derived.
- Materialized caches where the source of truth lives elsewhere.
- ETL scratch space within a batch job.

---

### PGM507 — DROP NOT NULL on existing table allows NULL values
{: #pgm507}

**Severity**: Info

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

---

### PGM508 — Duplicate or redundant index detected (prefix of another index)
{: #pgm508}

**Severity**: Info

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
- The new index's name is already taken by a relation in the table's schema, an index or a table alike. PostgreSQL creates nothing: it skips a `CREATE INDEX IF NOT EXISTS` with a notice and rejects any other, which [PGM027](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm027) reports. An unnamed index never has a taken name: when its generated name (`{table}_{columns}_idx`) is taken, PostgreSQL adds a numeric suffix (`orders_customer_id_idx1`), and the finding uses that name.
- A later statement of the file adds a constraint of the new index's name to the table, so the linter cannot tell which of the two indexes the statement created. PostgreSQL rejects that later statement.

**Fix**: Drop the redundant index:
```sql
DROP INDEX CONCURRENTLY idx_orders_cust_short;
```

---

### PGM509 — Mixed-case identifier or reserved word requires double-quoting
{: #pgm509}

**Severity**: Info

Detects table and column names that require perpetual double-quoting — either because they contain uppercase characters or because they match a PostgreSQL reserved word.

**Example** (flagged):
```sql
CREATE TABLE "User" ("Id" bigint, "order" text);
-- Every query must now use: SELECT "Id", "order" FROM "User";
```

**Why it matters**:
- Every query must use the exact case and double-quotes — forgetting them silently references a different (lowercased) identifier.
- IDE autocompletion and ORMs may generate incorrect SQL.
- `pg_dump` output becomes harder to read and modify.

**Does NOT fire when**:
- The identifier is all-lowercase and not a PostgreSQL reserved word.
- The identifier is a schema name or index name (only table and column names are checked).

**Fix**: Use a lowercase, non-reserved name:
```sql
CREATE TABLE users (id bigint, order_status text);
```

---

## 9xx — Meta-behavior Rules

### PGM901 — Down migration severity cap
{: #pgm901}

**Severity**: Info

Not a standalone lint rule. When a migration file is identified as a down migration, all findings from other rules are capped to INFO severity. Down migrations are informational only — they represent the undo path and are not expected to follow the same safety rules as forward migrations.

Detection is by filename suffix: the stem (filename minus `.sql` extension) must end with `.down` or `_down`. Examples:
- `000001_create_users.down.sql` — detected (go-migrate convention)
- `V001__create_users_down.sql` — detected (underscore convention)
- `downtown_orders.sql` — **not** detected (contains "down" but not as a suffix)

Liquibase changesets are never down migrations: the Liquibase loaders take only the SQL a changeset runs on `update`, so the SQL of `<rollback>` blocks is never linted.

This rule cannot be suppressed (it is applied automatically by the pipeline).

---

## Quick reference table

| Rule | Severity | Description |
|------|----------|-------------|
| [PGM001](#pgm001) | Critical | Missing CONCURRENTLY on CREATE INDEX |
| [PGM002](#pgm002) | Critical | Missing CONCURRENTLY on DROP INDEX |
| [PGM003](#pgm003) | Critical | CONCURRENTLY inside transaction |
| [PGM004](#pgm004) | Critical | DETACH PARTITION on existing table without CONCURRENTLY |
| [PGM005](#pgm005) | Major | ATTACH PARTITION of existing table without pre-validated CHECK |
| [PGM006](#pgm006) | Minor | Volatile default on column |
| [PGM007](#pgm007) | Critical | ALTER COLUMN TYPE on existing table causes table rewrite |
| [PGM008](#pgm008) | Critical | ADD COLUMN NOT NULL without DEFAULT on existing table |
| [PGM009](#pgm009) | Info | DROP COLUMN on existing table |
| [PGM010](#pgm010) | Minor | DROP COLUMN silently removes unique constraint |
| [PGM011](#pgm011) | Major | DROP COLUMN silently removes primary key |
| [PGM012](#pgm012) | Minor | DROP COLUMN silently removes foreign key |
| [PGM013](#pgm013) | Critical | SET NOT NULL on existing table requires ACCESS EXCLUSIVE lock |
| [PGM014](#pgm014) | Critical | ADD FOREIGN KEY on existing table without NOT VALID |
| [PGM015](#pgm015) | Critical | ADD CHECK on existing table without NOT VALID |
| [PGM016](#pgm016) | Major | ADD PRIMARY KEY on existing table without USING INDEX |
| [PGM017](#pgm017) | Critical | ADD UNIQUE on existing table without USING INDEX |
| [PGM018](#pgm018) | Critical | CLUSTER on existing table |
| [PGM019](#pgm019) | Critical | ADD EXCLUDE constraint on existing table |
| [PGM020](#pgm020) | Minor | DISABLE TRIGGER on table suppresses FK enforcement |
| [PGM021](#pgm021) | Critical | VACUUM FULL on existing table |
| [PGM022](#pgm022) | Critical | Missing CONCURRENTLY on REINDEX |
| [PGM023](#pgm023) | Minor | Multiple ALTER TABLE statements on the same table can be combined |
| [PGM024](#pgm024) | Critical | DROP TABLE or CREATE TABLE PARTITION OF locks the parent partitioned table |
| [PGM025](#pgm025) | Minor | DROP COLUMN silently removes EXCLUDE constraint |
| [PGM026](#pgm026) | Critical | One transaction locks several partitioned parent tables |
| [PGM027](#pgm027) | Major | Statement fails against the schema from earlier migrations |
| [PGM101](#pgm101) | Minor | Column uses timestamp without time zone |
| [PGM102](#pgm102) | Minor | Column uses timestamp or timestamptz with precision 0 |
| [PGM103](#pgm103) | Minor | Column uses char(n) type |
| [PGM104](#pgm104) | Minor | Column uses the money type |
| [PGM105](#pgm105) | Info | Column uses serial/bigserial instead of identity column |
| [PGM106](#pgm106) | Minor | Column uses json type instead of jsonb |
| [PGM107](#pgm107) | Major | Primary key column uses integer or smallint instead of bigint |
| [PGM108](#pgm108) | Info | Column uses varchar(n) instead of text |
| [PGM109](#pgm109) | Minor | Column uses floating-point type instead of numeric |
| [PGM201](#pgm201) | Minor | DROP TABLE on existing table |
| [PGM202](#pgm202) | Major | DROP TABLE CASCADE on existing table |
| [PGM203](#pgm203) | Minor | TRUNCATE TABLE on existing table |
| [PGM204](#pgm204) | Major | TRUNCATE TABLE CASCADE on existing table |
| [PGM205](#pgm205) | Critical | DROP SCHEMA CASCADE |
| [PGM301](#pgm301) | Info | INSERT INTO existing table in migration |
| [PGM302](#pgm302) | Minor | UPDATE on existing table in migration |
| [PGM303](#pgm303) | Minor | DELETE FROM existing table in migration |
| [PGM401](#pgm401) | Minor | Missing IF EXISTS on DROP TABLE / DROP INDEX |
| [PGM402](#pgm402) | Minor | Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX |
| [PGM403](#pgm403) | Minor | CREATE TABLE IF NOT EXISTS for already-existing table is a misleading no-op |
| [PGM501](#pgm501) | Major | Foreign key without covering index on referencing columns |
| [PGM502](#pgm502) | Major | Table without primary key |
| [PGM503](#pgm503) | Info | UNIQUE NOT NULL used instead of PRIMARY KEY |
| [PGM504](#pgm504) | Info | RENAME TABLE on existing table |
| [PGM505](#pgm505) | Info | RENAME COLUMN on existing table |
| [PGM506](#pgm506) | Info | CREATE UNLOGGED TABLE |
| [PGM507](#pgm507) | Info | DROP NOT NULL on existing table allows NULL values |
| [PGM508](#pgm508) | Info | Duplicate or redundant index detected (prefix of another index) |
| [PGM509](#pgm509) | Info | Mixed-case identifier or reserved word requires double-quoting |
| [PGM901](#pgm901) | Info | Down migration severity cap |
