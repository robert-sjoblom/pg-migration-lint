# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`pg-migration-lint` is a Rust CLI tool that statically analyzes PostgreSQL migration files for safety and correctness issues. It builds an internal table catalog by replaying migration history, then lints only new/changed files. Outputs SARIF (GitHub Code Scanning) and SonarQube Generic Issue Import JSON.

## Key Commands

### Build and Test
```bash
cargo build                    # Build debug binary
cargo build --release          # Build optimized binary
cargo test                     # Run all tests
cargo clippy                   # Run linter
cargo check                    # Fast compilation check
cargo fmt                      # Format code
cargo test --features docgen   # Run all tests including docgen snapshot tests
```

### Running the Tool
```bash
# Lint all migrations
cargo run -- --config pg-migration-lint.toml

# Lint only changed files (typical CI usage)
cargo run -- --changed-files db/migrations/V042__add_index.sql,db/migrations/V043__add_fk.sql

# Explain a specific rule
cargo run -- --explain PGM001

# Override output format
cargo run -- --format text
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
5. **Rules** (`src/rules/`): Lints changed files against rules (PGM001-PGM019, PGM101-PGM106, PGM201-PGM204, PGM301-PGM303, PGM401-PGM403, PGM501-PGM506)
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
    TruncateTable(TruncateTable),
    InsertInto(InsertInto),
    UpdateTable(UpdateTable),
    DeleteFrom(DeleteFrom),
    RenameTable(RenameTable),
    RenameColumn(RenameColumn),
    Ignored { raw_sql: String },        // Parsed but not relevant (GRANT, COMMENT ON)
    Unparseable { raw_sql: String, table_hint: Option<String> },
}
```

`CreateTable` uses a `TablePersistence` enum (`Permanent`, `Unlogged`, `Temporary`) instead of a boolean `temporary` field.

Supporting types:
- `QualifiedName` - schema-qualified name with `catalog_key()` returning `"schema.name"` after normalization
- `ColumnDef { name, type_name, nullable, default_expr, is_inline_pk }`
- `TypeName { name, modifiers }` - e.g., `varchar(100)` has modifiers `[100]`
- `DefaultExpr` - enum: `Literal`, `FunctionCall { name, args }`, `Other`
- `TableConstraint` - enum: `PrimaryKey`, `ForeignKey`, `Unique`, `Check`
- `AlterTableAction` - enum: `AddColumn`, `DropColumn`, `AddConstraint`, `AlterColumnType`, `Other`

Each statement is wrapped in `Located<IrNode>` with `SourceSpan` for line number tracking.

### Table Catalog

Built by replaying migrations in order. Represents schema state at each migration point (`src/catalog/types.rs`):

```rust
pub struct Catalog {
    tables: HashMap<String, TableState>,
}

pub struct TableState {
    pub name: String,
    pub columns: Vec<ColumnState>,
    pub indexes: Vec<IndexState>,
    pub constraints: Vec<ConstraintState>,
    pub has_primary_key: bool,
    pub incomplete: bool,  // true if unparseable SQL touched this table
}
```

Key methods on `TableState`:
- `get_column(&self, name: &str) -> Option<&ColumnState>`
- `has_covering_index(&self, fk_columns: &[String]) -> bool` - prefix matching for PGM501
- `has_unique_not_null(&self) -> bool` - for PGM503 detection

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
- **PGM001**: Missing `CONCURRENTLY` on `CREATE INDEX`
- **PGM002**: Missing `CONCURRENTLY` on `DROP INDEX`
- **PGM003**: `CONCURRENTLY` inside transaction
- **PGM006**: Volatile default on column (forces table rewrite)
- **PGM007**: `ALTER COLUMN TYPE` causing table rewrite
- **PGM008**: `ADD COLUMN NOT NULL` without default
- **PGM009**: `DROP COLUMN` on existing table
- **PGM010**: `DROP COLUMN` silently removes unique constraint
- **PGM011**: `DROP COLUMN` silently removes primary key
- **PGM012**: `DROP COLUMN` silently removes foreign key
- **PGM013**: `SET NOT NULL` requires ACCESS EXCLUSIVE lock
- **PGM014**: `ADD FOREIGN KEY` without `NOT VALID`
- **PGM015**: `ADD CHECK` without `NOT VALID`
- **PGM016**: `ADD PRIMARY KEY` without prior `UNIQUE` index
- **PGM017**: `ADD UNIQUE` without `USING INDEX`
- **PGM018**: `CLUSTER` on existing table
- **PGM019**: `ADD EXCLUDE` constraint on existing table

**1xx — Type Anti-patterns:**
- **PGM101–105**: PostgreSQL "Don't Do This" type rules (timestamp, timestamp(0), char(n), money, serial)
- **PGM106**: Don't use `json` (use `jsonb`)

**2xx — Destructive Operations:**
- **PGM201**: `DROP TABLE` on existing table
- **PGM202**: `DROP TABLE CASCADE` on existing table
- **PGM203**: `TRUNCATE TABLE` on existing table
- **PGM204**: `TRUNCATE TABLE CASCADE` on existing table

**3xx — DML in Migrations:**
- **PGM301**: `INSERT INTO` existing table in migration
- **PGM302**: `UPDATE` on existing table in migration
- **PGM303**: `DELETE FROM` existing table in migration

**4xx — Idempotency Guards:**
- **PGM401**: Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`
- **PGM402**: Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`
- **PGM403**: `CREATE TABLE IF NOT EXISTS` for already-existing table

**5xx — Schema Design:**
- **PGM501**: Foreign key without covering index
- **PGM502**: Table without primary key
- **PGM503**: `UNIQUE NOT NULL` used instead of primary key
- **PGM504**: `RENAME TABLE` on existing table
- **PGM505**: `RENAME COLUMN` on existing table
- **PGM506**: `CREATE UNLOGGED TABLE`

**9xx — Meta-behavior:**
- **PGM901**: Down migrations (all findings capped to INFO) — meta-behavior, not a standalone rule

### Suppression System

Inline SQL comments control rule suppression:

```sql
-- Next-statement suppression
-- pgm-lint:suppress PGM001
CREATE INDEX idx_foo ON bar (col);

-- File-level suppression (must appear before SQL statements)
-- pgm-lint:suppress-file PGM001,PGM501
```

### Liquibase Support

Two-tier strategy for Liquibase XML processing (JRE required):

1. **Preferred**: `liquibase-bridge.jar` - Embeds Liquibase, produces JSON with exact changeset-to-SQL-to-line mapping
2. **Secondary**: `liquibase update-sql` - Less structured output, heuristic parsing

The bridge jar (`bridge/`) is a separate Java subproject (~100 LOC) built with Maven.

## Development Workflow

### Adding a New Rule

1. Define rule in `src/rules/pgmXXX.rs`
2. Implement the `Rule` trait with all methods:
   - `id()` - stable identifier like "PGM001"
   - `default_severity()` - Critical, Major, Warning, Info
   - `description()` - short summary
   - `explain()` - detailed explanation with examples and fixes
   - `check()` - main rule logic
3. Register in `RuleRegistry::register_defaults()` in `src/rules/mod.rs`
4. Add component test fixtures in `tests/fixtures/` with positive and negative cases
5. Add unit tests for helper functions in the rule file
6. Add integration test in fixture repo `tests/fixtures/repos/all-rules/`

See `test_plan.md` for comprehensive test case examples per rule.

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

This single-pass design with selective cloning is more efficient than dual-replay and provides rules with both before/after states.

The `apply` function:
- `CreateTable` → insert into catalog with columns, constraints, indexes
- `AlterTable(AddColumn)` → push to columns, update types as needed
- `AlterTable(AddConstraint)` → push to constraints, set `has_primary_key` if PK
- `CreateIndex` → push to indexes (preserving column order)
- `DropTable` → remove from catalog entirely
- `DropIndex` → remove from table's indexes
- `AlterTable(AlterColumnType)` → update column type_name
- `AlterTable(DropColumn)` → remove column and affected indexes/constraints
- `Unparseable` with table_hint → mark table `incomplete = true`

### Changed File Detection

The tool does NOT invoke git directly. Changed files are passed via:
```bash
--changed-files file1.sql,file2.sql
--changed-files-from changed.txt
```

CI determines the diff (e.g., `git diff --name-only origin/main...HEAD`).

## Configuration

Default config file: `pg-migration-lint.toml`

```toml
[migrations]
paths = ["db/migrations", "db/changelog.xml"]
strategy = "liquibase"  # or "filename_lexicographic"
include = ["*.sql", "*.xml"]
exclude = ["**/test/**"]
default_schema = "public"  # Schema for unqualified table names

[liquibase]
bridge_jar_path = "tools/liquibase-bridge.jar"
binary_path = "/usr/local/bin/liquibase"
strategy = "auto"  # tries bridge → update-sql

[output]
formats = ["sarif", "sonarqube"]
dir = "build/reports/migration-lint"

[cli]
fail_on = "critical"  # exit non-zero if findings meet/exceed this severity
```

## Multi-Agent Implementation Approach

This project follows a phased multi-agent architecture (see `implementation_plan.md` and `phase_0_type_trait_definitions.md`):

**Phase 0 (Overseer)**: Scaffold repo, define all shared types/traits
- All types are defined in `phase_0_type_trait_definitions.md`
- Creates `src/` module stubs with trait definitions
- Creates test fixtures and catalog builder helpers

**Phase 1 (Parallel Subagents)**:
- **Parser Agent**: `src/parser/pg_query.rs`, `src/input/sql.rs` - pg_query → IR conversion
- **Liquibase Agent**: `src/input/liquibase_*.rs`, `bridge/` - Two-tier Liquibase support (bridge jar + update-sql)
- **Catalog Agent**: `src/catalog/replay.rs` - Single-pass replay with `apply()` function
- **Rules Agent**: `src/rules/pgm001.rs` through `pgm011.rs` - Implement all 11 rules
- **Output Agent**: `src/output/sarif.rs`, `sonarqube.rs`, `text.rs`, `src/suppress.rs`

**Phase 2 (Overseer)**: Wire pipeline in `main.rs`, integration tests

Each subagent receives:
- Relevant spec sections
- The trait/type definitions they must implement (from phase_0_type_trait_definitions.md)
- Sample input fixtures and expected outputs
- Explicit file ownership boundaries

## Output Formats

### SARIF 2.1.0
Standard GitHub Code Scanning format. Upload via `github/codeql-action/upload-sarif@v3`.

### SonarQube Generic Issue Import
```json
{
  "issues": [{
    "engineId": "pg-migration-lint",
    "ruleId": "PGM001",
    "severity": "CRITICAL",
    "type": "BUG",
    "primaryLocation": {
      "message": "...",
      "filePath": "...",
      "textRange": { "startLine": 3, "endLine": 3 }
    }
  }]
}
```

### Text
Human-readable for local development:
```
CRITICAL PGM001 db/migrations/V042__add_index.sql:3
  CREATE INDEX on existing table 'orders' should use CONCURRENTLY.
```

## Distribution

- Primary: Statically linked `x86_64-unknown-linux-musl` binary
- Secondary: OCI container with JRE + bridge jar
- Bridge jar: Separate release artifact for Liquibase XML support

## Testing Strategy

The project uses a four-layer testing approach (see `test_plan.md`):

1. **Unit tests**: Pure functions, type parsing, suppression parsing, FK prefix matching
2. **Component tests**: Test module boundaries in isolation (parser, catalog replay, each rule)
3. **Integration tests**: Full pipeline against fixture repos in `tests/fixtures/repos/`
4. **E2E tests**: Invoke compiled binary as subprocess, assert exit codes and file outputs

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

See `test_plan.md` sections 3-5 for comprehensive test case coverage per rule.

## Important Constraints

- No `unwrap()` or `expect()` in library code - use `thiserror` for error handling
- All public functions require doc comments
- Index column order must be preserved (affects FK covering index checks via `has_covering_index()`)
- `pg_query` uses libpg_query bindings - consult its AST structure for parser work
- Down migrations (`.down.sql` / `_down.sql` suffix) always get INFO severity cap (PGM901)
- Error paths: config errors exit 2, parse failures on individual files warn and continue
- Rules are `Send + Sync` to support parallel execution in the future
- TypeName stores modifiers as `Vec<i64>` for types like `varchar(100)`, `numeric(10,2)`

## Key Files Reference

- **Specification**: `SPEC.md` - Complete rule definitions, input formats, output formats
- **Implementation plan**: `implementation_plan.md` - Multi-agent architecture, phases, dependencies
- **Type definitions**: `phase_0_type_trait_definitions.md` - All Rust types and traits
- **Test plan**: `test_plan.md` - Comprehensive test strategy and test case catalog

These documents are the source of truth for behavior and architecture. When implementing, refer to:
- `SPEC.md` for "what" (requirements, rule behavior)
- `implementation_plan.md` for "how" (architecture, phasing)
- `phase_0_type_trait_definitions.md` for "interfaces" (exact type signatures)
- `test_plan.md` for "validation" (test cases, assertions)
