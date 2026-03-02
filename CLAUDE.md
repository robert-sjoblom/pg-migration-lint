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
3. **Normalize** (`src/normalize.rs`): Assigns `default_schema` to unqualified names so catalog keys are schema-qualified
4. **Catalog** (`src/catalog/`): Replays all migrations to build table state
5. **Rules** (`src/rules/`): Lints changed files against rules (PGM001-PGM022, PGM101-PGM109, PGM201-PGM205, PGM301-PGM303, PGM401-PGM403, PGM501-PGM509)
6. **Output** (`src/output/`): Emits SARIF, SonarQube JSON, or text

### Intermediate Representation (IR)

The tool converts SQL AST into a simplified IR layer to decouple rules from parser internals. The IR types are fully defined in `phase_0_type_trait_definitions.md`.

Key IR nodes (`src/parser/ir.rs`):
```rust
pub enum IrNode {
    CreateTable(CreateTable),
    AlterTable(AlterTable),
    CreateIndex(CreateIndex),
    DropIndex(DropIndex),
    DropTable(DropTable),
    DropSchema(DropSchema),
    TruncateTable(TruncateTable),
    InsertInto(InsertInto),
    UpdateTable(UpdateTable),
    DeleteFrom(DeleteFrom),
    Cluster(Cluster),
    VacuumFull(VacuumFull),
    AlterIndexAttachPartition { parent_index_name, child_index_name },
    RenameTable { name, new_name },
    RenameColumn { table, old_name, new_name },
    Ignored { raw_sql: String },        // Parsed but not relevant (GRANT, COMMENT ON)
    Unparseable { raw_sql: String, table_hint: Option<String> },
}
```

`CreateTable` uses a `TablePersistence` enum (`Permanent`, `Unlogged`, `Temporary`) instead of a boolean `temporary` field.

Supporting types:
- `QualifiedName` - schema-qualified name with `catalog_key()` returning `"schema.name"` after normalization
- `ColumnDef { name, type_name, nullable, default_expr, is_inline_pk, is_serial }`
- `TypeName { name, modifiers }` - e.g., `varchar(100)` has modifiers `[100]`
- `DefaultExpr` - enum: `Literal`, `FunctionCall { name, args }`, `Other`
- `TableConstraint` - enum: `PrimaryKey`, `ForeignKey`, `Unique`, `Check`, `Exclude`
- `AlterTableAction` - enum: `AddColumn`, `DropColumn`, `AddConstraint`, `AlterColumnType`, `SetNotNull`, `DropNotNull`, `SetDefault`, `DropDefault`, `DropConstraint`, `ValidateConstraint`, `Other`

Each statement is wrapped in `Located<IrNode>` with `SourceSpan` for line number tracking.

### Table Catalog

Built by replaying migrations in order. Represents schema state at each migration point (`src/catalog/types.rs`):

```rust
pub struct Catalog {
    tables: HashMap<String, TableState>,
}

pub struct TableState {
    pub name: String,
    pub display_name: String,
    pub columns: Vec<ColumnState>,
    pub indexes: Vec<IndexState>,
    pub constraints: Vec<ConstraintState>,
    pub has_primary_key: bool,
    pub incomplete: bool,       // true if unparseable SQL touched this table
    pub is_partitioned: bool,
    pub partition_by: Option<PartitionByInfo>,
    pub parent_table: Option<String>,
}
```

Key methods on `TableState`:
- `get_column(&self, name: &str) -> Option<&ColumnState>`
- `has_covering_index(&self, fk_columns: &[String]) -> bool` - btree prefix matching for PGM501 (skips non-btree, partial, ONLY indexes)
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

Each rule implements the `Rule` trait (`src/rules/mod.rs`):

```rust
pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    fn default_severity(&self) -> Severity;
    fn description(&self) -> &'static str;
    fn explain(&self) -> &'static str;  // For --explain output
    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>;
}
```

`LintContext` provides:
```rust
pub struct LintContext<'a> {
    pub catalog_before: &'a Catalog,  // State BEFORE applying current unit
    pub catalog_after: &'a Catalog,   // State AFTER applying current unit
    pub tables_created_in_change: &'a HashSet<String>,  // For "new table" detection
    pub run_in_transaction: bool,     // From MigrationUnit metadata
    pub is_down: bool,                // Down/rollback migration flag
    pub file: &'a PathBuf,
}
```

Rules use `catalog_before` to check if tables are pre-existing (PGM001/002) and `catalog_after` for post-file checks (PGM501/502/503). The two-catalog approach enables single-pass replay without needing separate replay runs.

#### Rule Severities
- **CRITICAL**: Causes downtime or data corruption (e.g., missing `CONCURRENTLY`)
- **MAJOR**: Performance issues or schema problems (e.g., missing FK index, no primary key)
- **WARNING**: Potentially unintended behavior
- **INFO**: Informational findings

#### Rules

**0xx — Unsafe DDL:**
**1xx — Type Anti-patterns:**
**2xx — Destructive Operations:**
**3xx — DML in Migrations:**
**4xx — Idempotency Guards:**
**5xx — Schema Design:**
**9xx — Meta-behavior:**
- **PGM901**: Down migrations (all findings capped to INFO) — meta-behavior, not a standalone rule

## Development Workflow

### Adding a New Rule

1. Define rule in `src/rules/pgmXXX.rs`
2. Implement the `Rule` trait with all methods:
   - `id()` - stable identifier like "PGM001"
   - `default_severity()` - Critical, Major, Warning, Info
   - `description()` - short summary
   - `explain()` - detailed explanation with examples and fixes
   - `check()` - main rule logic
3. Wire up dispatch arms in `impl Rule for RuleId` in `src/rules/rule_id.rs` (`default_severity`, `description`, `explain`, `check`)
4. Add component test fixtures in `tests/fixtures/` with positive and negative cases
5. Add unit tests for helper functions in the rule file
6. Add integration test in fixture repo `tests/fixtures/repos/all-rules/`

### Working with IR

When modifying the IR layer (`src/parser/ir.rs`):
- Changes cascade to parser, catalog replay, and all rules
- Update the IR contract first, then notify affected components
- Preserve `SourceSpan` (line numbers) for accurate error reporting

### Catalog Replay

The replay engine (`src/catalog/replay.rs`) has a single function:
```rust
pub fn apply(catalog: &mut Catalog, unit: &MigrationUnit);
```

The pipeline in `main.rs` drives replay:
```rust
for unit in history.units {
    if unit.source_file is in changed_files {
        let catalog_before = catalog.clone();
        apply(&mut catalog, &unit);
        lint(&unit, &catalog_before, &catalog);  // catalog_after is current catalog
    } else {
        apply(&mut catalog, &unit);
    }
}
```

The `apply` function handles all `IrNode` variants.

### Catalog Test Builders

Use the builder pattern for catalog state assertions (defined in `src/catalog/builder.rs`):
```rust
let catalog = CatalogBuilder::new()
    .table("orders", |t| {
        t.column("id", "int", false)
         .column("status", "text", true)
         .index("idx_status", &["status"], false)
         .pk(&["id"])
         .fk("fk_customer", &["customer_id"], "customers", &["id"])
    })
    .build();
```

### Test Fixture Repos

Integration tests use fixture repos in `tests/fixtures/repos/`:
- `clean/` - All migrations correct, expect 0 findings
- `all-rules/` - One violation per rule, expect 11 findings
- `suppressed/` - All violations suppressed, expect 0 findings
- `liquibase-xml/` - Tests Liquibase bridge/update-sql parsing

## Important Constraints

- No `unwrap()` or `expect()` in library code - use `thiserror` for error handling
- All public functions require doc comments
- Index column order must be preserved (affects FK covering index checks via `has_covering_index()`)
- `pg_query` uses libpg_query bindings - use the pg_query_spike.rs test file to figure out the AST.
- Down migrations (`.down.sql` / `_down.sql` suffix) always get INFO severity cap (PGM901)
- Error paths: config errors exit 2, parse failures on individual files warn and continue
- Never write inline spike tests or temporary test files — always add spike tests to `tests/pg_query_spike.rs`

## Key Files Reference

- **Specification**: `SPEC.md` - Complete rule definitions, input formats, output formats

These documents are the source of truth for behavior and architecture. When implementing, refer to:
- `SPEC.md` for "what" (requirements, rule behavior)
