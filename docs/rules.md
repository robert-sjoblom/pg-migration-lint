# Rule Reference

`pg-migration-lint` ships with 29 lint rules across five categories:

- **Unsafe DDL** (PGM001–PGM017) — detect locking, rewrites, runtime failures, and silent side effects in DDL migrations.
- **Type Anti-patterns** (PGM101–PGM106) — flag column types that should be avoided per PostgreSQL best practice.
- **Destructive Operations** (PGM201) — flag data-loss operations.
- **Idempotency Guards** (PGM401–PGM402) — detect missing IF EXISTS / IF NOT EXISTS guards.
- **Schema Design** (PGM501–PGM505) — schema quality and informational findings.
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

**Severity**: Critical

Detects `CREATE INDEX` on an existing table without the `CONCURRENTLY` option. Without `CONCURRENTLY`, PostgreSQL acquires an ACCESS EXCLUSIVE lock for the entire duration of the index build, blocking all reads and writes.

**Example** (bad):
```sql
CREATE INDEX idx_orders_status ON orders (status);
```

**Fix**:
```sql
CREATE INDEX CONCURRENTLY idx_orders_status ON orders (status);
```

Does not fire when the table is created in the same set of changed files (locking an empty table is harmless). See also [PGM003](#pgm003--concurrently-inside-transaction).

---

### PGM002 — Missing CONCURRENTLY on DROP INDEX

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

See also [PGM003](#pgm003--concurrently-inside-transaction).

---

### PGM003 — CONCURRENTLY inside transaction

**Severity**: Critical

Detects `CREATE INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY` inside a migration unit that runs in a transaction. PostgreSQL does not allow concurrent index operations inside a transaction block — the command will fail at runtime.

**Example** (bad — Liquibase changeset with default `runInTransaction`):
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

See also [PGM001](#pgm001--missing-concurrently-on-create-index) and [PGM002](#pgm002--missing-concurrently-on-drop-index).

---

### PGM006 — Volatile default on column

**Severity**: Minor

Detects `ALTER TABLE ... ADD COLUMN` with a function call as the DEFAULT expression on an existing table. On PostgreSQL 11+, non-volatile defaults are applied lazily without rewriting the table. Volatile defaults (`now()`, `random()`, `gen_random_uuid()`, etc.) force a full table rewrite under an ACCESS EXCLUSIVE lock.

**Severity levels per finding**:
- **Minor**: Known volatile functions (`now`, `current_timestamp`, `random`, `gen_random_uuid`, `uuid_generate_v4`, `clock_timestamp`, `timeofday`, `txid_current`, `nextval`)
- **Info**: Unknown function calls — developer should verify volatility

**Example** (flagged):
```sql
ALTER TABLE orders ADD COLUMN created_at timestamptz DEFAULT now();
```

**Fix**:
```sql
ALTER TABLE orders ADD COLUMN created_at timestamptz;
-- Then backfill:
UPDATE orders SET created_at = now() WHERE created_at IS NULL;
```

Does not fire on `CREATE TABLE` (no existing rows to rewrite).

---

### PGM007 — ALTER COLUMN TYPE on existing table

**Severity**: Critical

Detects `ALTER TABLE ... ALTER COLUMN ... TYPE ...` on pre-existing tables. Most type changes require a full table rewrite under an ACCESS EXCLUSIVE lock.

**Safe casts** (no finding):
- `varchar(N)` → `varchar(M)` where M > N
- `varchar(N)` → `text`
- `numeric(P,S)` → `numeric(P2,S)` where P2 > P and same scale
- `bit(N)` → `bit(M)` where M > N
- `varbit(N)` → `varbit(M)` where M > N

**Info cast**: `timestamp` → `timestamptz` (safe in PG 15+ with UTC timezone; verify your timezone config)

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

### PGM008 — ADD COLUMN NOT NULL without DEFAULT

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

**Severity**: Minor

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in a UNIQUE constraint or unique index. PostgreSQL automatically drops dependent constraints, silently removing uniqueness guarantees.

**Example** (bad):
```sql
-- Table has UNIQUE(email)
ALTER TABLE users DROP COLUMN email;
-- The unique constraint is silently removed.
```

**Fix**: Verify that the uniqueness guarantee is no longer needed before dropping the column.

See also [PGM011](#pgm011--drop-column-silently-removes-primary-key), [PGM012](#pgm012--drop-column-silently-removes-foreign-key).

---

### PGM011 — DROP COLUMN silently removes primary key

**Severity**: Major

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in the table's primary key. The table loses its row identity, affecting replication, ORMs, query planning, and data integrity.

**Example** (bad):
```sql
-- Table has PRIMARY KEY (id)
ALTER TABLE orders DROP COLUMN id;
-- The primary key is silently removed.
```

**Fix**: Add a new primary key on remaining columns before or after dropping the column.

See also [PGM010](#pgm010--drop-column-silently-removes-unique-constraint), [PGM012](#pgm012--drop-column-silently-removes-foreign-key).

---

### PGM012 — DROP COLUMN silently removes foreign key

**Severity**: Minor

Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in a FOREIGN KEY constraint. The referential integrity guarantee is silently lost, potentially allowing orphaned rows.

**Example** (bad):
```sql
-- Table has FOREIGN KEY (customer_id) REFERENCES customers(id)
ALTER TABLE orders DROP COLUMN customer_id;
-- The foreign key constraint is silently removed.
```

**Fix**: Verify that the referential integrity guarantee is no longer needed before dropping the column.

See also [PGM010](#pgm010--drop-column-silently-removes-unique-constraint), [PGM011](#pgm011--drop-column-silently-removes-primary-key).

---

### PGM013 — SET NOT NULL requires ACCESS EXCLUSIVE lock

**Severity**: Critical

Detects `ALTER TABLE ... ALTER COLUMN ... SET NOT NULL` on a pre-existing table. This acquires an ACCESS EXCLUSIVE lock and performs a full table scan to verify no existing rows contain NULL.

**Example** (bad):
```sql
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
```

**Fix** (safe three-step pattern, PostgreSQL 12+):
```sql
-- Step 1: Add a CHECK constraint with NOT VALID (instant)
ALTER TABLE orders ADD CONSTRAINT orders_status_nn
  CHECK (status IS NOT NULL) NOT VALID;
-- Step 2: Validate (SHARE UPDATE EXCLUSIVE lock, concurrent reads OK)
ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;
-- Step 3: Set NOT NULL (instant since PG 12 sees the validated CHECK)
ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
-- Step 4 (optional): Drop the now-redundant CHECK
ALTER TABLE orders DROP CONSTRAINT orders_status_nn;
```

See also [PGM015](#pgm015--add-check-without-not-valid).

---

### PGM014 — ADD FOREIGN KEY without NOT VALID

**Severity**: Critical

Detects `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY` on a pre-existing table without the `NOT VALID` modifier. Without `NOT VALID`, PostgreSQL immediately validates all existing rows under a SHARE ROW EXCLUSIVE lock.

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

See also [PGM015](#pgm015--add-check-without-not-valid).

---

### PGM015 — ADD CHECK without NOT VALID

**Severity**: Critical

Detects `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)` on a pre-existing table without `NOT VALID`. Without `NOT VALID`, PostgreSQL acquires an ACCESS EXCLUSIVE lock and scans the entire table to verify all existing rows.

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

See also [PGM013](#pgm013--set-not-null-requires-access-exclusive-lock), [PGM014](#pgm014--add-foreign-key-without-not-valid).

---

### PGM016 — ADD PRIMARY KEY without USING INDEX

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

---

### PGM017 — ADD UNIQUE without USING INDEX

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

See also [PGM016](#pgm016--add-primary-key-without-using-index).

---

## 1xx — Type Anti-pattern Rules

These rules flag column types that should be avoided per the PostgreSQL wiki's ["Don't Do This"](https://wiki.postgresql.org/wiki/Don't_Do_This) recommendations.

### PGM101 — Don't use timestamp without time zone

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

### PGM102 — Don't use timestamp(0) or timestamptz(0)

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

### PGM103 — Don't use char(n)

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

### PGM104 — Don't use the money type

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

### PGM105 — Don't use serial / bigserial

**Severity**: Info

Detects columns declared as `serial`, `bigserial`, or `smallserial`. Since PostgreSQL 10, identity columns (`GENERATED ALWAYS AS IDENTITY`) provide the same auto-incrementing behavior with tighter ownership, better permission handling, and SQL standard compliance.

**Example** (flagged):
```sql
CREATE TABLE orders (id serial PRIMARY KEY);
```

**Fix**:
```sql
CREATE TABLE orders (
  id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY
);
```

---

### PGM106 — Don't use json (prefer jsonb)

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

## 2xx — Destructive Operation Rules

### PGM201 — DROP TABLE on existing table

**Severity**: Minor

Detects `DROP TABLE` targeting a pre-existing table. The DDL is instant (no table scan or extended lock), so this is not a downtime risk — it is a data loss risk.

**Example**:
```sql
DROP TABLE orders;
```

**Recommended approach**:
1. Ensure no application code, views, or foreign keys reference the table.
2. Consider renaming the table first and waiting before dropping.
3. Take a backup of the table data if it may be needed later.

---

## 4xx — Idempotency Guard Rules

### PGM401 — Missing IF EXISTS on DROP TABLE / DROP INDEX

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

---

### PGM402 — Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX

**Severity**: Minor

Detects `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause. Without `IF NOT EXISTS`, the statement fails if the object already exists, causing hard failures in migration pipelines that may be re-run.

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

See also [PGM401](#pgm401--missing-if-exists-on-drop-table--drop-index).

---

## 5xx — Schema Design Rules

### PGM501 — Foreign key without covering index

**Severity**: Major

Detects foreign key constraints where the referencing table has no index whose leading columns match the FK columns in order. Without such an index, deletes and updates on the referenced table cause sequential scans on the referencing table.

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

Uses prefix matching: FK columns `(a, b)` are covered by index `(a, b)` or `(a, b, c)` but **not** by `(b, a)` or `(a)`. Column order matters. The check uses the catalog state after the entire file is processed, so creating the index later in the same file avoids a false positive.

---

### PGM502 — Table without primary key

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

Temporary tables are excluded. When [PGM503](#pgm503--unique-not-null-used-instead-of-primary-key) fires (UNIQUE NOT NULL substitute detected), PGM502 does not fire for the same table.

---

### PGM503 — UNIQUE NOT NULL used instead of PRIMARY KEY

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

When PGM503 fires, [PGM502](#pgm502--table-without-primary-key) does not fire for the same table.

---

### PGM504 — RENAME TABLE on existing table

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

## Meta-behavior Rules

### PGM901 — Down migration severity cap

**Severity**: Info

Not a standalone lint rule. When a migration file is identified as a down/rollback migration (e.g., `*.down.sql`), all findings from other rules are capped to INFO severity. Down migrations are informational only — they represent the undo path and are not expected to follow the same safety rules as forward migrations.

This rule cannot be suppressed (it is applied automatically by the pipeline).

---

## Quick reference table

| Rule | Severity | Description |
|------|----------|-------------|
| PGM001 | Critical | Missing CONCURRENTLY on CREATE INDEX |
| PGM002 | Critical | Missing CONCURRENTLY on DROP INDEX |
| PGM003 | Critical | CONCURRENTLY inside transaction |
| PGM006 | Minor | Volatile default on column |
| PGM007 | Critical | ALTER COLUMN TYPE causes table rewrite |
| PGM008 | Critical | ADD COLUMN NOT NULL without DEFAULT |
| PGM009 | Info | DROP COLUMN on existing table |
| PGM010 | Minor | DROP COLUMN silently removes unique constraint |
| PGM011 | Major | DROP COLUMN silently removes primary key |
| PGM012 | Minor | DROP COLUMN silently removes foreign key |
| PGM013 | Critical | SET NOT NULL requires ACCESS EXCLUSIVE lock |
| PGM014 | Critical | ADD FOREIGN KEY without NOT VALID |
| PGM015 | Critical | ADD CHECK without NOT VALID |
| PGM016 | Major | ADD PRIMARY KEY without USING INDEX |
| PGM017 | Critical | ADD UNIQUE without USING INDEX |
| PGM101 | Minor | Don't use timestamp without time zone |
| PGM102 | Minor | Don't use timestamp(0) / timestamptz(0) |
| PGM103 | Minor | Don't use char(n) |
| PGM104 | Minor | Don't use money type |
| PGM105 | Info | Don't use serial / bigserial |
| PGM106 | Minor | Don't use json (prefer jsonb) |
| PGM201 | Minor | DROP TABLE on existing table |
| PGM401 | Minor | Missing IF EXISTS on DROP TABLE / DROP INDEX |
| PGM402 | Minor | Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX |
| PGM501 | Major | Foreign key without covering index |
| PGM502 | Major | Table without primary key |
| PGM503 | Info | UNIQUE NOT NULL used instead of PRIMARY KEY |
| PGM504 | Info | RENAME TABLE on existing table |
| PGM505 | Info | RENAME COLUMN on existing table |
| PGM901 | Info | Down migration severity cap (meta-behavior) |
