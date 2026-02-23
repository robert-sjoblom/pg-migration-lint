# Partitioned Table Support

## Context

pg-migration-lint currently has zero awareness of PostgreSQL table partitioning. Partitioned tables (`PARTITION BY`, `PARTITION OF`, `ATTACH PARTITION`, `DETACH PARTITION`) are parsed by pg_query but silently ignored — `CREATE TABLE ... PARTITION BY` is treated as a regular table, `CREATE TABLE ... PARTITION OF` loses the parent relationship, and ATTACH/DETACH fall through to `AlterTableAction::Other`. This causes:

- **False positives**: PGM001 fires on `CREATE INDEX` for partitioned tables, but `CONCURRENTLY` is not even supported there — the safe pattern is completely different (CREATE INDEX ON ONLY + CONCURRENTLY per child + ATTACH).
- **False negatives**: No rules exist for dangerous partition operations (DETACH without CONCURRENTLY, ATTACH without pre-validated CHECK).
- **Catalog blindness**: The catalog doesn't track parent/child relationships, partition method, or partition columns, so no rule can reason about partition-specific behavior.

This plan designs the full partition support across three implementation passes.

---

## Implementation Passes

| Pass | Scope | Ships findings? |
|------|-------|-----------------|
| **Pass 1** | Foundation: IR, catalog, parser, replay | No |
| **Pass 2** | Update affected existing rules to be partition-aware | Yes (fewer FPs) |
| **Pass 3** (future) | Implement proposed rules PGM1004, PGM1005, etc. | Yes (new findings) |

---

## Pass 1 — Foundation

### 1.1 IR Changes (`src/parser/ir.rs`)

**Extend `CreateTable`:**
```rust
pub struct CreateTable {
    // ... existing fields ...
    /// Present when `CREATE TABLE ... PARTITION BY (RANGE|LIST|HASH)`.
    pub partition_by: Option<PartitionBy>,
    /// Present when `CREATE TABLE child PARTITION OF parent FOR VALUES ...`.
    /// Contains the parent table name. The child inherits the parent's columns.
    pub partition_of: Option<QualifiedName>,
}
```

**New type `PartitionBy`:**
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionBy {
    pub strategy: PartitionStrategy,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionStrategy {
    Range,
    List,
    Hash,
}
```

**New `AlterTableAction` variants:**
```rust
pub enum AlterTableAction {
    // ... existing variants ...
    AttachPartition {
        /// The partition (child) being attached.
        child: QualifiedName,
    },
    DetachPartition {
        /// The partition (child) being detached.
        child: QualifiedName,
        concurrent: bool,
    },
}
```

> Design note: ATTACH/DETACH are `AlterTableAction` variants (not top-level `IrNode`s) because pg_query emits them as `AlterTableCmd` inside `AlterTableStmt`. The parent table name comes from the `AlterTable.name` field. This matches how we model other ALTER TABLE subcommands.

**Update test builders** (`#[cfg(test)]` impls):
- `CreateTable::test()` gets `partition_by: None, partition_of: None`
- Add `.with_partition_by()` and `.with_partition_of()` builder methods

### 1.2 Catalog Changes (`src/catalog/types.rs`)

**Extend `TableState`:**
```rust
pub struct TableState {
    // ... existing fields ...
    /// True if this table was created with `PARTITION BY`.
    pub is_partitioned: bool,
    /// Partition strategy and columns, if partitioned.
    pub partition_by: Option<PartitionByInfo>,
    /// If this table is a partition child, the catalog key of the parent.
    pub parent_table: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PartitionByInfo {
    pub strategy: PartitionStrategy,  // reuse from IR
    pub columns: Vec<String>,
}
```

**Extend `Catalog`:**
```rust
pub struct Catalog {
    tables: HashMap<String, TableState>,
    index_to_table: HashMap<String, String>,
    /// Forward lookup: parent table key -> list of child partition keys.
    partition_children: HashMap<String, Vec<String>>,
}
```

**New methods on `Catalog`:**
- `pub fn get_partition_children(&self, parent_key: &str) -> &[String]`
- `pub fn is_partition_child(&self, table_key: &str) -> bool`
- `pub fn attach_partition(&mut self, parent_key: &str, child_key: &str)`
- `pub fn detach_partition(&mut self, parent_key: &str, child_key: &str)`

**Update `CatalogBuilder` (`src/catalog/builder.rs`):**
- Add `.partitioned_by(strategy, columns)` method on table builder
- Add `.partition_of(parent_key)` method on table builder
- Maintain `partition_children` map during building

### 1.3 Parser Changes (`src/parser/pg_query.rs`)

**`convert_create_table`** — extract partition info from `CreateStmt`:
- `create.partspec: Option<PartitionSpec>` -> `partition_by` field
  - `PartitionSpec.strategy` maps to our `PartitionStrategy` enum
  - `PartitionSpec.part_params` contains partition column names (extract from `PartitionElem` nodes)
- `create.inh_relations` + `create.partbound` -> `partition_of` field
  - When `partbound` is `Some`, this is a `CREATE TABLE ... PARTITION OF`
  - The parent name comes from `inh_relations[0]` (a `RangeVar`)
  - Columns come from the parent, not the child statement

**`convert_alter_table_cmd`** — handle new subtype variants:
- `AlterTableType::AtAttachPartition`:
  - `cmd.def` contains a `PartitionCmd` node
  - Extract child table from `PartitionCmd.name` (a `RangeVar`)
  - Return `AlterTableAction::AttachPartition { child }`
- `AlterTableType::AtDetachPartition`:
  - `cmd.def` contains a `PartitionCmd` node
  - Extract child table from `PartitionCmd.name`
  - Extract `concurrent` flag from `PartitionCmd.concurrent`
  - Return `AlterTableAction::DetachPartition { child, concurrent }`

**pg_query protobuf types used:**
- `PartitionSpec { strategy: PartitionStrategy, part_params: Vec<Node> }` — strategy is Range/List/Hash enum
- `PartitionCmd { name: Option<RangeVar>, bound: Option<PartitionBoundSpec>, concurrent: bool }`
- `PartitionBoundSpec` — present on `CreateStmt.partbound` for PARTITION OF

### 1.4 Normalize Changes (`src/normalize.rs`)

The normalizer walks all `IrNode` variants and calls `set_default_schema()` on unqualified names. Needs updates for:
- `CreateTable.partition_of` — normalize the parent reference
- `AlterTableAction::AttachPartition { child }` — normalize child name
- `AlterTableAction::DetachPartition { child, .. }` — normalize child name

### 1.5 Replay Changes (`src/catalog/replay.rs`)

**`apply_create_table`:**
- When `partition_by` is `Some`: set `table.is_partitioned = true` and store `PartitionByInfo`
- When `partition_of` is `Some`:
  - Set `table.parent_table = Some(parent_key)`
  - Register child in `catalog.partition_children` map
  - Copy columns from parent if child's column list is empty (PARTITION OF inherits columns)

**`apply_alter_table`** — new match arms:
- `AlterTableAction::AttachPartition { child }`:
  - `catalog.attach_partition(parent_key, child_key)`
  - Set `child.parent_table = Some(parent_key)`
- `AlterTableAction::DetachPartition { child, .. }`:
  - `catalog.detach_partition(parent_key, child_key)`
  - Clear `child.parent_table`

**`apply_drop_table`:**
- When dropping a partitioned parent: also clean up `partition_children` entries
- When dropping a child: remove from parent's `partition_children` list

### 1.6 Tests

- **Unit tests** in `src/parser/pg_query.rs`: parse `CREATE TABLE ... PARTITION BY RANGE (col)`, `CREATE TABLE ... PARTITION OF parent FOR VALUES FROM (...) TO (...)`, `ALTER TABLE ... ATTACH/DETACH PARTITION`
- **Replay tests** in `src/catalog/replay.rs`: partition_children tracking, attach/detach, drop cascading cleanup, PARTITION OF column inheritance
- **CatalogBuilder tests**: new builder methods work correctly

---

## Pass 2 — Existing Rules Become Partition-Aware

Rules that need changes (with catalog partition info available from Pass 1):

### PGM001 — CREATE INDEX without CONCURRENTLY
**Current**: Fires when `CREATE INDEX` on existing table lacks `CONCURRENTLY`.
**Problem**: `CREATE INDEX CONCURRENTLY` is **not supported** on partitioned tables. The safe pattern is:
1. `CREATE INDEX ON ONLY parent (...)`
2. `CREATE INDEX CONCURRENTLY` on each child
3. `ALTER INDEX parent_idx ATTACH PARTITION child_idx`

**Change**: When the target table `is_partitioned`, suppress the current finding. Optionally emit a different message suggesting the ON ONLY + per-child CONCURRENTLY pattern. (This might warrant a new rule ID or a variant message — TBD during implementation.)

### PGM002 — DROP INDEX without CONCURRENTLY
**Current**: Fires when `DROP INDEX` on existing table lacks `CONCURRENTLY`.
**Change**: Similar consideration — dropping an index from a partitioned parent may behave differently. Verify behavior and adjust message if needed.

### PGM501 — FK without covering index
**Current**: Checks if the FK source table has a covering index.
**Change**: If the table is a partition child, indexes may be inherited from the parent. Check parent's indexes too via `catalog.get_partition_children` / `parent_table`.

### PGM502 — Table without primary key
**Current**: Fires on tables without `has_primary_key`.
**Change**: Partition children inherit PK from parent. If `parent_table` is `Some` and parent `has_primary_key`, suppress finding.

### PGM503 — UNIQUE NOT NULL instead of PK
**Change**: Similar to PGM502 — partition children inherit constraints from parent.

### PGM016 — ADD PRIMARY KEY without prior UNIQUE index
**Change**: On partitioned tables, PK must include partition columns. This is enforced by PostgreSQL itself, but the linter message could be improved.

### PGM401/402 — Idempotency guards
**Change**: Verify these rules handle `CREATE TABLE ... PARTITION OF` and `CREATE INDEX ON ONLY` correctly.

### Audit checklist (rules to verify but likely no changes needed):
- PGM003 (CONCURRENTLY inside transaction) — fine as-is
- PGM006-PGM015 (column-level DDL) — fine, column operations on partitioned tables behave the same
- PGM101-106 (type anti-patterns) — fine as-is
- PGM201-204 (destructive ops) — fine, DROP/TRUNCATE on partitions is normal
- PGM301-303 (DML in migrations) — fine as-is
- PGM504-506 (rename, unlogged) — fine as-is

---

## Pass 3 (Future) — New Partition Rules

These are already specified in `docs/PROPOSED_RULES.md`:

### PGM1004 -> PGM019 — DETACH PARTITION without CONCURRENTLY
- CRITICAL severity
- Fires on `AlterTableAction::DetachPartition { concurrent: false }` where parent exists in `catalog_before`
- IR and catalog support from Pass 1 makes this straightforward

### PGM1005 -> PGM020 — ATTACH PARTITION without pre-validated CHECK
- MAJOR severity
- Fires when child has no CHECK constraints in `catalog_before`
- Checks `catalog_before.get_table(child_key).constraints` for any `ConstraintState::Check`

### Additional future rules to consider:
- CREATE INDEX on partitioned table without the ON ONLY pattern (related to PGM001 partition handling)
- INHERITS-based partitioning anti-pattern (needs `CreateStmt.inh_relations` without `partbound`)

---

## Files Modified

### Pass 1
| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `PartitionBy`, `PartitionStrategy`, extend `CreateTable`, extend `AlterTableAction` |
| `src/parser/pg_query.rs` | Extract partition info from `CreateStmt`, handle `AtAttachPartition`/`AtDetachPartition` |
| `src/catalog/types.rs` | Add partition fields to `TableState`, `PartitionByInfo`, extend `Catalog` |
| `src/catalog/builder.rs` | Add partition builder methods |
| `src/catalog/replay.rs` | Handle partition in create/alter/drop |
| `src/normalize.rs` | Normalize new `QualifiedName` fields |

### Pass 2
| File | Change |
|------|--------|
| `src/rules/pgm001.rs` | Suppress/change for partitioned tables |
| `src/rules/pgm002.rs` | Verify/adjust for partitioned tables |
| `src/rules/pgm501.rs` | Check parent indexes for partition children |
| `src/rules/pgm502.rs` | Suppress for partition children with parent PK |
| `src/rules/pgm503.rs` | Suppress for partition children |
| Other rule files | Audit (likely no changes) |

---

## Verification

### Pass 1
```bash
cargo test                    # All existing tests still pass
cargo clippy --all-targets    # No warnings
```
- New unit tests for parser: partition DDL -> correct IR
- New unit tests for replay: partition_children tracking, attach/detach
- New CatalogBuilder tests: `.partitioned_by()`, `.partition_of()`

### Pass 2
- New/updated rule tests with partitioned table fixtures
- Integration test fixture repo with partitioned tables
- Verify PGM001 doesn't fire on `CREATE INDEX` targeting partitioned parent
- Verify PGM502 doesn't fire on partition children

---

## PostgreSQL Reference

Key behaviors verified during design:

- **CREATE INDEX on partitioned parent** (no ONLY): Recursively creates indexes on all existing partitions and future ones.
- **CREATE INDEX CONCURRENTLY on partitioned parent**: **Not supported.** PostgreSQL rejects it.
- **CREATE INDEX ON ONLY parent**: Creates an invalid index on the parent only. Must manually attach child indexes via `ALTER INDEX ... ATTACH PARTITION`.
- **DETACH PARTITION CONCURRENTLY**: PostgreSQL 14+. Uses `SHARE UPDATE EXCLUSIVE` lock instead of `ACCESS EXCLUSIVE`.
- **ATTACH PARTITION**: If child has a validated CHECK constraint implying the partition bound, PostgreSQL skips the full-table scan.

Sources:
- https://www.postgresql.org/docs/current/sql-createindex.html
- https://www.postgresql.org/docs/current/ddl-partitioning.html
