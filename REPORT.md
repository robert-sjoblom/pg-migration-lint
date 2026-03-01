# pg-migration-lint — Comprehensive Review Report

**Date:** 2026-02-27
**Scope:** DRY, KISS, PostgreSQL correctness, maintainability, separation of concerns, module structure, rule gap analysis

---

## Executive Summary

The codebase is well-structured, well-tested (545 tests, ~15k LOC), and covers the core PostgreSQL migration safety landscape thoroughly. The IR layer provides clean decoupling, the pipeline is efficient (single-pass), and individual rule files are focused and readable.

**Critical findings:**
- Several high-value rules are missing compared to competitors (integer PK, enum ADD VALUE in txn, DROP NOT NULL)

---

## 1. PostgreSQL Correctness

### 1.3 Catalog Model Gaps

| Gap | Impact | Priority |
|-----|--------|----------|
| `VALIDATE CONSTRAINT` not tracked (`not_valid` stays `true` forever) | PGM013 can't detect safe 3-step pattern | Medium |
| `DROP CONSTRAINT` not replayed (constraint stays in catalog) | Stale FK/PK/UNIQUE state for PGM501/502/503 | Medium |
| `DROP NOT NULL` not tracked (column stays `nullable=false`) | Stale nullability for PGM016/503 | Medium |
| `SET DEFAULT`/`DROP DEFAULT` not replayed | Stale default_expr state | Low |
| Index access method not tracked (B-tree vs GiST/GIN/BRIN) | `has_covering_index` can't filter non-B-tree | Low |
| EXCLUDE constraint columns not parsed | DROP COLUMN doesn't remove affected EXCLUDE | Low (acknowledged in TODO) |

---

## 4. Maintainability & Module Structure

### 4.1 API Surface — Zero `pub(crate)` Discipline

There is exactly **one** `pub(crate)` in the entire codebase (`output::emit_to_file`). Everything else is fully `pub`, making the library API maximally broad. Internal mutation methods like `Catalog::get_table_mut()`, `Catalog::register_index()`, and `catalog::replay::apply()` are accessible to downstream consumers.

**Recommendation:** Audit all `pub` items and tighten to `pub(crate)` where only used within the crate. Key candidates: catalog mutation methods, `RawMigrationUnit`, internal catalog helpers.

### 4.2 Glob Re-exports Flatten Namespaces

**Files:** `catalog/mod.rs:8` (`pub use types::*`), `parser/mod.rs:6` (`pub use ir::*`)

These dump ~30 types each into parent namespaces, making it harder to trace where types are defined. The curated `lib.rs` re-exports are undermined by these wildcards.

---

## 5. Rule Gap Analysis

### 5.1 Must-Have — High Value, Low Effort

> Full specs for these rules are in [`docs/PROPOSED_RULES.md`](docs/PROPOSED_RULES.md).

| Rule | Proposed ID | Severity | Effort |
|------|-------------|----------|--------|
| ~~Integer Primary Key Detection~~ | ~~PGM1107~~ | ~~MAJOR~~ | Promoted to **PGM107** |
| ~~`DROP NOT NULL` on Existing Table~~ | ~~PGM1022~~ | ~~MINOR~~ | Promoted to **PGM507** |

### 5.2 Should-Have — Medium Value, Medium Effort

> Full specs for REINDEX are in [`docs/PROPOSED_RULES.md`](docs/PROPOSED_RULES.md) (PGM1024).

| Rule | Proposed ID | Severity | Effort |
|------|-------------|----------|--------|
| ~~`VACUUM FULL` on existing table~~ | ~~PGM1023~~ | ~~CRITICAL~~ | Promoted to **PGM023** |
| `REINDEX` without `CONCURRENTLY` | PGM1024 | CRITICAL | Tier 2 (new IR variant) |
| Duplicate/redundant indexes | — | WARNING | Tier 1 (catalog-only) |
| Transaction nesting (`BEGIN`/`COMMIT` in migration) | — | WARNING | Tier 2 (new IR variant) |

### 5.3 Nice-to-Have — Low Value or Deferred

| Rule | Notes |
|------|-------|
| `varchar(n)` type rule | Already designed, deferred pending per-rule disable config |
| `float`/`real`/`double precision` type rule | Already designed, deferred |
| `REFRESH MATERIALIZED VIEW` without `CONCURRENTLY` | Uncommon in migration files |
| `INHERITS`-based partitioning | Already designed, awaiting IR extension |
| `DROP DATABASE` in migration | Extremely rare |
| `CREATE EXTENSION` in transaction | Rare edge case |
| Domain constraint rules | Very niche (squawk has these) |
| Unvalidated `NOT VALID` constraint detection | Requires cross-migration analysis (architectural change) |

### 5.4 Comparison with Competitors

**Rules squawk has that pg-migration-lint does NOT:**
- `prefer-bigint-over-int` / `prefer-bigint-over-smallint` — **Proposed: PGM1107**
- `ban-drop-not-null` — **Proposed: PGM1022**
- `ban-drop-database` — Gap: low priority
- `transaction-nesting` — Gap: proposed in §5.2 (no ID yet)
- `prefer-text-field` — Gap: deferred
- `ban-create-domain-with-constraint` — Gap: niche

**Rules strong_migrations has that pg-migration-lint does NOT:**
**All squawk and strong_migrations rules are covered** by existing pg-migration-lint rules, except enum `ADD VALUE` in transaction which was rejected — on PostgreSQL 12+ the operation is rollback-safe.

---

## 6. Prioritized Action Items

> Rule specs are in [`docs/PROPOSED_RULES.md`](docs/PROPOSED_RULES.md). Catalog plan is in §7 below.

### Low (Polish)
7. Replace glob re-exports with explicit re-exports

---

## 7. Catalog Improvement: DROP CONSTRAINT Replay

### Problem

`ALTER TABLE ... DROP CONSTRAINT` currently falls through to `AlterTableAction::Other` in the parser. The catalog replay ignores `Other` actions, so dropped constraints persist in catalog state indefinitely. This produces stale state that affects downstream rules:

- **PGM501** (FK without covering index): may report findings for foreign keys that no longer exist.
- **PGM502** (table without PK): may miss findings when a primary key has been dropped.
- **PGM503** (UNIQUE NOT NULL instead of PK): may report on unique constraints that have been removed.

### Solution

1. **New IR variant**: `AlterTableAction::DropConstraint { name: String }`.
2. **Parser**: Map `AlterTableSubType::AtDropConstraint` to the new variant instead of `Other`.
3. **Catalog replay**: On `DropConstraint { name }`:
   - Remove the constraint by name from `TableState.constraints`.
   - If the removed constraint was a `PrimaryKey`, set `has_primary_key = false` and remove the synthetic `pkey` index.
   - No-op if no constraint with that name exists (idempotent).

### Files affected

| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `DropConstraint { name: String }` to `AlterTableAction` |
| `src/parser/pg_query.rs` | Map `AtDropConstraint` → `AlterTableAction::DropConstraint` |
| `src/catalog/replay.rs` | Handle `DropConstraint` in `apply()`: remove constraint, update `has_primary_key` |

### Tests needed

| Test case | Assertion |
|-----------|-----------|
| Drop FK constraint | FK removed from `TableState.constraints` |
| Drop PK constraint | PK removed, `has_primary_key` set to `false`, synthetic index removed |
| Drop UNIQUE constraint | UNIQUE removed from `TableState.constraints` |
| Drop CHECK constraint | CHECK removed from `TableState.constraints` |
| Drop nonexistent constraint | No panic, no-op |
| `has_primary_key` flag after PK drop | Flag is `false`, PGM502 now fires |

---

## 8. Catalog Model Gaps — Implementation Analysis

Detailed implementation plan for each gap listed in §1.3. All changes are purely additive (new enum variants, optional fields) — no breaking changes.

### 8.1 VALIDATE CONSTRAINT

**Problem:** `ConstraintState` already has a `not_valid: bool` field, but `ALTER TABLE ... VALIDATE CONSTRAINT` falls through to `AlterTableAction::Other`. Once a constraint is added with `NOT VALID`, it stays `not_valid = true` forever. PGM013 cannot detect the safe 3-step pattern (add NOT VALID → validate → set NOT NULL).

**Current code path:** `AtValidateConstraint` → catch-all → `AlterTableAction::Other` → replay ignores it.

**Solution:**

| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `AlterTableAction::ValidateConstraint { name: String }` |
| `src/parser/pg_query.rs` | Map `AtValidateConstraint` → new variant |
| `src/catalog/replay.rs` | Find constraint by name, set `not_valid = false` (FK and CHECK only) |

**Tests:** Validate FK, validate CHECK, validate nonexistent (no-op), PGM014/015 after validate.

**Complexity:** Small | **Rules:** PGM013, PGM014, PGM015

---

### 8.2 DROP CONSTRAINT

See §7 above. Identical structure to this section.

**Complexity:** Small | **Rules:** PGM501, PGM502, PGM503

---

### 8.3 DROP NOT NULL

**Problem:** `AtDropNotNull` maps to `AlterTableAction::Other`. Column stays `nullable = false` in catalog after `ALTER TABLE ... ALTER COLUMN ... DROP NOT NULL`. Stale nullability affects PGM016 (ADD PK checks nullable columns) and PGM503 (UNIQUE NOT NULL detection).

**Current code path:** `AtDropNotNull` (pg_query.rs:552–554) → `Other` → replay ignores it.

**Note:** This gap overlaps with proposed rule PGM1022. The catalog fix is a prerequisite — PGM1022 needs `DropNotNull` as a distinct `AlterTableAction` variant to match on.

**Solution:**

| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `AlterTableAction::DropNotNull { column_name: String }` |
| `src/parser/pg_query.rs` | Map `AtDropNotNull` → new variant (replace line 552–554) |
| `src/catalog/replay.rs` | Find column by name, set `nullable = true` |

**Tests:** DROP NOT NULL on not-null column → `nullable = true`, on already-nullable (no-op), nonexistent column (no-op).

**Complexity:** Small | **Rules:** PGM016, PGM503 (catalog accuracy); enables PGM1022

---

### 8.4 SET DEFAULT / DROP DEFAULT

**Problem:** `ColumnState` has `has_default: bool` and `default_expr: Option<DefaultExpr>`, but `AtSetDefault` and `AtDropDefault` both fall through to `Other`. Default expressions are frozen at CREATE TABLE time.

**Current code path:** `AtSetDefault` / `AtDropDefault` → catch-all → `Other` → replay ignores.

**Impact:** No active rules depend on default state after CREATE TABLE. This is future-proofing — it would matter if PGM006 (volatile default) were extended to cover `ALTER TABLE ... SET DEFAULT`.

**Solution:**

| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `SetDefault { column_name: String, expr: DefaultExpr }` and `DropDefault { column_name: String }` |
| `src/parser/pg_query.rs` | Map `AtSetDefault` → new variant (extract expr via existing `convert_default_expr`), `AtDropDefault` → new variant |
| `src/catalog/replay.rs` | SetDefault: update `has_default = true`, `default_expr = Some(expr)`. DropDefault: set both to `false` / `None` |

**Tests:** SET DEFAULT literal, SET DEFAULT function, DROP DEFAULT clears both fields, nonexistent column (no-op).

**Complexity:** Small-to-medium (default expression extraction already proven in parser) | **Rules:** None active; future PGM006 extension

---

### 8.5 Index Access Method

**Problem:** `IndexState` and `CreateIndex` have no `access_method` field. `has_covering_index` treats all indexes equally — a GIN or BRIN index on FK columns is falsely counted as covering. Only B-tree indexes can serve FK lookups.

**Current state:** pg_query's `IndexStmt.access_method` is available but not extracted. `has_covering_index` (catalog/types.rs) does prefix matching on column names without filtering by index type.

**Solution:**

| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `access_method: Option<String>` to `CreateIndex` |
| `src/catalog/types.rs` | Add `access_method: Option<String>` to `IndexState` |
| `src/parser/pg_query.rs` | Extract `idx.access_method` (empty string = btree default) |
| `src/catalog/replay.rs` | Pass `access_method` when constructing `IndexState` |
| `src/catalog/types.rs` | In `has_covering_index`: skip indexes where `access_method` is not B-tree (or None/empty, which defaults to B-tree) |

**Tests:** CREATE INDEX USING gin → stored as `"gin"`, CREATE INDEX (no USING) → `None` (B-tree default), PGM501 with GIN on FK columns → still fires, PGM501 with B-tree → does not fire.

**Complexity:** Small-to-medium | **Rules:** PGM501 (accuracy)

---

### 8.6 EXCLUDE Constraint Columns

**Problem:** `TableConstraint::Exclude` and `ConstraintState::Exclude` store only `name: Option<String>` — the element list (columns + operators) is discarded. When a column referenced by an EXCLUDE constraint is dropped, `involves_column()` can't detect the relationship, so the constraint isn't removed. There is an existing TODO in replay.rs acknowledging this.

**Current state:** Parser maps `ConstrExclusion` → `Exclude { name }` (stops there). `ConstraintState::involves_column` returns `false` for EXCLUDE, so DROP COLUMN never removes it.

**Solution:**

| File | Change |
|------|--------|
| `src/parser/ir.rs` | Extend `TableConstraint::Exclude` with `columns: Vec<String>` (element column names) |
| `src/parser/pg_query.rs` | Extract column names from `Constraint.exclusions` (each is an `IndexElem`) |
| `src/catalog/types.rs` | Mirror `columns` to `ConstraintState::Exclude`, implement `involves_column` for it |
| `src/catalog/replay.rs` | DROP COLUMN: remove EXCLUDE if column is in element list. RENAME COLUMN: update column names |

**Tests:** CREATE TABLE with EXCLUDE → columns captured, DROP COLUMN in EXCLUDE → constraint removed, DROP unrelated column → constraint kept, RENAME COLUMN → element updated.

**Complexity:** Medium (most complex AST navigation of the 6 gaps) | **Rules:** None directly; fixes DROP COLUMN correctness for EXCLUDE

---

### Summary

| Gap | Complexity | Rules Affected | Priority |
|-----|-----------|----------------|----------|
| 8.1 VALIDATE CONSTRAINT | Small | PGM013, PGM014, PGM015 | Medium |
| 8.2 DROP CONSTRAINT (§7) | Small | PGM501, PGM502, PGM503 | Medium |
| 8.3 DROP NOT NULL | Small | PGM016, PGM503; enables PGM1022 | Medium |
| 8.4 SET/DROP DEFAULT | Small-Med | None active | Low |
| 8.5 Index access method | Small-Med | PGM501 | Low |
| 8.6 EXCLUDE columns | Medium | None; correctness fix | Low |

### Recommended Order

**Batch 1** — all three touch the same files (`ir.rs`, `pg_query.rs`, `replay.rs`) with the same pattern (new `AlterTableAction` variant → parser mapping → replay handler). Implement together to minimize churn:
1. DROP CONSTRAINT (§7)
2. VALIDATE CONSTRAINT
3. DROP NOT NULL

**Batch 2** — independent, lower priority:
4. Index access method
5. SET/DROP DEFAULT

**Deferred:**
6. EXCLUDE columns
