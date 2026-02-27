# Proposed Rules

Proposed rules use a `PGM1XXX` prefix indicating their target **range**, not a reserved slot. The leading `1` denotes "proposed"; the remaining digits identify the category (e.g., `PGM1506` targets the 5xx range). When promoted to implementation, a rule takes the next available ID in its range — so if `PGM1508` is promoted before `PGM1507`, it becomes `PGM506` (not `PGM508`). See `PLANNED_SCHEMA_CHANGES.md` for the full numbering scheme.

---

## 0xx — Unsafe DDL

### PGM1021 — `ALTER TYPE ... ADD VALUE` in Transaction

- **Range**: 0xx (UnsafeDDL)
- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `ALTER TYPE ... ADD VALUE` when `run_in_transaction` is true.
- **Why**: Adding an enum value cannot be rolled back inside a transaction. If the migration fails partway, the enum value persists after rollback. There is no way to remove it without `DROP TYPE` and recreating.
- **Does not fire when**:
  - `run_in_transaction` is false.
- **Message**: `ALTER TYPE '{type_name}' ADD VALUE '{value}' inside a transaction cannot be rolled back. If the migration fails, the enum value will persist. Run this migration outside a transaction.`
- **IR impact**: New top-level `IrNode::AlterEnum { type_name: String, value: String, if_not_exists: bool }`. Parser: handle `NodeEnum::AlterEnumStmt`. Catalog/normalize: no-op.

---

### PGM1022 — `DROP NOT NULL` on Existing Table

- **Range**: 0xx (UnsafeDDL)
- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `ALTER TABLE ... ALTER COLUMN ... DROP NOT NULL` on an existing table (not created in the same changeset).
- **Why**: Dropping NOT NULL silently allows NULLs where application code may assume non-NULL. This is especially dangerous when the column feeds into aggregations, joins, or application logic that doesn't check for NULL.
- **Does not fire when**:
  - Table was created in the same changeset (`tables_created_in_change`).
  - Table does not exist in `catalog_before`.
- **Message**: `DROP NOT NULL on column '{col}' of existing table '{table}' allows NULL values where the application may assume non-NULL. Verify that all code paths handle NULLs.`
- **IR impact**: New `AlterTableAction::DropNotNull { column_name: String }`. Replaces current `AtDropNotNull → Other` mapping. Catalog replay: set `column.nullable = true` (fixes catalog gap).

---

### PGM1023 — `VACUUM FULL` on Existing Table

- **Range**: 0xx (UnsafeDDL)
- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `VACUUM FULL` targeting a table that exists in `catalog_before`.
- **Why**: `VACUUM FULL` rewrites the entire table under an ACCESS EXCLUSIVE lock, blocking all reads and writes for the duration. On large tables this can mean minutes to hours of downtime. Use `pg_repack` or `pg_squeeze` for online compaction.
- **Does not fire when**:
  - Plain `VACUUM` (without `FULL`).
  - Table is new (not in `catalog_before`).
  - Table not in catalog.
- **Message**: `VACUUM FULL on table '{table}' rewrites the entire table under ACCESS EXCLUSIVE lock, blocking all reads and writes. Use pg_repack or pg_squeeze for online compaction.`
- **IR impact**: New `IrNode::VacuumFull(VacuumFull)` with `table: QualifiedName`. Parser: handle `NodeEnum::VacuumStmt`, only emit `VacuumFull` when FULL option present; plain VACUUM → `Ignored`.

---

### PGM1024 — `REINDEX` without `CONCURRENTLY`

- **Range**: 0xx (UnsafeDDL)
- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `REINDEX TABLE|INDEX|DATABASE|SCHEMA` without `CONCURRENTLY` (PostgreSQL 12+).
- **Why**: `REINDEX` without `CONCURRENTLY` acquires an ACCESS EXCLUSIVE lock on the table (or for `REINDEX INDEX`, on the parent table), blocking all reads and writes.
- **Does not fire when**:
  - `CONCURRENTLY` option is present.
- **Note**: Fires unconditionally (no catalog check needed) since even `REINDEX INDEX` locks the parent table.
- **Message**: `REINDEX {kind} '{name}' without CONCURRENTLY acquires ACCESS EXCLUSIVE lock, blocking all reads and writes. Use REINDEX {kind} CONCURRENTLY '{name}' (PostgreSQL 12+).`
- **IR impact**: New `IrNode::Reindex(Reindex)` with `kind: ReindexKind` (Table/Index/Schema/Database/System), `name: String`, `concurrent: bool`. Parser: handle `NodeEnum::ReindexStmt`.

---

## 1xx — Type anti-patterns

### PGM1107 — Integer Primary Key

- **Range**: 1xx (TypeAntiPattern)
- **Severity**: MAJOR
- **Status**: Not yet implemented.
- **Triggers**: A primary key column with `TypeName.name` in `("int4", "int2")`. Detected in `CREATE TABLE` (inline PK via `ColumnDef.is_inline_pk` or table-level `PRIMARY KEY` constraint) and `ALTER TABLE ... ADD PRIMARY KEY`.
- **Why**: `integer` max is ~2.1 billion, `smallint` max is ~32,000. High-write tables routinely exhaust these ranges. Migration to `bigint` requires an ACCESS EXCLUSIVE lock and full table rewrite — a painful, high-risk operation on production tables.
- **Does not fire when**:
  - PK column type is `int8` / `bigint`.
  - Column is not part of a primary key.
- **Message**: `Primary key column '{col}' on '{table}' uses '{type}'. Consider using bigint to avoid exhausting the integer range on high-write tables.`
- **IR impact**: None — existing IR is sufficient (`ColumnDef.is_inline_pk`, `TableConstraint::PrimaryKey`, `TypeName.name`).

---

## 2xx — Destructive operations

### ~~PGM1205~~ — Promoted to **PGM205**

---

## 5xx — Schema design & informational

### PGM1507 — `CREATE OR REPLACE FUNCTION` / `PROCEDURE` (maybe not?)

- **Range**: 5xx (Informational)
- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE OR REPLACE FUNCTION` or `CREATE OR REPLACE PROCEDURE`.
- **Why**: PostgreSQL prevents signature changes (return type, argument names/types) via `CREATE OR REPLACE` — attempting this produces an error. The risk is logic regression: `OR REPLACE` silently overwrites the existing function body with no "already exists" safety check. A developer reverting a function or deploying a buggy version has no friction — the migration succeeds and the regression is invisible until the function is called in production.
- **Does not fire when**:
  - `OR REPLACE` is absent (plain `CREATE FUNCTION` / `CREATE PROCEDURE` fails explicitly if the function already exists, forcing intentional action).
- **Message**: `CREATE OR REPLACE FUNCTION '{name}' silently overwrites the existing function body. It cannot change signatures, but it can introduce logic regressions with no warning. Verify the replacement is intentional.`
- **IR impact**: Requires a new top-level `IrNode` variant `CreateOrReplaceFunction { name: String }`. `pg_query` emits `CreateFunctionStmt` with `replace: bool`. Only the name and `replace` flag need to be extracted for v1.

---

### PGM1508 — `CREATE OR REPLACE VIEW` (maybe not?)

- **Range**: 5xx (Informational)
- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE OR REPLACE VIEW`.
- **Why**: PostgreSQL prevents removing columns or changing existing column types via `CREATE OR REPLACE VIEW` — attempting this produces an error. However it does permit adding new columns at the end, which silently affects any caller using `SELECT *` positional access. The primary risk is logic regression: `OR REPLACE` silently overwrites the view query with no "already exists" safety check. A developer reverting a view or deploying a buggy version has no friction. Dependent views, rules, or `WITH CHECK OPTION` constraints may also behave differently under the replacement query without any warning at migration time.
- **Does not fire when**:
  - `OR REPLACE` is absent (plain `CREATE VIEW` fails explicitly if the view already exists, forcing intentional action).
- **Message**: `CREATE OR REPLACE VIEW '{name}' silently overwrites the existing view query. New columns added at the end affect SELECT * callers. Verify the replacement is intentional and check dependent views and rules.`
- **IR impact**: Requires a new top-level `IrNode` variant `CreateOrReplaceView { name: String }`. `pg_query` emits `ViewStmt` with `replace: bool`. Only the name and `replace` flag need to be extracted for v1.

---

## Revision notes

These rules extend the current rule set. Proposed rules use the `PGM1XXX` prefix during the proposal phase, where the leading `1` means "proposed" and the remaining digits identify the target range. When a rule is promoted to implementation, it drops the leading `1` and takes the **next available ID** in its range — the exact slot is determined at promotion time, not by the proposal number.

Changes to existing spec sections required:

- **§4.2**: Add promoted rules to the rule table.
- **§3.2 IR node table**: Add `DropSchema`, `CreateOrReplaceFunction`, `CreateOrReplaceView`, `AlterEnum`, `VacuumFull`, `Reindex`.
- **§11 Project structure**: Add rule files to `src/rules/` as rules are promoted.
- **PGM901 scope**: Update to cover all promoted rules.
