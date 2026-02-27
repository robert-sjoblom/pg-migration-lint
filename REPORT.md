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

#### Integer Primary Key Detection (already designed, unimplemented)
PK columns using `int4`/`int2` instead of `int8`/`bigint`. Max ~2.1B is routinely exhausted in high-write tables. Both squawk (`prefer-bigint-over-int`) and strong_migrations flag this. **Already fully designed in SPEC.md** — just needs implementation.

**Severity:** MAJOR | **Family:** 1xx (TypeAntiPattern) | **Effort:** Tier 1 (existing IR sufficient)

#### `ALTER TYPE ... ADD VALUE` in Transaction
Adding an enum value cannot be rolled back inside a transaction. If the migration fails partway, the enum value persists after rollback. Strong_migrations flags this.

**Severity:** CRITICAL | **Family:** 0xx (UnsafeDDL) | **Effort:** Tier 2 (new `IrNode::AlterEnum` variant)

#### `DROP NOT NULL` on Existing Table
Dropping a NOT NULL constraint silently allows NULLs where application code assumes non-NULL. Squawk has `ban-drop-not-null`.

**Severity:** WARNING | **Family:** 0xx | **Effort:** Tier 1 (match on existing `AlterTableAction::Other` or promote to new variant)

### 5.2 Should-Have — Medium Value, Medium Effort

| Rule | What It Detects | Severity | Effort |
|------|----------------|----------|--------|
| `VACUUM FULL` on existing table | ACCESS EXCLUSIVE lock, full table rewrite | CRITICAL | Tier 2 (new IR variant) |
| `REINDEX` without `CONCURRENTLY` | ACCESS EXCLUSIVE lock (PG 12+ has CONCURRENTLY) | CRITICAL | Tier 2 (new IR variant) |
| Duplicate/redundant indexes | Index whose columns are a prefix of another | WARNING | Tier 1 (catalog-only) |
| Transaction nesting (`BEGIN`/`COMMIT` in migration) | Nested transaction errors in transactional migrations | WARNING | Tier 2 (new IR variant) |

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
- `prefer-bigint-over-int` / `prefer-bigint-over-smallint` — **Gap: planned but not implemented**
- `ban-drop-not-null` — **Gap: proposed above**
- `ban-drop-database` — **Gap: low priority**
- `transaction-nesting` — **Gap: proposed above**
- `prefer-text-field` — Gap: deferred
- `ban-create-domain-with-constraint` — Gap: niche

**Rules strong_migrations has that pg-migration-lint does NOT:**
- Enum `ADD VALUE` in transaction — **Gap: proposed above**

**All other squawk and strong_migrations rules are covered** by existing pg-migration-lint rules.

---

## 6. Prioritized Action Items

### High (Architecture / New Rules)
2. **Implement integer PK rule** (already designed, highest-value gap)
3. **Implement `ALTER TYPE ADD VALUE` in transaction** rule
8. **Implement `DROP NOT NULL` rule**
11. **Add `DROP CONSTRAINT` to catalog replay**
15. Implement `VACUUM FULL` / `REINDEX CONCURRENTLY` rules

4. **Add `pub(crate)` discipline** across the crate
### Medium (DRY / Missing Rules)

### Low (Polish)
12. Replace glob re-exports with explicit re-exports

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
