# Proposed Rules

Proposed rules use a `PGM1XXX` prefix indicating their target **range**, not a reserved slot. The leading `1` denotes "proposed"; the remaining digits identify the category (e.g., `PGM1506` targets the 5xx range). When promoted to implementation, a rule takes the next available ID in its range — so if `PGM1508` is promoted before `PGM1507`, it becomes `PGM506` (not `PGM508`). See `PLANNED_SCHEMA_CHANGES.md` for the full numbering scheme.

---

## 0xx — Unsafe DDL

### ~~PGM1021~~ — Removed

`ALTER TYPE ... ADD VALUE` is rollback-safe on PostgreSQL 12+. No longer relevant.

---

### ~~PGM1022~~ — Promoted to **PGM507**

---

### ~~PGM1023~~ — Promoted to **PGM023**

---

### ~~PGM1024~~ — Promoted to **PGM024**

---

## 1xx — Type anti-patterns

### ~~PGM1107~~ — Promoted to **PGM107**

---

### PGM1108 — Prefer `text` over `varchar(n)`

- **Range**: 1xx (TypeAntiPattern)
- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: Column with `TypeName.name` of `varchar` and non-empty `modifiers` (i.e., `varchar(100)`, not bare `varchar`). Detected in `CREATE TABLE` column definitions and `ALTER TABLE ... ADD COLUMN`.
- **Why**: In PostgreSQL, `varchar(n)` has zero performance benefit over `text` — internally they are the same `varlena` storage. The length check adds overhead, and when the limit inevitably needs to increase, `ALTER COLUMN TYPE varchar(200)` requires an ACCESS EXCLUSIVE lock and full table rewrite (on PostgreSQL < 14, or when *decreasing* the limit on 14+). Use `text` with a CHECK constraint if validation is needed — CHECK constraints can be added `NOT VALID` and validated without a rewrite.
- **Does not fire when**:
  - Bare `varchar` (no length modifier — equivalent to `text`).
  - `text` columns.
  - Existing table columns (only fires on new column definitions).
- **Message**: `Column '{col}' uses varchar({n}). Prefer text — varchar(n) has no performance benefit in PostgreSQL, and changing the limit requires a table rewrite.`
- **IR impact**: None — `TypeName` already has `name` and `modifiers`.

---

### PGM1109 — Floating-point column type

- **Range**: 1xx (TypeAntiPattern)
- **Severity**: WARNING
- **Status**: Not yet implemented.
- **Triggers**: Column with `TypeName.name` in `("float4", "float8", "real", "double precision", "float")`. Detected in `CREATE TABLE` column definitions and `ALTER TABLE ... ADD COLUMN`.
- **Why**: IEEE 754 floating-point types suffer from precision issues (`0.1 + 0.2 ≠ 0.3`). For money, quantities, measurements, or any domain where exact decimal values matter, `numeric`/`decimal` is correct. Floating-point errors compound in aggregations and can cause silent data corruption.
- **Does not fire when**:
  - `numeric` / `decimal` columns.
  - Existing table columns (only fires on new column definitions).
- **Message**: `Column '{col}' uses '{type}'. Floating-point types have precision issues (0.1 + 0.2 ≠ 0.3). Use numeric for exact values.`
- **IR impact**: None — same pattern as other type rules, matching on `TypeName.name`.
- **Spike tests needed**: Add spike tests to `tests/pg_query_spike.rs` to verify how pg_query normalizes `float` with precision specifiers — `float(1)` through `float(24)` should map to `float4`/`real`, `float(25)` through `float(53)` to `float8`/`double precision`. Confirm the normalized `TypeName.name` values before implementing.

---

## 2xx — Destructive operations

### ~~PGM1205~~ — Promoted to **PGM205**

---

## 5xx — Schema design & informational

### ~~PGM1509~~ — Promoted to **PGM508**

- **Range**: 5xx (SchemaDesign)
- **Severity**: WARNING
- **Status**: Not yet implemented.
- **Triggers**: `CREATE INDEX` in the changed file where, after applying the migration (`catalog_after`), a non-unique index on a table is a column prefix of another index on the same table. Fires in two directions:
  1. The new index is redundant (its columns are a prefix of an existing index).
  2. The new index makes an existing non-unique index redundant (existing index's columns are a prefix of the new one).
  Also fires for exact duplicates (same columns, same access method) with a sharper message.
- **Why**: Redundant indexes waste disk space, slow down writes (every INSERT/UPDATE/DELETE must maintain all indexes), and add vacuum overhead. A btree index on `(a, b)` already serves lookups on `(a)` alone — a separate index on `(a)` provides no additional query capability.
- **Does not fire when**:
  - The shorter (redundant) index is a UNIQUE index — it enforces a constraint that the longer index does not.
  - Either index is a partial index (has a WHERE clause) — partial indexes serve different query patterns. Documenting this as a known limitation; comparing WHERE clauses for equivalence is complex and deferred.
  - Either index is an expression index (columns contain expressions rather than simple column names) — expression indexes are not directly comparable by column name.
  - The indexes use different access methods (e.g., btree vs GIN) — different access methods serve fundamentally different query types.
- **Message (prefix redundancy)**: `Index '{shorter_idx}' on '{table}' ({shorter_cols}) is redundant — index '{longer_idx}' ({longer_cols}) covers the same prefix.`
- **Message (exact duplicate)**: `Index '{new_idx}' on '{table}' ({cols}) is an exact duplicate of index '{existing_idx}'.`
- **IR impact**: None — `IndexState` already tracks column names and ordering. Requires `access_method: Option<String>` on `IndexState` and `CreateIndex` (currently missing). The parser should extract `IndexStmt.access_method` from pg_query — empty string means btree (the PostgreSQL default). `has_covering_index` should also be updated to skip non-btree indexes.
- **Catalog prerequisite**: Without `access_method` tracking, the rule cannot distinguish btree from non-btree indexes. Safe fallback: assume btree when `access_method` is `None` (matches PostgreSQL's default), which is correct for the vast majority of indexes.

---

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
- **§3.2 IR node table**: Add `DropSchema`, `CreateOrReplaceFunction`, `CreateOrReplaceView`, `VacuumFull`, `Reindex`, `RefreshMatView` (if promoted).
- **§11 Project structure**: Add rule files to `src/rules/` as rules are promoted.
- **PGM901 scope**: Update to cover all promoted rules.
