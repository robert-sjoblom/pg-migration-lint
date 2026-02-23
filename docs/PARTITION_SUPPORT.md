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

> Type placement: `PartitionStrategy` is defined in `src/parser/ir.rs` and reused by `PartitionByInfo` in `src/catalog/types.rs`. This creates a `catalog -> parser` dependency. This is acceptable — the catalog already depends on the parser crate for `QualifiedName` and other IR types. If this coupling becomes problematic later (e.g., if catalog needs to be an independent crate), extract `PartitionStrategy` into a shared types module. For now, reuse is the simpler choice.

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

**Not modeled in Pass 1: `ALTER INDEX ... ATTACH PARTITION`**

`ALTER INDEX parent_idx ATTACH PARTITION child_idx` is emitted by pg_query as an `AlterTableStmt` with `relkind = OBJECT_INDEX` and subtype `AT_AttachPartition`. This is a distinct statement from `ALTER TABLE ... ATTACH PARTITION` — it attaches a child index to a parent index, not a child table to a parent table. Pass 1 does not model this because no existing rule needs it. However, Pass 3's PGM001 partition-aware indexing pattern (ON ONLY + per-child CONCURRENTLY + ATTACH INDEX) will require it. At that point, add either:
- A new `IrNode::AlterIndex(AlterIndexAction::AttachPartition { parent_index, child_index })` variant, or
- Detection within the existing `AlterTable` path by checking `relkind`

The choice depends on whether any other `ALTER INDEX` operations are worth modeling at that time. For now, `ALTER INDEX ... ATTACH PARTITION` falls through to `AlterTableAction::Other` / `Ignored`, which is correct — no rule inspects it.

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
  - If the parent does not exist in the catalog: **set `parent_table = Some(parent_key)` anyway** but skip column inheritance and `partition_children` registration. This legitimately happens when the parent was created in a previous migration outside the linter's scope. The child is created as a standalone table with `parent_table` pointing to a nonexistent entry. Rules must check `catalog.get_table(parent_key).is_some()` before using the parent. This differs from the ATTACH missing-child policy (where we skip silently) because here we have the child in hand — recording `parent_table` costs nothing and lets rules like PGM501 correctly suppress findings even when the parent's details are unknown.
  - If the parent exists: set `table.parent_table = Some(parent_key)`, register child in `catalog.partition_children` map.
  - **Column inheritance** (when parent exists): Snapshot the parent's current columns into the child's `columns` vec. This is an intentional simplification — in PostgreSQL, partition children don't have independent column definitions and share the parent's definition dynamically. Our snapshot goes stale if someone later runs `ALTER TABLE parent ADD COLUMN` without also touching the child. This is acceptable because: (a) PostgreSQL itself propagates column changes to children, so any rule checking the child after the ALTER will see the column added to the parent's `TableState`, and rules that need the child's actual column list (PGM501 FK index checks) delegate to the parent via `parent_table` anyway; (b) the snapshot is only used for rules that inspect the child in the same migration unit where it was created. If this proves insufficient, the fix is to make column lookups on children delegate to the parent — but that adds complexity with no current consumer.

**`apply_alter_table`** — new match arms:
- `AlterTableAction::AttachPartition { child }`:
  - If the child does not exist in the catalog: **skip silently**. This legitimately happens when the child was created externally, in a previous migration outside the linter's scope, or via dynamic SQL. Log at debug level for diagnostics but do not create a stub entry — a stub with no columns/indexes would be worse than absent, since rules would see it and draw wrong conclusions. The `parent_table` field on the child remains unset, so partition-aware rules will not apply (conservative: they'll treat it as a standalone table).
  - If the child exists: `catalog.attach_partition(parent_key, child_key)` and set `child.parent_table = Some(parent_key)`
- `AlterTableAction::DetachPartition { child, .. }`:
  - `catalog.detach_partition(parent_key, child_key)`
  - Clear `child.parent_table` (if child exists in catalog; skip silently if not)

**`apply_drop_table`:**

In PostgreSQL, `DROP TABLE parent` fails if children exist (unless `CASCADE` is specified), and `DROP TABLE parent CASCADE` drops all children too. The IR already tracks `DropTable.cascade`.

- When `cascade` is `true` and the dropped table `is_partitioned`:
  - **Recursively** remove the entire partition subtree. A child may itself be a partitioned table (sub-partitioning, PG11+), so single-level cleanup is insufficient. Walk `partition_children` depth-first: for each child, if that child has its own `partition_children` entries, recurse into those first, then remove the child from the catalog. This ensures grandchildren (and deeper) are cleaned up, their `parent_table` references don't go stale, and no orphaned `partition_children` entries remain.
  - **Implementation note**: Use a visited set during the recursive walk. PostgreSQL does not allow partition cycles, but the catalog can contain invalid states from malformed migrations. A cycle in `partition_children` without a visited set would loop indefinitely. The visited set costs nothing and makes the implementation unconditionally safe.
  - Remove the `partition_children` entry for the parent
- When `cascade` is `false` and the dropped table `is_partitioned`:
  - In practice, PostgreSQL would reject this if children exist. But the linter doesn't enforce DDL validity — it replays what the migration says. Clean up the `partition_children` entry for the parent. Leave children in the catalog with a stale `parent_table` reference (the parent no longer exists). Rules should tolerate `parent_table` pointing to a nonexistent catalog entry — check `catalog.get_table(parent_key).is_some()` before using it.
  - **Interaction with PGM502/PGM503**: Children with stale `parent_table` (parent dropped without CASCADE) hit the "parent not in catalog" suppression path — PGM502/PGM503 will suppress findings on these orphaned children. This is acceptable: if someone dropped the partitioned parent without CASCADE, the migration is already invalid DDL that PostgreSQL would reject. The linter is not responsible for catching impossible catalog states produced by broken migrations.
- When dropping a child (has `parent_table` set): remove from parent's `partition_children` list, regardless of CASCADE.

### 1.6 Additional IR Change: `CreateIndex.only`

Pass 2's PGM001 handling needs to distinguish `CREATE INDEX ON table` from `CREATE INDEX ON ONLY table`. Add to `CreateIndex`:
```rust
pub struct CreateIndex {
    // ... existing fields ...
    /// True when `CREATE INDEX ON ONLY table`. Creates an invalid parent-only index.
    pub only: bool,
}
```
This is extracted from `IndexStmt.relation.inh` in pg_query: `only = !relation.inh`. Verified via spike test (`tests/pg_query_spike.rs::spike_index_rangevar_inh`): pg_query explicitly sets `inh = true` for normal `CREATE INDEX` and `inh = false` for `CREATE INDEX ON ONLY`. The protobuf `bool` default of `false` is not a problem — pg_query always sets `inh` explicitly, so the zero-value case doesn't arise. The IR field `only` defaults to `false` (normal index), which is correct. Include this in Pass 1 so the IR is complete before rules consume it.

### 1.7 Tests

- **Parser unit tests** in `src/parser/pg_query.rs`:
  - Parse `CREATE TABLE ... PARTITION BY RANGE (col)` → correct `partition_by`
  - Parse `CREATE TABLE ... PARTITION BY LIST (col)` → correct strategy
  - Parse `CREATE TABLE ... PARTITION BY HASH (col)` → correct strategy
  - Parse `CREATE TABLE child PARTITION OF parent FOR VALUES FROM (...) TO (...)` → correct `partition_of`
  - Parse `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...` → `AttachPartition`
  - Parse `ALTER TABLE parent DETACH PARTITION child` → `DetachPartition { concurrent: false }`
  - Parse `ALTER TABLE parent DETACH PARTITION child CONCURRENTLY` → `DetachPartition { concurrent: true }`
  - Parse `CREATE INDEX ON ONLY parent (col)` → `CreateIndex { only: true }`
- **Replay unit tests** in `src/catalog/replay.rs`:
  - `partition_children` tracking: create parent, create PARTITION OF child → parent has child in `partition_children`
  - Attach: `ATTACH PARTITION` on existing child → updates both `partition_children` and `parent_table`
  - Attach missing child: `ATTACH PARTITION` when child not in catalog → no-op, no panic
  - Detach: `DETACH PARTITION` → removes from `partition_children`, clears `parent_table`
  - Detach missing child: `DETACH PARTITION` when child not in catalog → no-op, no panic
  - **Drop parent CASCADE**: drop partitioned parent with `cascade: true` → children removed from catalog entirely, `partition_children` entry removed
  - **Drop parent without CASCADE**: drop partitioned parent with `cascade: false` → `partition_children` entry removed, children remain in catalog with stale `parent_table`
  - **Drop parent CASCADE with multiple children**: verify all children are removed, not just the first
  - **Drop parent CASCADE with sub-partitioning**: parent → child (itself partitioned) → grandchild; drop parent CASCADE removes all three levels, no stale `partition_children` or `parent_table` entries remain
  - **Drop child cleanup**: drop partition child → removed from parent's `partition_children` list
  - **PARTITION OF with parent in catalog**: child gets parent's columns, `partition_children` updated, `parent_table` set
  - **PARTITION OF with parent not in catalog**: child created with `parent_table` set but no column inheritance, no `partition_children` entry (parent unknown)
- **CatalogBuilder tests**: `.partitioned_by()` and `.partition_of()` methods work correctly, `partition_children` map is consistent

---

## Pass 2 — Existing Rules Become Partition-Aware

Rules that need changes (with catalog partition info available from Pass 1):

### PGM001 — CREATE INDEX without CONCURRENTLY
**Current**: Fires when `CREATE INDEX` on existing table lacks `CONCURRENTLY`.
**Problem**: `CREATE INDEX CONCURRENTLY` is **not supported** on partitioned tables. The safe pattern is:
1. `CREATE INDEX ON ONLY parent (...)`
2. `CREATE INDEX CONCURRENTLY` on each child
3. `ALTER INDEX parent_idx ATTACH PARTITION child_idx`

**Change**: Same rule ID (PGM001), distinct message for partitioned tables. When the target table `is_partitioned`:
- **Suppress** the standard "should use CONCURRENTLY" finding (it's wrong — PG rejects it).
- **Emit** a new finding at the same severity (CRITICAL) with message: `CREATE INDEX on partitioned table '{name}' will lock all partitions. Use CREATE INDEX ON ONLY, then CREATE INDEX CONCURRENTLY on each partition, then ALTER INDEX ... ATTACH PARTITION.`
- The finding references the same rule ID because the root concern is identical (index creation holding locks on an existing table). A separate rule ID is not warranted — the distinction is in the message and remediation, not the category.
- If `CREATE INDEX ON ONLY` is used (detected via the `only` field on `CreateIndex` IR — not yet present, must be added in Pass 1): suppress PGM001 entirely, since `ON ONLY` creates an invalid parent-only index with no lock on children. Note: PGM001 already only fires when the target table exists in `catalog_before` (new tables are not the dangerous case), so the `only` suppression is only reachable for pre-existing partitioned parents. For new tables, PGM001 wouldn't fire regardless of `only`. The suppression is technically redundant in that case but harmless — the implementer should check `only` inside the existing `catalog_before` guard, not as an independent early-return.

### PGM002 — DROP INDEX without CONCURRENTLY
**Current**: Fires when `DROP INDEX` on existing table lacks `CONCURRENTLY`.

**PostgreSQL behavior**: `DROP INDEX` on a parent partition index drops the entire index tree (parent + all child indexes) and takes `ACCESS EXCLUSIVE` lock on every partition. `DROP INDEX CONCURRENTLY` is supported here and drops each child's index concurrently. For indexes created with `ON ONLY` (invalid parent-only indexes), `DROP INDEX` without `CONCURRENTLY` is safe since the index only exists on the parent and no child locks are taken.

**Change**: PGM002 fires correctly for partitioned tables — `DROP INDEX CONCURRENTLY` is the right advice. No suppression needed. However, if the index is on a parent and was created with `ON ONLY` (the invalid-index pattern), the finding is a false positive. Detecting this requires tracking whether an index was created with `ON ONLY` in the catalog's `IndexState`, which is out of scope for Pass 2. **Conclusion**: No changes to PGM002 in Pass 2. The false positive on `ON ONLY` parent indexes is a known limitation, documentable in the rule's `explain()` text.

### PGM501 — FK without covering index

**Current**: Checks if the FK source table has a covering index on the FK columns.

**PostgreSQL partition behavior**:
- **FK from partitioned table** (PG11+): The FK constraint is declared on the parent but enforced per-partition. A covering index must exist on each partition individually — an `ON ONLY` index on the parent is invalid and does not serve this purpose.
- **FK from partition child**: The FK is inherited from the parent declaration. The covering index situation depends on how indexes were created — via recursive `CREATE INDEX` on parent (creates real indexes per child), or via the `ON ONLY` + per-child `CONCURRENTLY` pattern (child indexes exist but parent index is invalid).

**Problem**: The catalog, post-Pass 1, only tracks indexes that were explicitly created in the migration sequence. It has no visibility into indexes on pre-existing partitions, and cannot distinguish between a valid recursive index and an invalid `ON ONLY` parent index. Walking up to check the parent's indexes (as previously specified) would check the wrong place — the parent's index may be invalid.

**Change**: When the FK source table `is_partitioned` or has `parent_table` set (is a partition child):
- **Suppress** the standard PGM501 finding entirely (no CRITICAL/MAJOR finding emitted).
- **Do not emit an INFO finding.** An INFO that fires on every FK involving a partitioned table, with no actionable fix other than "check manually," is pure noise. Users cannot resolve it, so it accumulates as permanent warnings in CI output. Instead, document the limitation in PGM501's `explain()` text: _"PGM501 does not analyze foreign keys on partitioned tables or partition children. Index coverage for partitioned FK sources must be verified per-partition manually, as the linter cannot reliably determine index state across partitions."_
- Users who want to be reminded can use `--explain PGM501` to see this note. This keeps the rule output clean while the limitation is discoverable.
- This is the conservative choice. Attempting to infer correctness from partial catalog state will produce unreliable results (both false positives and false negatives). Full partition enumeration would require tracking every child's index state independently, which is out of scope.

### PGM502 — Table without primary key

**Current**: Fires on tables without `has_primary_key`.

**PostgreSQL partition behavior**: Partition children structurally inherit the PK from the parent. The PK constraint on a partitioned parent must include all partition key columns (PostgreSQL enforces this). The backing index for this constraint is per-partition — the parent-level index is marked invalid, while each partition has a real, valid local index. From a correctness standpoint, every partition does have a PK.

**Change**: If `parent_table` is `Some`, suppress the finding in these cases:
- Parent exists in catalog and `has_primary_key`: suppress. PK inheritance is structural and automatic in declarative partitioning.
- Parent exists in catalog and does **not** have PK: **do not suppress**. The parent genuinely lacks a PK, so the child lacks one too. Fire the finding on the child.
- **Parent not in catalog** (`parent_table` is `Some` but `catalog.get_table(parent_key)` returns `None`): **suppress**. We know the table is a partition child (the DDL said `PARTITION OF`), we just can't confirm the PK because the parent was created outside the linter's scope. Firing the finding would false-positive on every migration that creates a partition of a pre-existing parent — which is the common case in incremental CI linting. The conservative choice here is to trust that a production partitioned table has a PK, since PostgreSQL enforces PK-must-include-partition-columns structurally and most schemas define one.

The fact that the parent-level PK backing index is technically invalid is a PostgreSQL implementation detail, not a correctness concern for PGM502's purpose (ensuring tables have a unique row identifier).

For partitioned parents themselves: no change needed. If someone creates a partitioned table without a PK, PGM502 should fire — the lack of PK is intentional to flag regardless of partitioning.

### PGM503 — UNIQUE NOT NULL instead of PK

**Change**: Same three-case logic as PGM502:
- Parent in catalog with PK: suppress.
- Parent in catalog without PK: do not suppress.
- Parent not in catalog: suppress (same rationale — trust that production parents have a PK; firing would false-positive on the common incremental-CI case).

### PGM016 — ADD PRIMARY KEY without prior UNIQUE index
**Change**: No change needed. PGM016 fires when `ADD PRIMARY KEY` is used without a prior `UNIQUE` index on the same columns. On partitioned tables, the PK must include partition key columns — but this is enforced by PostgreSQL itself at DDL time. A migration with a non-conforming PK will fail before it reaches the linter. PGM016's existing logic (check for prior UNIQUE index) applies identically to partitioned and non-partitioned tables.

### PGM401 — Missing IF EXISTS on DROP TABLE / DROP INDEX
**Change**: No change needed. `DROP TABLE IF EXISTS` and `DROP INDEX IF EXISTS` apply identically regardless of partitioning. `CREATE TABLE ... PARTITION OF` does not change DROP behavior.

### PGM402 — Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX
**Change**: No change needed. `CREATE TABLE ... PARTITION OF` supports `IF NOT EXISTS` the same way as regular `CREATE TABLE`. `CREATE INDEX ON ONLY` is still a `CREATE INDEX` and PGM402's `if_not_exists` check works as-is. Verified: the parser already extracts `if_not_exists` from `IndexStmt`, and the `only` field (added in Pass 1) is orthogonal to idempotency.

### Audit checklist (rules to verify but likely no changes needed):
- PGM003 (CONCURRENTLY inside transaction) — fine as-is
- PGM006-PGM015 (column-level DDL) — fine, column operations on partitioned tables behave the same
- PGM101-106 (type anti-patterns) — fine as-is
- PGM201-204 (destructive ops) — fine, DROP/TRUNCATE on partitions is normal
- PGM301-303 (DML in migrations) — fine as-is
- PGM504-506 (rename, unlogged) — fine as-is

### Out of scope: INHERITS-based partitioning

`CREATE TABLE child INHERITS (parent)` (without `PARTITION OF`) is the legacy pre-PG10 inheritance mechanism. pg_query emits this as `CreateStmt.inh_relations` with no `partbound`. This causes the same catalog blindness as partition children for PGM502/PGM503 — the child appears to lack a PK even though it inherits one.

**Decision**: INHERITS is **out of scope for Pass 1 and Pass 2**. Rationale:
- INHERITS-based partitioning is deprecated in favor of declarative partitioning (PG10+).
- The inheritance semantics differ significantly (no automatic index/constraint propagation, no partition pruning).
- Modeling it correctly requires tracking a different kind of parent-child relationship with different rules about what gets inherited.
- The false positive rate on real-world codebases is low — most INHERITS usage is legacy and won't appear in new migrations.

Pass 1 will **not** detect `inh_relations` without `partbound`. If this becomes a problem, it can be addressed in a future pass by setting a `inherits_from: Option<String>` on `TableState` and teaching PGM502/PGM503 to check it.

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
- **Limitation**: The relevant CHECK is specifically one that implies the partition bound (e.g., `CHECK (created_at >= '2024-01-01' AND created_at < '2024-02-01')` for a RANGE partition on `created_at`). An unrelated CHECK (e.g., `CHECK (amount > 0)`) does not help — PostgreSQL still performs a full-table scan on ATTACH. However, analyzing whether a CHECK expression implies the partition bound requires expression comparison against `PartitionBoundSpec`, which is significantly complex. **Decision**: Use presence of any CHECK as a proxy. If the child has at least one CHECK, suppress. This is conservative in the wrong direction (may miss findings) but avoids false positives. The `explain()` text should note that the CHECK must match the partition bound to be effective, so users understand what PostgreSQL actually requires.

### Additional future rules to consider:
- CREATE INDEX on partitioned table without the ON ONLY pattern (related to PGM001 partition handling)
- INHERITS-based partitioning anti-pattern (needs `CreateStmt.inh_relations` without `partbound`)

---

## Files Modified

### Pass 1
| File | Change |
|------|--------|
| `src/parser/ir.rs` | Add `PartitionBy`, `PartitionStrategy`, extend `CreateTable`, extend `AlterTableAction`, add `CreateIndex.only` |
| `src/parser/pg_query.rs` | Extract partition info from `CreateStmt`, handle `AtAttachPartition`/`AtDetachPartition` |
| `src/catalog/types.rs` | Add partition fields to `TableState`, `PartitionByInfo`, extend `Catalog` |
| `src/catalog/builder.rs` | Add partition builder methods |
| `src/catalog/replay.rs` | Handle partition in create/alter/drop |
| `src/normalize.rs` | Normalize new `QualifiedName` fields |

### Pass 2
| File | Change |
|------|--------|
| `src/rules/pgm001.rs` | Suppress standard finding + emit partition-specific message for `is_partitioned`; suppress entirely for `only` |
| `src/rules/pgm501.rs` | Suppress finding when FK source is partitioned or partition child; document limitation in `explain()` |
| `src/rules/pgm502.rs` | Suppress for partition children (parent has PK, or parent not in catalog); fire when parent in catalog without PK |
| `src/rules/pgm503.rs` | Suppress for partition children (parent has PK, or parent not in catalog); fire when parent in catalog without PK |

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
- **PGM001**: Verify standard finding suppressed for `is_partitioned` target; verify partition-specific CRITICAL emitted instead; verify full suppression for `CREATE INDEX ON ONLY`; verify normal behavior unchanged for non-partitioned tables
- **PGM501**: Verify finding suppressed (no output) when FK source `is_partitioned`; verify finding suppressed when FK source is partition child (`parent_table` set); verify normal MAJOR finding still fires on non-partitioned FK source without covering index; verify `explain()` text documents the partition limitation
- **PGM502**: Verify finding suppressed for partition children when parent `has_primary_key`; verify finding still fires on partition children when parent lacks PK; verify finding still fires on partitioned parents without PK; verify finding suppressed when child has `parent_table` but parent is not in catalog (PARTITION OF unknown parent — the common incremental-CI case)
- **PGM503**: Verify finding suppressed for partition children when parent `has_primary_key`; verify finding still fires on partition children when parent lacks PK; verify finding suppressed when child has `parent_table` but parent is not in catalog

---

## PostgreSQL Reference

Key behaviors verified during design:

- **CREATE INDEX on partitioned parent** (no ONLY): Recursively creates indexes on all existing partitions and future ones. Takes `ACCESS EXCLUSIVE` lock on each partition.
- **CREATE INDEX CONCURRENTLY on partitioned parent**: **Not supported.** PostgreSQL rejects it.
- **CREATE INDEX ON ONLY parent**: Creates an invalid index on the parent only. Must manually attach child indexes via `ALTER INDEX ... ATTACH PARTITION`. No lock on children.
- **DROP INDEX on partitioned parent index**: Drops the entire index tree (parent + all child indexes). Takes `ACCESS EXCLUSIVE` lock on every partition.
- **DROP INDEX CONCURRENTLY on partitioned parent index**: Supported. Drops each child's index concurrently.
- **DROP INDEX on ON-ONLY parent index**: Safe — the index only exists on the parent, no child locks taken.
- **ALTER INDEX parent_idx ATTACH PARTITION child_idx**: pg_query emits this as `AlterTableStmt` with `relkind = OBJECT_INDEX` and subtype `AT_AttachPartition`. This is distinct from `ALTER TABLE ... ATTACH PARTITION`.
- **DETACH PARTITION CONCURRENTLY**: PostgreSQL 14+. Uses `SHARE UPDATE EXCLUSIVE` lock instead of `ACCESS EXCLUSIVE`.
- **ATTACH PARTITION**: If child has a validated CHECK constraint implying the partition bound, PostgreSQL skips the full-table scan.
- **FK referencing a partitioned table** (PG12+): The referenced partitioned table must have a PK or UNIQUE constraint that includes all partition key columns. The backing index exists per-partition (invalid at parent level). Not supported before PG12.
- **FK on a partitioned table** (PG11+): The FK constraint is declared on the parent but enforced at the partition level. A covering index for the FK columns must exist per-partition — an `ON ONLY` parent index is invalid and does not serve. Not supported before PG11.
- **PK on partitioned tables**: PK must include all partition key columns (PostgreSQL enforces this structurally). The backing index is per-partition; the parent-level index is marked invalid. PK constraints are always inherited by partition children automatically.
- **INHERITS (legacy)**: `CREATE TABLE child INHERITS (parent)` does **not** propagate indexes or unique constraints to children. Only columns and CHECK constraints are inherited. This is fundamentally different from declarative partitioning.

Sources:
- https://www.postgresql.org/docs/current/sql-createindex.html
- https://www.postgresql.org/docs/current/sql-dropindex.html
- https://www.postgresql.org/docs/current/sql-alterindex.html
- https://www.postgresql.org/docs/current/ddl-partitioning.html
- https://www.postgresql.org/docs/current/ddl-inherit.html
