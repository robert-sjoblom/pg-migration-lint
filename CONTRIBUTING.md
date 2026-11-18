# Contributing: Rule System Guide

This guide explains how to add new rules to `pg-migration-lint` and the key APIs available to rule authors.

## Rule system overview

The lint pipeline feeds rules like this:

```
SQL files → Parser → IR (IrNode) → Normalize → Catalog Replay → Rule Engine → Findings
```

Each migration unit is processed through the pipeline. For changed files, the engine clones the catalog before applying the unit, then passes both `catalog_before` and `catalog_after` to each rule via `LintContext`. Rules inspect IR nodes and catalog state, returning `Vec<Finding>`.

## Adding a new rule

### 1. Create the rule file

Create `src/rules/pgmXXX.rs`. Its first line pulls the rule's markdown file (step 6) in as the module doc, and the same file is the `EXPLAIN` text, so the prose is written once:

```rust
#![doc = include_str!("docs/pgmXXX.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Short one-line description";

pub(super) const EXPLAIN: &str = include_str!("docs/pgmXXX.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major; // Info, Minor, Major, or Critical

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    // Rule logic here
}
```

`DESCRIPTION` is the only copy of the rule's title. `--explain`, `--list-rules`, the GitHub review comment and `docs/rules.md` each compose their own heading from it, so the markdown body must not repeat it (see step 6). The `SPEC.md` heading for the rule must use the same words, backticks aside; a test under `--features docgen` checks that.

### 2. Add the enum variant

In `src/rules/rule_id.rs`, add a variant to the flat `RuleId` enum in the appropriate family section:

```rust
/// Short doc comment describing the rule.
#[strum(serialize = "PGMXXX")]
PgmXXX,
```

Then add one line to the `dispatch_rules!` macro invocation (also in `src/rules/rule_id.rs`), which generates all four `Rule` trait methods (`default_severity`, `description`, `explain`, `check`) from this single mapping:

```rust
dispatch_rules! {
    // ...
    PgmXXX => pgmXXX,
    // ...
}
```

The `#[strum(serialize)]` attribute handles `as_str()`, `FromStr`, and `Display` automatically via strum derives.

Every other exhaustive `match` on `RuleId` needs an arm for the new variant too: `sonarqube_meta()` and `effort_minutes()` in `src/output/sonarqube.rs`. If the rule should be opt-in, also add it to the `matches!` in `RuleId::default_enabled()`.

### 3. Add the module declaration

In `src/rules/mod.rs`, add:

```rust
mod pgmXXX;
```

The rule is automatically included — `RuleId::lint_rules()` iterates all non-meta enum variants via `strum::EnumIter`.

In the same file's tests, bump the `RuleId::iter().count()` assertion in `test_rule_id_display_round_trip`.

### 4. Add fixture SQL

Add a violation example in `tests/fixtures/repos/all-rules/migrations/`. The all-rules fixture should have one violation per rule and is tested by integration tests.

Add a suppressed violation in `tests/fixtures/repos/suppressed/migrations/` as well (e.g. `-- pgm-lint:suppress-file PGMXXX`). `tests/baseline.rs` fails if any lint rule does not fire in `all-rules/`, or does not fire before suppression in `suppressed/`.

### 5. Write unit tests

Add tests with positive and negative cases in the rule file's `#[cfg(test)] mod tests` section. Use `CatalogBuilder` and the `lint_ctx!` macro from `src/rules/test_helpers.rs` to set up test state:

```rust
#[test]
fn test_violation_fires() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();
    let after = before.clone();
    lint_ctx!(ctx, &before, &after, "migrations/002.sql");

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![/* ... */],
    }))];

    let findings = RuleId::PgmXXX.check(&stmts, &ctx);
    insta::assert_yaml_snapshot!(findings);
}
```

`lint_ctx!` also takes `created: ["orders"]` for tables created in the same change, or `txn: false` for a unit that runs outside a transaction.

### 6. Add the rule's markdown file

Create `src/rules/docs/pgmXXX.md`. This one file is the rule's module doc, the body that `--explain PGMXXX` prints verbatim, and the body of the rule's section in `docs/rules.md`, so it is written once and rendered three ways. Look at existing files for the shape: an opening paragraph on what is detected and why it matters, `**Example**` and `**Fix**` labels with fenced SQL, then notes and cross-references.

The file has to satisfy rustdoc, a terminal and Jekyll at once, so `src/rules/rule_docs_tests.rs` enforces these rules on every rule:

- body only: no title line and no `#` headings (the title comes from `DESCRIPTION`)
- every code fence carries a language tag (` ```sql `, ` ```text `, ` ```xml `). An untagged fence, or a line indented four spaces outside a fence, is a Rust doctest to rustdoc and `cargo test` would try to compile it
- no Jekyll, Liquid or kramdown syntax (`{:`, `{{`, `{%`)
- cross-references use the absolute docs URL, `[PGM004](https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm004)`, never a bare `#pgm004` anchor, so they resolve in the terminal, in GitHub comments and in rustdoc
- the file ends with exactly one newline

### 7. Update the rule count and family range

The rule count and the family ranges are written by hand in a few places. Bump the count in `README.md` (it appears twice) and `docs/index.md`. If the new ID extends its family's range, update the range in `README.md` and `docs/rules.md.j2`. `docs/rules.md` fills in its count from `{{ rule_count }}`, so the template's count needs no change.

### 8. Run tests and review snapshots

```bash
cargo test --features docgen
```

New insta snapshots will be created automatically. Review them with `cargo insta review` or inspect the files in `src/rules/snapshots/`. The docgen `rules_md` snapshot and the SonarQube `all_rules_metadata_snapshot` (in `src/output/snapshots/`) change too.

After accepting snapshots, sync the generated docs:

```bash
make docs-sync
```

This updates `docs/rules.md` from the docgen snapshot. Commit both the snapshot and the generated docs.

## Key APIs

### LintContext

Provided to every rule's `check()` function:

| Field | Type | Purpose |
|-------|------|---------|
| `catalog_before` | `&Catalog` | State before the current unit was applied |
| `catalog_after` | `&Catalog` | State after the current unit was applied |
| `tables_created_in_change` | `&HashSet<RelationName>` | Tables created in the current set of changed files |
| `run_in_transaction` | `bool` | Whether the migration unit runs in a transaction |
| `is_down` | `bool` | Whether this is a down/rollback migration |
| `file` | `&Path` | The source file being linted |

**Helper methods**:

- `ctx.is_existing_table(key)` — true if table exists in `catalog_before` AND is not in `tables_created_in_change`. Use for locking/performance rules where brand-new tables are exempt.
- `ctx.table_matches_scope(key, scope)` — checks table existence against a `TableScope`:
  - `TableScope::ExcludeCreatedInChange` — same as `is_existing_table()`
  - `TableScope::AnyPreExisting` — table exists in `catalog_before` regardless of `tables_created_in_change`
- `ctx.transaction_windows(statements)`: the unit's statements grouped by the transaction PostgreSQL runs them in, using `run_in_transaction`. See [`transaction_window`](#transaction_window) under Shared helpers.

### Names

- Relation names in the IR that rules receive are `ResolvedName`s. `name.relation()` is the `&RelationName` catalog key, and `name.display_name()` is the text for messages: the bare name when the schema came from `default_schema`, `schema.name` when the SQL wrote one.
- `RelationName { schema, name }` has no `Display` impl, so a catalog key can't end up in a message. Use `display_name()`, or `TableState::display_name` for a table from the catalog.

### Catalog / TableState

`Catalog` query methods:

- `catalog.has_table(key)` / `catalog.get_table(key)` — look up a table by its `&RelationName` key
- `catalog.get_index(key)` — look up an index by its `&RelationName` key, in that key's schema only
- `catalog.table_for_index(key)` — find which table owns an index, looked up the same way

`TableState` query methods:

- `table.get_column(name)` — look up a column by name
- `table.has_indexed_fk_column(fk_columns)` — true if any usable index contains at least one FK column, in any position (PGM501)
- `table.has_unique_not_null()` — detect UNIQUE NOT NULL substitute for PK (PGM503 reports it, PGM502 skips such a table)
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

For rules that inspect ALTER TABLE actions on existing tables. PGM004, PGM005, PGM007–PGM009, PGM013–PGM015, PGM019 and PGM507 call it directly. PGM010–PGM012 and PGM025 call it through `drop_column_check::check_drop_column_constraints`, which passes on only `DROP COLUMN` actions, along with the table's state in `catalog_before`. It handles the boilerplate of iterating statements, filtering to `AlterTable`, checking table scope, and iterating actions:

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

### `drop_column_check::check_drop_column_constraints`

For rules that report what `ALTER TABLE ... DROP COLUMN` silently removes along with the column (PGM010–PGM012 and PGM025). It calls `check` for each `DROP COLUMN` on a table in `catalog_before` (`TableScope::AnyPreExisting`), with the dropped column's name and the table's state before the unit:

```rust
pub fn check_drop_column_constraints<F>(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    check: F,
) -> Vec<Finding>
where
    F: FnMut(&str, &AlterTable, &TableState, &Located<IrNode>, &LintContext<'_>) -> Vec<Finding>,
```

### `column_type_check::check_column_types`

For rules that flag specific column types across `CREATE TABLE`, `ADD COLUMN`, and `ALTER COLUMN TYPE` (PGM101–PGM104, PGM106, PGM108 and PGM109):

```rust
pub fn check_column_types(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    rule: impl Rule,
    predicate: impl Fn(&TypeName) -> bool,
    message_fn: impl Fn(&str, &ResolvedName, &TypeName) -> String,
) -> Vec<Finding>
```

### `existing_table_check::check_existing_table`

For rules that flag one kind of statement on a pre-existing table (PGM201, PGM203, PGM301–PGM303 and PGM505). `extract` matches the `IrNode` variant the rule cares about and returns the table name with the finding's message, or `None` for every other statement. A finding is emitted when `ctx.is_existing_table()` holds, and carries the table as its dedup key, so the rule reports each table at most once per unit:

```rust
pub fn check_existing_table(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    rule: impl Rule,
    extract: impl FnMut(&IrNode) -> Option<(&ResolvedName, String)>,
) -> Vec<Finding>
```

### `using_index`

For rules that look at `ALTER TABLE ... ADD PRIMARY KEY USING INDEX` or `ADD UNIQUE USING INDEX` (PGM016, PGM017, PGM107 and PGM027):

- `using_index_constraints(at)`: the `(constraint kind, index name)` of each `USING INDEX` constraint in an `AlterTable`, in statement order
- `has_using_index(node)`: true for an `ALTER TABLE` with a `USING INDEX` constraint
- `UsingIndex::in_catalog(catalog, table, index_name)`: what the index name refers to in the altered table's schema, one of `OfTable { index, table }`, `OfAnotherTable { owner }` (PostgreSQL rejects the statement and PGM027 reports it) or `Missing`

PostgreSQL looks the index up when the statement runs, and attaching it renames it to the constraint's name, so judge each statement against the catalog just before it. `replay::map_with_state_before_if_needed` replays the unit's earlier statements on a copy of `catalog_before`, and only when some statement passes the filter:

```rust
map_with_state_before_if_needed(ctx.catalog_before, statements, has_using_index, |state, stmt| {
    // UsingIndex::in_catalog(state, at.name.relation(), index_name)
})
```

### `rejected_statements`

For statements PostgreSQL rejects against the schema just before them: a `USING INDEX` of another table's index, a rename onto a taken name, and `DROP INDEX` of the index behind a PRIMARY KEY or UNIQUE constraint. PGM027 reports them through `rejections(catalog_before, statements)`, which gives each statement's `Option<Rejection>`. A rule that would report what such a statement does leaves it to PGM027 (PGM002, PGM003, PGM401, PGM501 and PGM504):

```rust
let rejected = RejectedStatements::new(ctx.catalog_before, statements);
// ...
if rejected.contains(&stmt.span) {
    continue;
}
```

The unit is judged on the first `contains` call, so a rule that asks only before reporting replays nothing when it has nothing to report.

### `transaction_window`

For rules about what one transaction does at once, such as the locks it holds until it ends, or about whether a statement runs inside a transaction block, where `CONCURRENTLY` fails. `ctx.transaction_windows(statements)` splits the unit into `TransactionWindow`s, in statement order:

- `TransactionWindowKind::Autocommit`: one SQL statement outside a transaction block. The nodes a statement is split into (`DROP TABLE a, b`) share its span and its window.
- `TransactionWindowKind::BeginBlock`: from a `BEGIN` or `START TRANSACTION` in the file through the `COMMIT` or `ROLLBACK` that ends it, or to the end of the unit when nothing does.
- `TransactionWindowKind::ToolTransaction`: the transaction the migration tool opens when `run_in_transaction` is on, from the first statement to the end of the unit, or to a `COMMIT` or `ROLLBACK` in the file that ends it early. Statements after that are autocommit until a `BEGIN`. This assumes a runner that sends each statement separately and starts no new transaction after the file's `COMMIT`, like `psql --single-transaction`. JDBC runners with autocommit off (Flyway, Liquibase) start a new transaction after it, and golang-migrate's default single `Exec` runs the rest as a new implicit transaction.

Every statement, `TransactionControl` nodes included, is in exactly one window. As in PostgreSQL, a `BEGIN` inside a transaction stays in the open window and does nothing, and a `COMMIT` or `ROLLBACK` outside one does nothing and is an autocommit window of its own. `COMMIT AND CHAIN` arrives as `Commit` then `Begin` with the same span, so it ends one window and opens the next. A known limitation: `PREPARE TRANSACTION` ends the transaction block in PostgreSQL, but the IR keeps it `Ignored`, so its window continues past it.

```rust
let windows = ctx.transaction_windows(statements);
for window in &windows {
    // window.kind(), window.statement_range(), window.statements_in(statements)
}
for (index, stmt) in statements.iter().enumerate() {
    if windows.window_of(index).is_some_and(|w| w.is_transaction_block()) {
        // `stmt` runs inside a transaction block
    }
}
```

Look a statement up by its index, not its span: the two nodes of `COMMIT AND CHAIN` share a span but not a window. The windows are built on each call, in one pass, and not cached on the context, so a test can reuse one `lint_ctx!` context for several statement lists. Because `located` puts every node at the same span, a test that needs separate autocommit statements builds them with `located_at` on distinct lines, or parses SQL. They are not for deciding whether a table is new to the change; `ctx.is_existing_table` and `tables_created_in_change` do that.

### `partition_parent_lock`

For rules about the ACCESS EXCLUSIVE lock that `DROP TABLE` of a partition and `CREATE TABLE ... PARTITION OF` take on the partition's parent (PGM024 and PGM026). `ParentLocks::of_unit(statements)` notes the children the unit detaches, since dropping one of those locks no parent. `taken_by(node, ctx)` then gives the `ParentLock` a statement takes, or `None`: `statement` (`DropPartition` or `CreatePartitionOf`), the `child` it drops or creates, and the `parent` (`LockedParent { key, display_name }`). A parent counts only when it is in `catalog_before` and not created in the change (`ctx.is_existing_table`):

```rust
let parent_locks = ParentLocks::of_unit(statements);
for stmt in statements {
    if let Some(lock) = parent_locks.taken_by(&stmt.node, ctx) {
        // lock.statement, lock.child.display_name(), lock.parent.key, lock.parent.display_name
    }
}
```

### Lookup tables

`fn_volatility::lookup(name)` gives a built-in function's volatility (PGM006), and `reserved_keywords::is_reserved(name)` says whether a lowercase name is a reserved keyword (PGM509). Both files are generated from PostgreSQL by scripts under `verify/`; do not edit them by hand.

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
         .incomplete();  // mark as affected by unparseable SQL
    })
    .build();
```

Methods of the table builder the closure receives:

- columns: `column()`, `column_with_default()`
- indexes: `index()`, `index_with_method()` (an access method such as `"gin"`), `only_index()` (`ON ONLY`), `partial_index()` (with a `WHERE` clause), `expression_index()` (entries prefixed `"expr:"` are expressions)
- constraints: `pk()` (named `{table}_pkey`), `named_pk()`, `fk()`, `unique()`, `check_constraint()`, `exclude_constraint()`. An unnamed CHECK or EXCLUDE constraint gets the name PostgreSQL would generate. None of them adds a backing index
- table state: `incomplete()`, `partitioned_by()`, `partition_of()`, `display_name()`

To inspect a catalog in a test, use the `CatalogAssertions` trait from the same file: `table()`, `assert_table()`, `assert_no_table()`, `assert_partition_children()`, `assert_index_owner()`, `assert_index_unregistered()` and `assert_same_state_as()`. Each panics with what the catalog actually holds.

Table names, including `fk()`'s referenced table, are written `"orders"` (meaning `public.orders`) or `"audit.orders"`. `lint_ctx!` takes tables created in the same change in that written form, e.g. `lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders", "audit.orders"]);`. When a test needs a `RelationName` itself, `rel("orders")` and `rel_in("audit", "orders")` from `src/catalog/builder.rs` build it.

### test_helpers

- `lint_ctx!(ctx, &before, &after, "migrations/002.sql")`: the form rules use. Binds `ctx` through `make_ctx` and owns the file path and `created` set for you; append `created: ["orders"]` to fill the `created` set, or `txn: false` to go through `make_ctx_with_txn` instead
- `make_ctx(before, after, file, created)` — build a `LintContext` with default settings (in transaction, not a down migration)
- `make_ctx_with_txn(before, after, file, created, run_in_transaction)` — same but with explicit transaction flag
- `located(node)` — take a parser-form `IrNode<QualifiedName>`, resolve its names against `public` as the pipeline would, and wrap it in a `Located` with a dummy span at line 1
- `located_at(node, line)`: same as `located`, with the span at `line`, for tests whose findings depend on statement lines
- `normalized(statements)`: resolve a `Vec` of statements against `public`, either parser output such as `parse_sql(sql)` or statements `located` already resolved
- `catalog_from_sql(sql)`: the catalog the pipeline builds from `sql`, parsed, resolved against `public` and replayed into an empty catalog

### Insta snapshots

Most rule tests use `insta::assert_yaml_snapshot!()` for findings. Snapshots live in `src/rules/snapshots/`. When adding tests:

1. Write the test with `insta::assert_yaml_snapshot!(findings);`
2. Run `cargo test` — it will fail and create a `.snap.new` file
3. Run `cargo insta review` to accept or reject
4. Commit the `.snap` files

## Rule ID conventions

Rule IDs follow the pattern `PGM<family><number>`:

| Range | Family | Purpose |
|-------|--------|---------|
| 0xx | Unsafe DDL | Locking, table rewrites, silent side effects |
| 1xx | Type anti-patterns | PostgreSQL "Don't Do This" column types |
| 2xx | Destructive | Data loss operations (DROP TABLE, etc.) |
| 3xx | DML in migrations | INSERT/UPDATE/DELETE in migration files |
| 4xx | Idempotency | Missing IF EXISTS / IF NOT EXISTS |
| 5xx | Schema design | Schema quality & informational |
| 9xx | Meta | Meta-behaviors modifying other rules |

All variants live in a single flat `RuleId` enum (no per-family sub-enums). Gaps within a family are intentional. Do not reuse retired IDs.

**Proposed rules** use 4-digit IDs (e.g., PGM1202). When promoted to stable, they become 3-digit, typically keeping the same last three digits (e.g., PGM1202 → PGM202).

## Generated documentation (`docs/rules.md`)

`docs/rules.md` is **generated** — do not edit it by hand. It is produced from three sources:

1. **Rule metadata** — `DESCRIPTION` and `default_severity()` from each rule in `src/rules/`
2. **Rule markdown** — `src/rules/docs/pgmXXX.md`, read through `Rule::explain()` (one per rule)
3. **Template** — `docs/rules.md.j2` (minijinja template for the overall page structure)

The generation is feature-gated behind `--features docgen` (using the `minijinja` crate) and verified by an insta snapshot test.

### Workflow

```bash
# 1. Change rule code, examples, or template
# 2. Run tests — the docgen snapshot will fail if output changed
cargo test --features docgen

# 3. Review and accept the snapshot diff
cargo insta review

# 4. Sync snapshot → docs/rules.md
make docs-sync

# 5. Commit the snapshot + docs/rules.md together
```

### What triggers a snapshot failure

- Changing a rule's `DESCRIPTION` or `default_severity()`
- Editing any `src/rules/docs/pgmXXX.md` file
- Adding or removing a rule
- Changing `docs/rules.md.j2`

### File layout

| Path | Purpose |
|------|---------|
| `src/rules/docs/pgmXXX.md` | Per-rule content; also the module doc and the `--explain` body |
| `docs/rules.md.j2` | Page template |
| `src/docgen.rs` | Build context + render + snapshot test |
| `src/snapshots/…rules_md.snap` | Accepted snapshot (source of truth) |
| `docs/rules.md` | Generated output (synced from snapshot) |

## Conventions

- Rules must be `Send + Sync` (to support future parallel execution).
- No `unwrap()` or `expect()` in library code — use proper error handling.
- All public functions require doc comments.
- The `EXPLAIN` markdown must follow the contract in step 6, which `src/rules/rule_docs_tests.rs` checks for every rule.
- Index column order must be preserved (affects PGM508's prefix-based redundant-index detection via `is_prefix()`).
- Down migrations get INFO severity cap automatically (PGM901) — rules don't need to handle this.
- Error paths: parse failures on individual files warn and continue; rules don't see unparseable SQL (it becomes `IrNode::Unparseable` with a table hint that marks the table as `incomplete`).
