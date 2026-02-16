# Fix PGM012/PGM021: Check USING INDEX instead of catalog index existence

## Context

PGM012 (ADD PRIMARY KEY) and PGM021 (ADD UNIQUE) currently check whether a unique index/constraint already exists in `catalog_before`. If it does, they don't fire. This is wrong: even if a matching unique index exists, `ADD CONSTRAINT ... UNIQUE (cols)` without `USING INDEX` will **always build a new index** under ACCESS EXCLUSIVE lock. The pre-existing index is never automatically reused. Only the explicit `USING INDEX idx_name` syntax avoids the lock.

Additionally, PGM012 has a subtlety: even with `USING INDEX`, PostgreSQL implicitly runs `ALTER COLUMN SET NOT NULL` on any nullable PK columns, which requires a full table scan under ACCESS EXCLUSIVE lock. PGM016 won't catch this because there's no explicit `SET NOT NULL` statement.

## Correct logic

### PGM021 (ADD UNIQUE) — existing tables only

| Scenario | Fire? | Message |
|---|---|---|
| No `USING INDEX` | Yes | full message (see Stage 2 §2) |
| `USING INDEX`, index not in either catalog | Yes | full message (see Stage 2 §2) |
| `USING INDEX`, index exists but not unique | Yes | full message (see Stage 2 §2) |
| `USING INDEX`, index exists and is unique | No | — |

### PGM012 (ADD PRIMARY KEY) — existing tables only

| Scenario | Fire? | Message |
|---|---|---|
| No `USING INDEX` | Yes | full message (see Stage 2 §3) |
| `USING INDEX`, index not in either catalog | Yes | full message (see Stage 2 §3) |
| `USING INDEX`, index exists but not unique | Yes | full message (see Stage 2 §3) |
| `USING INDEX`, index exists, unique, but PK columns nullable in `catalog_before` | Yes | full message (see Stage 2 §3) |
| `USING INDEX`, index exists, unique, columns NOT NULL | No | — |

## Important: Empty `columns` with `USING INDEX`

When PostgreSQL parses `ALTER TABLE t ADD PRIMARY KEY USING INDEX idx_foo`, the `keys` field in the protobuf is **empty** — columns come from the index definition, not the SQL text. This means:

- The constraint's `columns` vec will be `[]` whenever `using_index` is `Some`.
- Rules must **never** rely on `columns` when `using_index` is set. Instead, get columns from the referenced `IndexState.columns`.
- Error messages for the no-`USING INDEX` path can safely use `columns` (it will be populated).

---

## Stage 1 — Infrastructure (no behavior change)

**Goal:** Add the `using_index` field through IR, parser, and catalog. Fix all pattern matches. After this stage, everything compiles and all existing tests pass unchanged — `using_index` is `None` for all existing code paths.

### 1.1. IR: Add `using_index` field
**File:** `src/parser/ir.rs` (lines 259-279)

Add `using_index: Option<String>` to both variants:
```rust
PrimaryKey {
    columns: Vec<String>,
    using_index: Option<String>,  // NEW
},
Unique {
    name: Option<String>,
    columns: Vec<String>,
    using_index: Option<String>,  // NEW
},
```

### 1.2. Parser: Extract `con.indexname`
**File:** `src/parser/pg_query.rs`

In `convert_table_constraint()` (line 527):
- Read `con.indexname` from the protobuf (field exists at tag 17, type `String`)
- Set `using_index: optional_name(&con.indexname)` on both `PrimaryKey` and `Unique` variants
- For inline column constraints (CREATE TABLE), `indexname` will be empty → `None`

Also in `convert_column_def()` (lines 216, 232) where inline PK/Unique are constructed — add `using_index: None`.

### 1.3. Catalog helper: index lookup by name
**File:** `src/catalog/types.rs`

The catalog already has `table_for_index(name) -> Option<&str>` for reverse lookup. We also need to retrieve the `IndexState` to check `unique`. Add a helper:

```rust
/// Look up an index by name across all tables. Returns the IndexState if found.
pub fn get_index(&self, index_name: &str) -> Option<&IndexState>
```

This looks up the table via `table_for_index`, then finds the `IndexState` by name.

### 1.4. Catalog replay: Update pattern matches and USING INDEX semantics
**File:** `src/catalog/replay.rs`

All pattern matches on `TableConstraint::PrimaryKey` and `TableConstraint::Unique` need `..` or the new field. Key locations:
- Lines 66, 111, 129, 313, 342 — production code
- Lines 823, 1145, 1148, 1373, 1459 — test constructors

**Semantic change in `apply_table_constraint` (line 311):**

When `PrimaryKey { using_index: Some(idx_name), .. }`, PostgreSQL reuses the existing index rather than creating a new one. The replay should **not** push a synthetic `{table}_pkey` index in this case — the referenced index already exists in the catalog from its `CREATE INDEX` statement. Only create the synthetic index when `using_index` is `None`:

```rust
TableConstraint::PrimaryKey { columns, using_index } => {
    table.has_primary_key = true;
    table.constraints.push(ConstraintState::PrimaryKey {
        columns: columns.clone(),
    });
    // Only create a synthetic PK index when there's no USING INDEX.
    // With USING INDEX, the referenced index already exists in the catalog.
    if using_index.is_none() {
        table.indexes.push(IndexState {
            name: format!("{}_pkey", table.name),
            columns: columns.clone(),
            unique: true,
        });
    }
}
```

For `Unique`, the current replay doesn't create an index (correct — PostgreSQL creates one implicitly, but we don't track it), so no semantic change needed beyond pattern match updates.

### 1.5. Other pattern match sites

- `src/rules/pgm005.rs` (lines 141, 188, 191) — test constructors, add `using_index: None`
- `src/rules/pgm004.rs` (lines 179, 282) — test constructors, add `using_index: None`
- `src/parser/pg_query.rs` (lines 1072, 1091, 1152, 1646) — test assertions, use `..`
- `tests/matrix.rs` (lines 283, 568, 597, 600) — test constructors, add `using_index: None`

### 1.6. Parser tests for USING INDEX

Add tests that verify the new field is correctly extracted:
- `ALTER TABLE t ADD CONSTRAINT c UNIQUE USING INDEX idx_foo` → `using_index: Some("idx_foo")`, `columns: []`
- `ALTER TABLE t ADD PRIMARY KEY USING INDEX idx_foo` → `using_index: Some("idx_foo")`, `columns: []`
- `ALTER TABLE t ADD PRIMARY KEY (id)` → `using_index: None`, `columns: ["id"]` (existing behavior preserved)

### Stage 1 verification

```bash
cargo check                    # Compilation — no missing fields
cargo test                     # All existing tests pass unchanged
cargo clippy -- -D warnings    # No warnings
```

All existing tests must pass **without** `INSTA_UPDATE` — no snapshots should change in this stage.

---

## Stage 2 — Rule behavior change

**Goal:** Rewrite PGM012/PGM021 rule logic, clean up dead code, add fixture coverage. This is where findings actually change.

### 2.1. Add PGM021 to `all-rules` fixture and e2e assertion
**Files:**
- `tests/fixtures/repos/all-rules/migrations/V005__new_violations.sql` — append:
  ```sql
  -- PGM021: ADD UNIQUE without USING INDEX on existing table
  ALTER TABLE customers ADD CONSTRAINT uq_customers_email UNIQUE (email);
  ```
- `tests/e2e.rs` (line 716) — add `"PGM021"` to the expected-rules array.

### 2.2. Rule logic: PGM021 (`src/rules/pgm021.rs`)

Replace the `has_unique_covering` check. Message templates:

- **No USING INDEX:** `"ADD UNIQUE on existing table '{table}' without USING INDEX on column(s) [{columns}]. Create a unique index CONCURRENTLY first, then use ADD CONSTRAINT ... UNIQUE USING INDEX."`
- **Index not found:** `"ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': referenced index does not exist."`
- **Index not unique:** `"ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': referenced index is not UNIQUE."`

```rust
let AlterTableAction::AddConstraint(TableConstraint::Unique {
    columns, using_index, ..
}) = action else {
    return vec![];
};

// ... table_key, catalog_before lookup ...

match using_index {
    Some(idx_name) => {
        // Look up in catalog_before first, then catalog_after
        let idx = ctx.catalog_before.get_index(idx_name)
            .or_else(|| ctx.catalog_after.get_index(idx_name));
        match idx {
            None => fire("... referenced index does not exist ..."),
            Some(idx) if !idx.unique => fire("... referenced index is not UNIQUE ..."),
            Some(_) => return vec![],  // safe
        }
    }
    None => fire("... without USING INDEX ..."),
}
```

Also update `explain()` to describe the USING INDEX requirement and the validation tiers.

### 2.3. Rule logic: PGM012 (`src/rules/pgm012.rs`)

Same as PGM021 but with an additional nullability check. Message templates:

- **No USING INDEX:** `"ADD PRIMARY KEY on existing table '{table}' without USING INDEX on column(s) [{columns}]. Create a UNIQUE index CONCURRENTLY first, then use ADD PRIMARY KEY USING INDEX."`
- **Index not found:** `"ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': referenced index does not exist."`
- **Index not unique:** `"ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': referenced index is not UNIQUE."`
- **Nullable columns:** `"ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': column(s) [{nullable_cols}] are nullable. PostgreSQL will implicitly SET NOT NULL under ACCESS EXCLUSIVE lock. Run ALTER COLUMN ... SET NOT NULL with a CHECK constraint first."`

**Critical:** When `using_index` is `Some`, the constraint's `columns` will be empty (see §"Empty columns" above). The nullable check must get columns from `IndexState.columns`, not from the constraint:

```rust
match using_index {
    Some(idx_name) => {
        let idx = ctx.catalog_before.get_index(idx_name)
            .or_else(|| ctx.catalog_after.get_index(idx_name));
        match idx {
            None => fire("... referenced index does not exist ..."),
            Some(idx) if !idx.unique => fire("... referenced index is not UNIQUE ..."),
            Some(idx) => {
                // Check nullability of PK columns using the INDEX's columns,
                // NOT the constraint's columns (which are empty with USING INDEX).
                if let Some(table) = ctx.catalog_before.get_table(table_key) {
                    let nullable_cols: Vec<_> = idx.columns.iter()
                        .filter(|c| table.get_column(c).map_or(false, |col| col.nullable))
                        .collect();
                    if !nullable_cols.is_empty() {
                        fire("... columns are nullable ...")
                    } else {
                        return vec![];  // safe
                    }
                } else {
                    return vec![];  // table not in catalog_before, skip
                }
            }
        }
    }
    None => fire("... without USING INDEX ..."),
}
```

Also update `explain()` to describe the USING INDEX requirement, the nullable column subtlety, and the recommended safe pattern.

### 2.4. Remove dead `has_unique_covering` method
**File:** `src/catalog/types.rs` (lines 138-168)

After this change, `has_unique_covering()` has no callers — PGM012 and PGM021 were its only users. Remove it to avoid a clippy dead-code warning.

### 2.5. Tests to update/add

**PGM021 unit tests** (`src/rules/pgm021.rs`):
- `test_add_unique_with_existing_unique_index_no_finding` → now fires (no `USING INDEX`)
- `test_add_unique_with_existing_unique_constraint_no_finding` → now fires (no `USING INDEX`)
- Add: `test_add_unique_using_index_with_backing_unique_index_no_finding`
- Add: `test_add_unique_using_index_non_unique_index_fires`
- Add: `test_add_unique_using_index_no_backing_index_fires`
- Add: `test_add_unique_using_index_created_in_same_migration_no_finding` (index in `catalog_after` only)

**PGM012 unit tests** (`src/rules/pgm012.rs`):
- `test_add_pk_with_unique_constraint_no_finding` → now fires (no `USING INDEX`)
- `test_add_pk_with_unique_index_no_finding` → now fires (no `USING INDEX`)
- Add: `test_add_pk_using_index_with_backing_unique_index_not_null_no_finding`
- Add: `test_add_pk_using_index_non_unique_index_fires`
- Add: `test_add_pk_using_index_no_backing_index_fires`
- Add: `test_add_pk_using_index_nullable_columns_fires`
- Add: `test_add_pk_using_index_created_in_same_migration_no_finding`

**Matrix tests** (`tests/matrix.rs`): Snapshot updates (constructors get `using_index: None`).

### 2.6. Snapshot updates

Run `INSTA_UPDATE=always cargo test` to regenerate all affected snapshots. Key snapshots:
- `pgm021__tests__*.snap` — message text changes
- `pgm012__tests__*.snap` — message text changes
- `matrix__*.snap` — message text changes
- `integration__enterprise_sliding_window.snap` — message text for 16 PGM012 findings (all are no-USING-INDEX, still fire, but wording changes)
- `integration__all_rules*.snap` — gains PGM021 finding

### Stage 2 verification

```bash
cargo check                           # Compilation
INSTA_UPDATE=always cargo test        # All tests, regenerate snapshots
cargo clippy -- -D warnings           # No warnings (no dead-code for removed has_unique_covering)
cargo test pgm012                     # PGM012 unit tests
cargo test pgm021                     # PGM021 unit tests
cargo test matrix                     # Matrix tests
cargo test integration                # Integration tests
cargo test e2e                        # E2E tests (now asserts PGM021)
```
