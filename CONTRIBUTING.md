# Contributing: Rule System Guide

This guide explains how to add new rules to `pg-migration-lint` and the key APIs available to rule authors.

## Rule system overview

The lint pipeline feeds rules like this:

```
SQL files → Parser → IR (IrNode) → Catalog Replay → Rule Engine → Findings
```

Each migration unit is processed through the pipeline. For changed files, the engine clones the catalog before applying the unit, then passes both `catalog_before` and `catalog_after` to each rule via `LintContext`. Rules inspect IR nodes and catalog state, returning `Vec<Finding>`.

## Adding a new rule

### 1. Create the rule file

Create `src/rules/pgmXXX.rs` with two constants and a `check` function:

```rust
pub(super) const DESCRIPTION: &str = "Short one-line description";

pub(super) const EXPLAIN: &str = "PGMXXX — Title\n\
    \n\
    What it detects:\n\
    ...\n\
    \n\
    Why it's dangerous:\n\
    ...\n\
    \n\
    Example (bad):\n\
      ...\n\
    \n\
    Fix:\n\
      ...";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    // Rule logic here
}
```

The `EXPLAIN` text must reference the rule's own ID (e.g., `"PGMXXX"`) — this is verified by tests.

### 2. Add the enum variant

In `src/rules/mod.rs`, add the variant to the appropriate enum:

- `UnsafeDdlRule` for PGM0xx rules (unsafe DDL — locking, rewrites, silent side effects)
- `TypeAntiPatternRule` for PGM1xx rules (column type anti-patterns)
- `DestructiveRule` for PGM2xx rules (data loss operations)
- `IdempotencyRule` for PGM4xx rules (missing IF EXISTS / IF NOT EXISTS)
- `SchemaDesignRule` for PGM5xx rules (schema quality & informational)
- `MetaRule` for PGM9xx rules (meta-behaviors that modify other rules)

Then wire up the four dispatch match arms:

```rust
// In UnsafeDdlRule::description()
Self::PgmXXX => pgmXXX::DESCRIPTION,

// In UnsafeDdlRule::explain()
Self::PgmXXX => pgmXXX::EXPLAIN,

// In UnsafeDdlRule::check()
Self::PgmXXX => pgmXXX::check(rule, statements, ctx),

// In From<UnsafeDdlRule> for Severity
UnsafeDdlRule::PgmXXX => Self::Critical, // or Major, Minor, Info
```

Also add the variant to `RuleId::as_str()` and `RuleId::from_str()`.

### 3. Add the module declaration

In `src/rules/mod.rs`, add:

```rust
mod pgmXXX;
```

The rule is automatically registered — `RuleRegistry::register_defaults()` iterates all enum variants via `strum::EnumIter`.

### 4. Add fixture SQL

Add a violation example in `tests/fixtures/repos/all-rules/migrations/`. The all-rules fixture should have one violation per rule and is tested by integration tests.

### 5. Write unit tests

Add tests in the rule file's `#[cfg(test)] mod tests` section. Use `CatalogBuilder` and `make_ctx()` to set up test state:

```rust
#[test]
fn test_violation_fires() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![/* ... */],
    }))];

    let findings = RuleId::UnsafeDdl(UnsafeDdlRule::PgmXXX).check(&stmts, &ctx);
    insta::assert_yaml_snapshot!(findings);
}
```

### 6. Run tests and review snapshots

```bash
cargo test
```

New insta snapshots will be created automatically. Review them with `cargo insta review` or inspect the files in `src/rules/snapshots/`.

## Key APIs

### LintContext

Provided to every rule's `check()` function:

| Field | Type | Purpose |
|-------|------|---------|
| `catalog_before` | `&Catalog` | State before the current unit was applied |
| `catalog_after` | `&Catalog` | State after the current unit was applied |
| `tables_created_in_change` | `&HashSet<String>` | Tables created in the current set of changed files |
| `run_in_transaction` | `bool` | Whether the migration unit runs in a transaction |
| `is_down` | `bool` | Whether this is a down/rollback migration |
| `file` | `&Path` | The source file being linted |

**Helper methods**:

- `ctx.is_existing_table(key)` — true if table exists in `catalog_before` AND is not in `tables_created_in_change`. Use for locking/performance rules where brand-new tables are exempt.
- `ctx.table_matches_scope(key, scope)` — checks table existence against a `TableScope`:
  - `TableScope::ExcludeCreatedInChange` — same as `is_existing_table()`
  - `TableScope::AnyPreExisting` — table exists in `catalog_before` regardless of `tables_created_in_change`

### Catalog / TableState

`Catalog` query methods:

- `catalog.has_table(key)` / `catalog.get_table(key)` — look up a table by catalog key
- `catalog.get_index(name)` — look up an index across all tables
- `catalog.table_for_index(name)` — find which table owns an index

`TableState` query methods:

- `table.get_column(name)` — look up a column by name
- `table.has_covering_index(fk_columns)` — prefix-matching for FK covering index checks (PGM501)
- `table.has_unique_not_null()` — detect UNIQUE NOT NULL substitute for PK (PGM503)
- `table.constraints_involving_column(name)` — find constraints that reference a column
- `table.indexes_involving_column(name)` — find indexes that reference a column

### Finding construction

Rules have two options for creating findings:

```rust
// Option A: Using the make_finding convenience method (uses rule's default severity)
rule.make_finding(message, ctx.file, &stmt.span)

// Option B: Direct construction (for per-finding severity overrides)
Finding::new(rule.id(), Severity::Info, message, ctx.file, &stmt.span)
```

## Shared helpers

### `alter_table_check::check_alter_actions`

For rules that inspect ALTER TABLE actions on existing tables (PGM007–PGM017). Handles the boilerplate of iterating statements, filtering to `AlterTable`, checking table scope, and iterating actions:

```rust
pub fn check_alter_actions<F>(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    scope: TableScope,
    check_action: F,
) -> Vec<Finding>
where
    F: FnMut(&AlterTable, &AlterTableAction, &Located<IrNode>, &LintContext<'_>) -> Vec<Finding>,
```

### `column_type_check::check_column_types`

For rules that flag specific column types across `CREATE TABLE`, `ADD COLUMN`, and `ALTER COLUMN TYPE` (PGM101–PGM106):

```rust
pub fn check_column_types(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    rule: impl Rule,
    predicate: impl Fn(&TypeName) -> bool,
    message_fn: impl Fn(&str, &QualifiedName, &TypeName) -> String,
) -> Vec<Finding>
```

## Testing patterns

### CatalogBuilder

Fluent API for building catalog state in tests:

```rust
let catalog = CatalogBuilder::new()
    .table("orders", |t| {
        t.column("id", "int", false)
         .column("status", "text", true)
         .index("idx_status", &["status"], false)
         .pk(&["id"])
         .fk("fk_customer", &["customer_id"], "customers", &["id"])
         .unique("uq_email", &["email"])
         .incomplete()  // mark as affected by unparseable SQL
    })
    .build();
```

Methods: `column()`, `column_with_default()`, `index()`, `pk()`, `fk()`, `unique()`, `incomplete()`.

### test_helpers

- `make_ctx(before, after, file, created)` — build a `LintContext` with default settings (in transaction, not a down migration)
- `make_ctx_with_txn(before, after, file, created, run_in_transaction)` — same but with explicit transaction flag
- `located(node)` — wrap an `IrNode` in a `Located` with a dummy span at line 1

### Insta snapshots

Most rule tests use `insta::assert_yaml_snapshot!()` for findings. Snapshots live in `src/rules/snapshots/`. When adding tests:

1. Write the test with `insta::assert_yaml_snapshot!(findings);`
2. Run `cargo test` — it will fail and create a `.snap.new` file
3. Run `cargo insta review` to accept or reject
4. Commit the `.snap` files

## Rule ID conventions

Rule IDs follow the pattern `PGM<family><number>`:

| Range | Family enum | Purpose |
|-------|-------------|---------|
| 0xx | `UnsafeDdlRule` | Locking, table rewrites, silent side effects |
| 1xx | `TypeAntiPatternRule` | PostgreSQL "Don't Do This" column types |
| 2xx | `DestructiveRule` | Data loss operations (DROP TABLE, etc.) |
| 4xx | `IdempotencyRule` | Missing IF EXISTS / IF NOT EXISTS |
| 5xx | `SchemaDesignRule` | Schema quality & informational |
| 9xx | `MetaRule` | Meta-behaviors modifying other rules |

Gaps within a family are intentional (e.g., PGM004–005 are unoccupied in 0xx). Do not reuse retired IDs.

**Proposed rules** use 4-digit IDs (e.g., PGM1202). When promoted to stable, they become 3-digit, typically keeping the same last three digits (e.g., PGM1202 → PGM202).

## Conventions

- Rules must be `Send + Sync` (to support future parallel execution).
- No `unwrap()` or `expect()` in library code — use proper error handling.
- All public functions require doc comments.
- The `EXPLAIN` constant must reference the rule's own ID string.
- Index column order must be preserved (affects FK covering index prefix matching).
- Down migrations get INFO severity cap automatically (PGM901) — rules don't need to handle this.
- Error paths: parse failures on individual files warn and continue; rules don't see unparseable SQL (it becomes `IrNode::Unparseable` with a table hint that marks the table as `incomplete`).
