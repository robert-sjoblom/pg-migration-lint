# Proposed Rules — PGM024–PGM039

## Rule IDs

All rules follow the `PGMnnn` format. IDs PGM024–PGM039 are assigned here; they remain reserved even if a rule is promoted only partially or deferred.

---

## PGM024 — `TRUNCATE TABLE` on existing table

- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `TRUNCATE TABLE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `TRUNCATE` is instant DDL (does not scan rows) and does not fire row-level `ON DELETE` triggers, bypassing any business logic or auditing those triggers enforce. All data in the table is permanently destroyed. Like `DROP TABLE`, the DDL cost is low but the consequence is irreversible data loss.
- **Interaction with PGM038**: Both PGM024 and PGM038 fire when `TRUNCATE CASCADE` targets an existing table. PGM024 covers the irreversible data destruction aspect; PGM038 covers the silent cascade to dependent tables.
- **Does not fire when**:
  - The table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `TRUNCATE TABLE '{table}' removes all rows from an existing table. This is irreversible and does not fire ON DELETE triggers.`
- **IR impact**: Requires a new top-level `IrNode` variant `TruncateTable { tables: Vec<String>, cascade: bool }`. `pg_query` emits `TruncateStmt` for this operation.

---

## PGM038 — `TRUNCATE TABLE ... CASCADE` on existing table

- **Severity**: MAJOR
- **Status**: Not yet implemented.
- **Triggers**: `TRUNCATE TABLE ... CASCADE` where the target table exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `TRUNCATE CASCADE` automatically extends the truncate to all tables that have FK references to the target table — and recursively to any tables referencing those, without bound. None of the additionally truncated tables are listed in the migration statement. The author may not be aware of all tables in the cascade chain. This is a superset of the PGM024 finding; both fire when `CASCADE` is present.
- **Interaction with PGM024**: Both PGM024 and PGM038 fire when `TRUNCATE CASCADE` targets an existing table. PGM024 covers the irreversible data destruction aspect; PGM038 covers the silent cascade to dependent tables.
- **Does not fire when**:
  - The table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
  - `CASCADE` is absent (PGM024 handles the non-cascade case).
- **Message**: `TRUNCATE TABLE '{table}' CASCADE silently extends to all tables with foreign key references to '{table}', and recursively to their dependents. Verify the full cascade chain is intentionally truncated.`
- **IR impact**: Uses the `cascade: bool` field on `TruncateTable` introduced by PGM024. No additional IR changes required.

---

## PGM025 — `DROP TABLE ... CASCADE` on existing table

- **Severity**: MAJOR
- **Status**: Not yet implemented.
- **Triggers**: `DROP TABLE ... CASCADE` where the target table exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `CASCADE` silently drops all objects that depend on the table — views, foreign key constraints on other tables referencing this one, sequences, rules, and triggers — without enumerating them in the migration. The author may not be aware of all dependents. `DROP TABLE` without `CASCADE` is already flagged by PGM022 (MINOR); the presence of `CASCADE` elevates severity because the blast radius extends invisibly beyond the table itself.
- **Interaction with PGM022**: Both PGM022 and PGM025 fire when `DROP TABLE CASCADE` targets an existing table. PGM022 covers the data destruction aspect; PGM025 covers the silent dependency removal.
- **Does not fire when**:
  - The table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
  - `CASCADE` is absent (PGM022 handles the non-cascade case).
- **Message**: `DROP TABLE '{table}' CASCADE silently drops all dependent objects (views, foreign keys on referencing tables, sequences). Verify all dependencies are intentionally removed.`
- **IR impact**: Requires a `cascade: bool` field on the existing `DropTable` IR node.

---

## PGM026 — `DROP SCHEMA ... CASCADE`

- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `DROP SCHEMA ... CASCADE` where the schema contains at least one table in `catalog_before`.
- **Why**: `DROP SCHEMA CASCADE` drops every object in the schema — tables, views, sequences, functions, types, and indexes — in a single statement. It is the most destructive single DDL statement in PostgreSQL and cannot be selectively undone. There is no analog to `DROP TABLE CASCADE` that limits the scope; the entire schema is destroyed.
- **Does not fire when**:
  - The schema contains no tables in `catalog_before` (empty schema, no data at risk).
  - `CASCADE` is absent (`DROP SCHEMA` without `CASCADE` fails if the schema is non-empty, which is a safe default).
- **Message**: `DROP SCHEMA '{schema}' CASCADE drops every object in the schema. This is irreversible and destroys all tables, views, sequences, functions, and types within it.`
- **IR impact**: Requires a new top-level `IrNode` variant `DropSchema { name: String, cascade: bool, if_exists: bool }`. `pg_query` emits `DropStmt(OBJECT_SCHEMA)`.

---

## PGM027 — `INSERT INTO` existing table in migration

- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `INSERT INTO` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Inserting into an existing table in a migration is often intentional seed or reference data, but bulk `INSERT ... SELECT` or large `VALUES` lists hold row locks for the full statement duration and can cause replication lag. The rule fires informational to prompt the author to confirm row volume is bounded and the insert is appropriate in a migration rather than application-layer seed code.
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `INSERT INTO existing table '{table}' in a migration. Ensure this is intentional seed data and that row volume is bounded. Bulk INSERT ... SELECT can cause replication lag and should be batched for large datasets.`
- **IR impact**: Requires a new top-level `IrNode` variant `InsertInto { table: String }`. `pg_query` emits `InsertStmt`. Only the target table name needs to be extracted.

---

## PGM028 — `UPDATE` on existing table in migration

- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `UPDATE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Unbatched `UPDATE` in a migration holds row-level locks on every matched row for the full statement duration. On large tables this blocks concurrent reads and writes, causes replication lag, and can cascade into lock queues behind the migration. The migration cannot know table row counts at analysis time, so the rule fires on any `UPDATE` against an existing table, prompting the author to verify row volume and consider batched execution.
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `UPDATE on existing table '{table}' in a migration. Unbatched updates hold row locks for the full statement duration. Verify row volume and consider batched execution to avoid replication lag and lock queue buildup.`
- **IR impact**: Requires a new top-level `IrNode` variant `UpdateTable { table: String }`. `pg_query` emits `UpdateStmt`. Only the target table name needs to be extracted.

---

## PGM029 — `DELETE FROM` existing table in migration

- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `DELETE FROM` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Unbatched `DELETE` in a migration holds row-level locks on every matched row for the full statement duration. On large tables this blocks concurrent writes, generates significant WAL, causes replication lag, and can cascade into lock queues. `DELETE` also does not reset sequences, unlike `TRUNCATE`, meaning a large delete followed by re-inserts can exhaust the sequence faster than expected.
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `DELETE FROM existing table '{table}' in a migration. Unbatched deletes hold row locks for the full statement duration and generate significant WAL. Verify row volume and consider batched execution to avoid replication lag and lock queue buildup.`
- **IR impact**: Requires a new top-level `IrNode` variant `DeleteFrom { table: String }`. `pg_query` emits `DeleteStmt`. Only the target table name needs to be extracted.

---

## PGM030 — `DISABLE TRIGGER` on existing table

- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `ALTER TABLE ... DISABLE TRIGGER` (any trigger, including `ALL`) on a table that exists in `catalog_before`.
- **Why**: Disabling triggers in a migration bypasses business logic and — critically — foreign key enforcement triggers. `DISABLE TRIGGER ALL` suppresses FK checks for the duration between the disable and the corresponding re-enable. If the re-enable is missing, omitted due to a migration failure, or placed in a separate migration that is never run, the integrity guarantee is permanently lost. Even intentional disables for bulk load performance are high-risk in migration files.
- **Does not fire when**:
  - The table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `DISABLE TRIGGER on table '{table}' suppresses triggers including foreign key enforcement. If this is not re-enabled in the same migration, referential integrity guarantees are lost until manually restored.`
- **IR impact**: Requires a new `AlterTableAction::DisableTrigger { trigger_name: Option<String>, all: bool }` variant. `pg_query` emits `AlterTableCmd(AT_DisableTrigger)` / `AT_DisableAlwaysTrigger`.

---

## PGM031 — `CLUSTER` on existing table

- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `CLUSTER table_name [USING index_name]` where the table exists in `catalog_before`.
- **Why**: `CLUSTER` rewrites the entire table and all its indexes in a new physical order, holding `ACCESS EXCLUSIVE` lock for the full duration of the rewrite. Unlike `VACUUM FULL`, there is no online alternative. On large tables this causes complete unavailability (all reads and writes blocked) for the duration — typically minutes to hours. It is almost never appropriate in an online migration.
- **Does not fire when**:
  - The table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `CLUSTER on table '{table}' rewrites the entire table under ACCESS EXCLUSIVE lock for the full duration. All reads and writes are blocked. This is rarely appropriate in an online migration.`
- **IR impact**: Requires a new top-level `IrNode` variant `Cluster { table: String, index: Option<String> }`. `pg_query` emits `ClusterStmt`.

---

## PGM032 — `DETACH PARTITION` without `CONCURRENTLY`

- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `ALTER TABLE parent DETACH PARTITION child` without the `CONCURRENTLY` option, where `parent` exists in `catalog_before`.
- **Why**: Plain `DETACH PARTITION` acquires `ACCESS EXCLUSIVE` on both the parent partitioned table and the child partition for the full duration of the operation. This blocks all reads and writes on the parent (and therefore all its partitions) until detach completes. PostgreSQL 14+ introduced `DETACH PARTITION ... CONCURRENTLY`, which uses a weaker lock and allows concurrent reads and writes. There is no reason to use the blocking form in an online migration against an existing partitioned table.
- **Does not fire when**:
  - `CONCURRENTLY` is present.
  - The parent table is created in the same set of changed files.
  - The parent table does not exist in `catalog_before`.
- **Minimum PostgreSQL version**: `DETACH PARTITION CONCURRENTLY` requires PostgreSQL 14+. The rule fires unconditionally but the message notes the version requirement.
- **Message**: `DETACH PARTITION on existing partitioned table '{parent}' without CONCURRENTLY acquires ACCESS EXCLUSIVE on the entire table, blocking all reads and writes. Use DETACH PARTITION ... CONCURRENTLY (PostgreSQL 14+).`
- **IR impact**: Requires a new top-level `IrNode` variant `DetachPartition { parent: String, child: String, concurrent: bool }`. `pg_query` emits `AlterTableCmd(AT_DetachPartition)` with a `concurrent` flag on the node.

---

## PGM033 — `ATTACH PARTITION` without pre-existing validated `CHECK` constraint

- **Severity**: MAJOR
- **Status**: Not yet implemented.
- **Triggers**: `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...` where:
  - `parent` exists in `catalog_before`.
  - `child` exists in `catalog_before`.
  - `child` has **no `CHECK` constraints** in `catalog_before`.
- **Why**: When attaching a partition, PostgreSQL must verify that every existing row in `child` satisfies the partition bound. If the child table already has a validated `CHECK` constraint whose expression implies the partition bound, PostgreSQL skips the full-table scan and trusts the constraint instead. Without such a constraint, PostgreSQL performs the scan under `ACCESS EXCLUSIVE` lock on the child table. For large child tables this causes extended unavailability.
- **Safe alternative**:
  ```sql
  -- Migration 1: add a CHECK constraint that mirrors the partition bound
  ALTER TABLE orders_2024 ADD CONSTRAINT orders_2024_partition_check
      CHECK (created_at >= '2024-01-01' AND created_at < '2025-01-01') NOT VALID;

  -- Migration 2: validate separately (SHARE UPDATE EXCLUSIVE — allows reads & writes)
  ALTER TABLE orders_2024 VALIDATE CONSTRAINT orders_2024_partition_check;

  -- Migration 3: attach (scan skipped because constraint is already validated)
  ALTER TABLE orders_partitioned ATTACH PARTITION orders_2024
      FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
  ```
- **Does not fire when**:
  - `parent` is created in the same set of changed files.
  - `child` is created in the same set of changed files (no existing rows; no scan occurs).
  - `parent` does not exist in `catalog_before`.
  - `child` does not exist in `catalog_before`.
  - `child` has at least one `CHECK` constraint in `catalog_before`.
- **Note**: v1 does not attempt to verify whether the existing `CHECK` expression semantically implies the partition bound — detecting an expression match requires evaluating predicate implication, which is out of scope. The rule fires on the absence of any `CHECK` constraint, which is the high-signal case. A false negative occurs when a `CHECK` constraint exists but does not imply the bound; this is acceptable for v1.
- **Message**: `ATTACH PARTITION of existing table '{child}' to '{parent}' will scan the entire child table under ACCESS EXCLUSIVE lock to verify the partition bound. Add a CHECK constraint mirroring the partition bound, validate it separately, then attach.`
- **IR impact**: Requires a new top-level `IrNode` variant `AttachPartition { parent: String, child: String }`. `pg_query` emits `AlterTableCmd(AT_AttachPartition)`.

---

## PGM034 — `ADD EXCLUDE` constraint on existing table

- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE (...)` on a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Adding an `EXCLUDE` constraint acquires `ACCESS EXCLUSIVE` lock (blocking all reads and writes) and scans all existing rows to verify the exclusion condition. Unlike `CHECK` and `FOREIGN KEY` constraints, PostgreSQL does not support `NOT VALID` for `EXCLUDE` constraints — attempting it produces a syntax error. There is also no equivalent to `ADD CONSTRAINT ... USING INDEX` for exclusion constraints, so the safe pre-build-then-attach pattern used for `UNIQUE` (PGM021) does not apply. There is currently no online path to add an exclusion constraint to a large existing table without an `ACCESS EXCLUSIVE` lock for the duration of the scan.
- **Does not fire when**:
  - The table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `Adding EXCLUDE constraint '{constraint}' on existing table '{table}' acquires ACCESS EXCLUSIVE lock and scans all rows. There is no online alternative — consider scheduling this during a maintenance window.`
- **IR impact**: Requires a new `TableConstraint::Exclude { name: Option<String> }` variant. `pg_query` emits `Constraint(CONSTR_EXCLUSION)`.

---

## PGM035 — `CREATE UNLOGGED TABLE`

- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE TABLE ... UNLOGGED` for any table.
- **Why**: Unlogged tables are not written to the WAL. This means: (1) all data is truncated on crash recovery, (2) they are not streamed to standby replicas via streaming replication, and (3) they are excluded from logical replication slots. In most production environments, unlogged tables are unsuitable for data that needs to survive a crash or be replicated. The pattern is sometimes used intentionally for ephemeral or staging data, so this is informational rather than a hard block.
- **Does not fire when**:
  - The `UNLOGGED` keyword is absent.
- **Message**: `Table '{table}' is created as UNLOGGED. Unlogged tables are truncated on crash recovery and not replicated to standbys. Confirm this is intentional for ephemeral data.`
- **IR impact**: Requires an `unlogged: bool` field on the `CreateTable` IR node alongside the existing `temporary: bool` field. `pg_query` exposes `relpersistence` on `CreateStmt` (`'p'` = permanent, `'u'` = unlogged, `'t'` = temporary).

---

## PGM036 — `CREATE OR REPLACE FUNCTION` / `PROCEDURE` (maybe not?)

- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE OR REPLACE FUNCTION` or `CREATE OR REPLACE PROCEDURE`.
- **Why**: PostgreSQL prevents signature changes (return type, argument names/types) via `CREATE OR REPLACE` — attempting this produces an error. The risk is logic regression: `OR REPLACE` silently overwrites the existing function body with no "already exists" safety check. A developer reverting a function or deploying a buggy version has no friction — the migration succeeds and the regression is invisible until the function is called in production.
- **Does not fire when**:
  - `OR REPLACE` is absent (plain `CREATE FUNCTION` / `CREATE PROCEDURE` fails explicitly if the function already exists, forcing intentional action).
- **Message**: `CREATE OR REPLACE FUNCTION '{name}' silently overwrites the existing function body. It cannot change signatures, but it can introduce logic regressions with no warning. Verify the replacement is intentional.`
- **IR impact**: Requires a new top-level `IrNode` variant `CreateOrReplaceFunction { name: String }`. `pg_query` emits `CreateFunctionStmt` with `replace: bool`. Only the name and `replace` flag need to be extracted for v1.

---

## PGM037 — `CREATE OR REPLACE VIEW` (maybe not?)

- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE OR REPLACE VIEW`.
- **Why**: PostgreSQL prevents removing columns or changing existing column types via `CREATE OR REPLACE VIEW` — attempting this produces an error. However it does permit adding new columns at the end, which silently affects any caller using `SELECT *` positional access. The primary risk is logic regression: `OR REPLACE` silently overwrites the view query with no "already exists" safety check. A developer reverting a view or deploying a buggy version has no friction. Dependent views, rules, or `WITH CHECK OPTION` constraints may also behave differently under the replacement query without any warning at migration time.
- **Does not fire when**:
  - `OR REPLACE` is absent (plain `CREATE VIEW` fails explicitly if the view already exists, forcing intentional action).
- **Message**: `CREATE OR REPLACE VIEW '{name}' silently overwrites the existing view query. New columns added at the end affect SELECT * callers. Verify the replacement is intentional and check dependent views and rules.`
- **IR impact**: Requires a new top-level `IrNode` variant `CreateOrReplaceView { name: String }`. `pg_query` emits `ViewStmt` with `replace: bool`. Only the name and `replace` flag need to be extracted for v1.

---

## PGM039 — `CREATE TABLE IF NOT EXISTS` for already-existing table

- **Severity**: WARNING
- **Status**: Not yet implemented.
- **Triggers**: `CREATE TABLE IF NOT EXISTS` targeting a table that already exists in the catalog at that point in the migration history.
- **Why**: `IF NOT EXISTS` makes the statement a silent no-op when the table already exists. If the column definitions in the `CREATE TABLE` differ from the actual table state (built up from the original `CREATE TABLE` plus subsequent `ALTER TABLE` statements), the migration author may believe the table has the shape described in the statement, when in reality PostgreSQL ignores it entirely. The migration chain is ambiguous — two competing definitions of the same table exist in the history, and only the first one (plus its alterations) is truth. This is especially common with Liquibase 4.26+, which supports `ifNotExists="true"` on `<createTable>`.
- **Does not fire when**:
  - The table does not already exist in the catalog (the statement genuinely creates it).
  - `IF NOT EXISTS` is absent (a duplicate `CREATE TABLE` without the guard would fail at runtime, which is a different problem).
- **Message**: `CREATE TABLE IF NOT EXISTS '{table}' is a no-op — the table already exists in the migration history. The definition in this statement is silently ignored by PostgreSQL. If the column definitions differ from the actual table state, this migration is misleading.`
- **IR impact**: None — `CreateTable.if_not_exists` already exists in the IR. The rule only needs `catalog_before` to check for prior existence.
- **Catalog impact**: The replay engine already skips `CREATE TABLE IF NOT EXISTS` when the table exists and emits a stderr warning (implemented ahead of the rule).

---

## Revision notes

These rules extend the v1 spec. Suggested next revision label: **v1.13**.

Changes to existing spec sections required:

- **§4.2**: Add PGM024–PGM039 to the rule table.
- **§4.1**: Note that the 0xx range now extends through PGM039; 1xx range is unchanged.
- **§3.2 IR node table**: Add `TruncateTable`, `DropSchema`, `InsertInto`, `UpdateTable`, `DeleteFrom`, `Cluster`, `DetachPartition`, `AttachPartition`, `CreateOrReplaceFunction`, `CreateOrReplaceView`; extend `DropTable` with `cascade: bool`; extend `CreateTable` with `unlogged: bool`; add `AlterTableAction::DisableTrigger`; add `TableConstraint::Exclude` with `not_valid: bool`.
- **§11 Project structure**: Add `pgm024.rs` through `pgm039.rs` to `src/rules/`.
- **PGM901 scope**: Update to cover PGM001–PGM039. `CREATE OR REPLACE` in a `.down.sql` is arguably more dangerous than in `.up.sql` — silently reverting a function or view definition that live traffic depends on — so PGM036 and PGM037 should be included.
