# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`pg-migration-lint` is a Rust CLI tool that statically analyzes PostgreSQL migration files for safety and correctness issues. It builds an internal table catalog by replaying migration history, then lints only new/changed files. Outputs SARIF (GitHub Code Scanning) and SonarQube Generic Issue Import JSON.

## Key Commands

### Build and Test
```bash
cargo build                    # Build debug binary
cargo clippy                   # Run linter
cargo check                    # Fast compilation check
cargo fmt                      # Format code
cargo test --features docgen   # Run all tests including docgen snapshot tests
```


## Architecture Overview

### Core Pipeline

```
Input Files → Parser → IR → Normalize → Replay Engine → Rule Engine → Reporter
                                            ↓
                                      Table Catalog
```

1. **Input Layer** (`src/input/`): Loads raw SQL and Liquibase migrations
2. **Parser** (`src/parser/`): Converts SQL to Intermediate Representation (IR) using `pg_query` bindings
3. **Normalize** (`src/normalize.rs`): `resolve_units` resolves every parsed `QualifiedName` against `default_schema` into a `ResolvedName`, so replay and rules only see schema-qualified names
4. **Catalog** (`src/catalog/`): Replays all migrations to build table state
5. **Rules** (`src/rules/`): Lints changed files against rules (PGM001-PGM025, PGM101-PGM109, PGM201-PGM205, PGM301-PGM303, PGM401-PGM403, PGM501-PGM509)
6. **Output** (`src/output/`): Emits SARIF, SonarQube JSON, or text

### Intermediate Representation (IR)

The tool converts SQL AST into a simplified IR layer to decouple rules from parser internals.

Key IR nodes (`src/parser/ir.rs`):
```rust
pub enum IrNode<N = ResolvedName> {
    CreateTable(CreateTable<N>),
    AlterTable(AlterTable<N>),
    CreateIndex(CreateIndex<N>),
    DropIndex(DropIndex<N>),
    DropTable(DropTable<N>),
    DropSchema(DropSchema),
    TruncateTable(TruncateTable<N>),
    InsertInto(InsertInto<N>),
    UpdateTable(UpdateTable<N>),
    DeleteFrom(DeleteFrom<N>),
    Cluster(Cluster<N>),
    VacuumFull(VacuumFull<N>),
    Reindex(Reindex<N>),
    AlterIndexAttachPartition { parent_index_name, child_index_name },
    RenameTable { name, new_name },
    RenameColumn { table, old_name, new_name },
    RenameIndex { index, new_name },
    Ignored { raw_sql: String },        // Parsed but not relevant (GRANT, COMMENT ON)
    Unparseable { raw_sql: String, table_hint: Option<N> },
}
```

The IR is generic over its relation-name type `N`, and so are `CreateTable`, `AlterTable`, `AlterTableAction`, `TableConstraint`, `CreateIndex`, `DropIndex` and the other name-bearing types. The parser emits `IrNode<QualifiedName>`; normalization produces the default `IrNode<ResolvedName>`, which is what replay and the rules see. Bare names (`RENAME TO` targets, index names in `CREATE INDEX`, constraint and column names) stay `String`.

`CreateTable` uses a `TablePersistence` enum (`Permanent`, `Unlogged`, `Temporary`) instead of a boolean `temporary` field.

Supporting types:
- `QualifiedName { schema: Option<String>, name }` - a relation name as written in the SQL (parser output); `resolve(default_schema)` turns it into a `ResolvedName`
- `ResolvedName` (`src/names.rs`) - a resolved name: `relation()` is its catalog key, `name()` the bare name, `display_name()` the name for messages (bare when the schema came from `default_schema`, `schema.name` when the SQL wrote one)
- `RelationName { schema, name }` (`src/names.rs`) - the catalog key; the schema is never optional, and there is no `Display` impl, so a key can't leak `public.` into messages
- `ColumnDef { name, type_name, nullable, default_expr, is_inline_pk, is_serial }`
- `TypeName { name, modifiers }` - e.g., `varchar(100)` has modifiers `[100]`
- `DefaultExpr` - enum: `Literal`, `FunctionCall { name, args }`, `Other`
- `TableConstraint` - enum: `PrimaryKey`, `ForeignKey`, `Unique`, `Check`, `Exclude`
- `AlterTableAction` - enum: `AddColumn`, `DropColumn`, `AddConstraint { constraint, origin }`, `AlterColumnType`, `SetNotNull`, `DropNotNull`, `SetDefault`, `DropDefault`, `DropConstraint`, `ValidateConstraint`, `Other`
- `ConstraintOrigin` - enum: `AddColumn` (written inline in an `ADD COLUMN`; directly follows that `AddColumn`), `AddConstraint` (an `ADD CONSTRAINT` subcommand). Rules ignore it; replay uses it to name constraints in Postgres's creation order. Build with `AlterTableAction::add_constraint` / `add_column_constraint`

Each statement is wrapped in `Located<IrNode>` with `SourceSpan` for line number tracking.

### Table Catalog

Built by replaying migrations in order. Represents schema state at each migration point (`src/catalog/types.rs`):

```rust
pub struct Catalog {
    tables: HashMap<RelationName, TableState>,
    index_to_table: HashMap<RelationName, RelationName>,  // index → owning table, both in the table's schema
}

pub struct TableState {
    pub name: RelationName,
    pub display_name: String,
    pub columns: Vec<ColumnState>,
    pub indexes: Vec<IndexState>,
    pub constraints: Vec<ConstraintState>,
    pub has_primary_key: bool,
    pub incomplete: bool,       // true if unparseable SQL touched this table
    pub is_partitioned: bool,
    pub partition_by: Option<PartitionByInfo>,
    pub parent_table: Option<RelationName>,
}
```

Key methods on `TableState`:
- `get_column(&self, name: &str) -> Option<&ColumnState>`
- `has_indexed_fk_column(&self, fk_columns: &[String]) -> bool` - true if any usable index contains at least one FK column, in any position (skips non-btree, partial, ONLY indexes); used by PGM501
- `has_unique_not_null(&self) -> bool` - for PGM503 detection (btree-only, skips partial/expression indexes)

The catalog tracks:
- Table creation/deletion
- Column additions/modifications/deletions (including type changes)
- Index creation/deletion (with column order preserved)
- Constraint additions
- Primary key existence

**Single-pass replay strategy**: The pipeline iterates through migration history once. For each unit:
- If in changed files: clone catalog, apply unit, lint with both before/after catalogs
- Otherwise: just apply unit to catalog
- No separate "replay_until" phase

### Rule System

`RuleId` implements the `Rule` trait (`src/rules/mod.rs`); the `dispatch_rules!` macro in `src/rules/rule_id.rs` forwards each variant to its rule module:

```rust
pub trait Rule: Send + Sync {
    fn id(&self) -> RuleId;
    fn default_severity(&self) -> Severity;
    fn description(&self) -> &'static str;
    fn explain(&self) -> &'static str;  // For --explain output
    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>;

    // Provided: a Finding with this rule's ID and default severity
    fn make_finding(&self, message: String, file: &std::path::Path, span: &SourceSpan) -> Finding {
        Finding::new(self.id(), self.default_severity(), message, file, span)
    }
}
```

`LintContext` provides:
```rust
pub struct LintContext<'a> {
    pub catalog_before: &'a Catalog,  // State BEFORE applying current unit
    pub catalog_after: &'a Catalog,   // State AFTER applying current unit
    pub tables_created_in_change: &'a HashSet<RelationName>,  // For "new table" detection
    pub run_in_transaction: bool,     // From MigrationUnit metadata
    pub is_down: bool,                // Down/rollback migration flag
    pub file: &'a Path,
}
```

Rules use `catalog_before` to check if tables are pre-existing (PGM001/002) and `catalog_after` for post-file checks (PGM501/502/503). The two-catalog approach enables single-pass replay without needing separate replay runs.

#### Rule Severities
Five levels (`src/rules/severity.rs`), ordered `Info < Minor < Major < Critical < Blocker`:
- **BLOCKER**: Reserved for user severity overrides; no rule defaults to it.
- **CRITICAL**: Causes downtime or data corruption (e.g., missing `CONCURRENTLY`)
- **MAJOR**: Performance issues or schema problems (e.g., missing FK index, no primary key)
- **MINOR**: Potentially unintended behavior
- **INFO**: Informational findings

#### Rules (54 total)

**0xx — Unsafe DDL** (PGM001–PGM025): Missing CONCURRENTLY, table rewrites, unsafe constraint additions, silent side effects from DROP COLUMN, VACUUM FULL, REINDEX, partition operations.
**1xx — Type Anti-patterns** (PGM101–PGM109): timestamp without tz, timestamp(0) rounding, char(n), money, serial, json, integer PK, varchar(n), floating-point.
**2xx — Destructive Operations** (PGM201–PGM205): DROP TABLE, DROP TABLE CASCADE, TRUNCATE, TRUNCATE CASCADE, DROP SCHEMA CASCADE.
**3xx — DML in Migrations** (PGM301–PGM303): INSERT, UPDATE, DELETE on existing tables.
**4xx — Idempotency Guards** (PGM401–PGM403): Missing IF EXISTS / IF NOT EXISTS, misleading IF NOT EXISTS no-ops.
**5xx — Schema Design** (PGM501–PGM509): Missing FK index, no PK, UNIQUE NOT NULL instead of PK, renames, unlogged tables, DROP NOT NULL, redundant indexes, mixed-case/reserved-word identifiers.
**9xx — Meta-behavior** (PGM901): Down migrations cap all findings to INFO.

## Development Workflow

### Adding a New Rule

1. Define rule in `src/rules/pgmXXX.rs`, starting with `#![doc = include_str!("docs/pgmXXX.md")]`. The module does not implement `Rule`; it exposes `pub(super)` items that `dispatch_rules!` wires up:
   - `DESCRIPTION: &str` - short summary; the only copy of the rule's title
   - `EXPLAIN: &str` - `include_str!("docs/pgmXXX.md")`
   - `DEFAULT_SEVERITY: Severity` - Critical, Major, Minor, Info
   - `fn check(rule: impl Rule, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>` - main rule logic; build findings with `rule.make_finding(..)`
2. Write `src/rules/docs/pgmXXX.md`, the rule's markdown body. It is the module doc, the `--explain` output (printed verbatim) and the `docs/rules.md` body. No title line, no headings, a language tag on every fence, absolute docs URLs for cross-references; `src/rules/rule_docs_tests.rs` enforces this.
3. Register the rule:
   - `src/rules/rule_id.rs`: add a `PgmXXX` variant to `RuleId` in its family section (doc comment plus `#[strum(serialize = "PGMXXX")]`), and a `PgmXXX => pgmXXX,` line to the `dispatch_rules!` invocation
   - `src/rules/mod.rs`: add `mod pgmXXX;`, and bump the `RuleId::iter().count()` assertion in `test_rule_id_display_round_trip`
   - `src/output/sonarqube.rs`: add the variant to the exhaustive `sonarqube_meta()` and `effort_minutes()` matches
   - Opt-in rules only: add the variant to `RuleId::default_enabled()`
4. Add unit tests with positive and negative cases in the rule file's `#[cfg(test)] mod tests`, using `CatalogBuilder` plus `lint_ctx!` and `located` from `src/rules/test_helpers.rs`, calling `RuleId::PgmXXX.check(..)` and snapshotting the findings with `insta`
5. Add a violation to fixture repo `tests/fixtures/repos/all-rules/` and a suppressed one to `tests/fixtures/repos/suppressed/`; `tests/baseline.rs` fails if any lint rule does not fire in either
6. Add the rule's section to `SPEC.md`: behaviour only (Severity, Triggers, Does not fire when, Message). The heading title must equal `DESCRIPTION` and the Severity bullet must start with `DEFAULT_SEVERITY`; a test under `--features docgen` checks both.
7. Run `cargo test --features docgen`, accept the `rules_md` snapshot along with the rule's own and the SonarQube `all_rules_metadata_snapshot` (`cargo insta accept`), then `make docs-sync` to regenerate `docs/rules.md`. Any later change to a rule's markdown needs the same pass.

### Working with IR

When modifying the IR layer (`src/parser/ir.rs`):
- Changes cascade to parser, catalog replay, and all rules
- Update the IR contract first, then notify affected components
- Preserve `SourceSpan` (line numbers) for accurate error reporting

### Catalog Replay

The replay engine (`src/catalog/replay.rs`) has a single function:
```rust
pub(crate) fn apply(catalog: &mut Catalog, unit: &MigrationUnit);
```

`MigrationUnit` defaults to `MigrationUnit<ResolvedName>`, so replay only accepts units that have been through `resolve_units`.

`LintPipeline` (`src/pipeline.rs`) drives replay. It owns the catalog and `tables_created_in_change`; `main.rs` feeds it the resolved units in order, calling `lint` for changed files and `replay` for the rest:
```rust
impl LintPipeline {
    fn replay(&mut self, unit: &MigrationUnit) {
        apply(&mut self.catalog, unit);
    }

    fn lint(&mut self, unit: &MigrationUnit, rules: &[RuleId]) -> Vec<Finding> {
        let catalog_before = self.catalog.clone();
        apply(&mut self.catalog, unit);
        record tables the unit creates in self.tables_created_in_change;
        run rules with (&catalog_before, &self.catalog);  // catalog_after is current catalog
        cap findings to INFO if unit.is_down;             // PGM901
    }
}
```

The `apply` function handles all `IrNode` variants.

### Catalog Test Builders

Use the builder pattern to set up catalog state in tests (defined in `src/catalog/builder.rs`):
```rust
let catalog = CatalogBuilder::new()
    .table("orders", |t| {
        t.column("id", "int", false)
         .column("status", "text", true)
         .index("idx_status", &["status"], false)
         .pk(&["id"])
         .fk("fk_customer", &["customer_id"], "customers", &["id"]);
    })
    .build();
```

Table names, including `fk`'s referenced table, are written `"orders"` (meaning `public.orders`; `TEST_DEFAULT_SCHEMA = "public"`) or `"audit.orders"`. `rel("orders")` and `rel_in("audit", "orders")` build the matching `RelationName`, and `test_relation(..)` accepts either written form.

To inspect a catalog in tests, use the `#[cfg(test)]` `CatalogAssertions` trait from the same file rather than `get_table(&rel(..))`; only tests of the lookup API itself call `get_table` directly. On failure each method panics with a message listing what the catalog actually holds:
- `catalog.table("orders") -> &TableState`
- `assert_table("orders")` / `assert_no_table("audit.orders")`
- `assert_partition_children("parent", &["child"])` - compared as sets
- `assert_index_owner("idx_status", "orders")` / `assert_index_unregistered("idx_status")` - index names are bare

### Test Fixture Repos

Integration tests use fixture repos in `tests/fixtures/repos/`:
- `all-rules/` - One violation per rule
- `bridge-table/` - PGM501 negative case: a bridge table whose composite PK `(x_id, y_id)` covers both FKs
- `catalog-ops/` - Catalog operations (DROP NOT NULL, DROP CONSTRAINT, SET/DROP DEFAULT, hash indexes)
- `clean/` - All migrations correct, expect 0 findings
- `enterprise/` - Realistic 31-file migration history
- `fk-with-later-index/` - PGM501 FK detection across migration boundaries
- `go-migrate/` - go-migrate convention (.up.sql / .down.sql pairs)
- `liquibase-fresh/` - Liquibase bridge on a fresh repo with every changeset changed, expect only PGM402
- `liquibase-multi-schema/` - Liquibase bridge with multi-schema changelogs
- `liquibase-xml/` - Liquibase XML bridge/update-sql parsing
- `multi-schema/` - Cross-schema FKs, name isolation, drop isolation
- `partition-lock-hazard/` - PGM024: a repartition of 5 partitioned parents in one transaction (one finding per parent), and the safe DETACH CONCURRENTLY then DROP
- `schema-qualified/` - Schema-qualified names and cross-schema references
- `suppressed/` - All violations suppressed, expect 0 findings

## Important Constraints

- No `unwrap()` or `expect()` in library code - use `thiserror` for error handling
- All public functions require doc comments
- Index column order must be preserved (affects PGM508's prefix-based redundant-index detection via `is_prefix()`)
- `pg_query` uses libpg_query bindings - use the pg_query_spike.rs test file to figure out the AST.
- Down migrations (`.down.sql` / `_down.sql` suffix) always get INFO severity cap (PGM901)
- Error paths: config errors exit 2, parse failures on individual files warn and continue
- Never write inline spike tests or temporary test files — always add spike tests to `tests/pg_query_spike.rs`

## Key Files Reference

- **Specification**: `SPEC.md` - Complete rule definitions, input formats, output formats

These documents are the source of truth for behavior and architecture. When implementing, refer to:
- `SPEC.md` for "what" (requirements, rule behavior)
