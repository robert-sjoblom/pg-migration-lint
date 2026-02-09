# pg-migration-lint — Implementation Plan

## 1. Agent Architecture

### 1.1 Overseer Agent

Single coordinating agent. Responsibilities:

- Scaffolds the repo, `Cargo.toml`, and module stubs with trait definitions and shared types
- Defines interface contracts (traits, IR types, catalog types) BEFORE dispatching subagents
- Dispatches work to subagents with precise context: the spec section, the interface contract, and example inputs/outputs
- Reviews PRs from subagents, runs `cargo check` / `cargo test` to verify integration
- Resolves conflicts when subagent outputs don't compose cleanly
- Owns `main.rs`, `config.rs`, and the integration test suite

### 1.2 Subagents

Each subagent works in its own branch against a defined interface. They receive:

- The relevant spec section(s)
- The trait/type definitions they must implement
- Sample input fixtures and expected output
- Explicit boundaries: what files they own, what they must not modify

| Agent | Owns | Depends On |
|-------|------|------------|
| **Parser Agent** | `parser/`, `input/sql.rs` | IR types (from overseer) |
| **Liquibase Agent** | `input/liquibase_*.rs`, `bridge/` | IR types |
| **Catalog Agent** | `catalog/` | IR types |
| **Rules Agent** | `rules/` | IR types, catalog types |
| **Output Agent** | `output/`, `suppress.rs` | Finding types (from rules) |

---

## 2. Phases

### Phase 0 — Scaffold & Contracts (Overseer)

**Goal**: establish the codebase skeleton and every shared type so subagents can work in parallel.

**Deliverables**:

```
Cargo.toml              # workspace with dependencies: pg_query, clap, serde, toml, quick-xml
src/main.rs             # CLI skeleton (clap), dispatches to pipeline
src/config.rs           # Config struct + TOML deserialization
src/parser/ir.rs        # Complete IR type definitions (§3.2 of spec)
src/catalog/types.rs    # TableState, ColumnState, IndexState, ConstraintState
src/rules/mod.rs        # Rule trait + Finding struct + Severity enum
src/output/mod.rs       # OutputFormat enum, Reporter trait
tests/fixtures/         # 10-15 sample migration files covering all rule triggers
```

**Key types to define**:

```rust
// ir.rs
pub enum IrNode { CreateTable { .. }, AlterTable { .. }, CreateIndex { .. }, ... }
pub struct Column { name, type_name, nullable, default_expr }
pub enum Constraint { PrimaryKey { .. }, ForeignKey { .. }, Unique { .. }, Check { .. } }

// catalog/types.rs  
pub struct Catalog { tables: HashMap<String, TableState> }
pub struct TableState { columns, indexes, constraints, has_primary_key, incomplete }

// rules/mod.rs
pub trait Rule { fn id(&self) -> &str; fn check(&self, ir: &[IrNode], catalog: &Catalog, ctx: &LintContext) -> Vec<Finding>; }
pub struct Finding { rule_id, severity, message, file, line, end_line }
pub enum Severity { Blocker, Critical, Major, Minor, Info }

// output/mod.rs
pub trait Reporter { fn emit(&self, findings: &[Finding], path: &Path) -> Result<()>; }
```

**Exit criteria**: `cargo check` passes. All type stubs compile. Fixture files exist.

---

### Phase 1 — Core Pipeline (Parallel)

Dispatch all 5 subagents simultaneously after Phase 0.

#### 1A — Parser Agent

**Scope**: `src/parser/pg_query.rs`, `src/parser/mod.rs`, `src/input/sql.rs`

**Contract**: implement `pub fn parse_sql(input: &str) -> Result<Vec<(IrNode, SourceSpan)>>` where `SourceSpan` carries byte offset + line number from `pg_query`.

**Tasks**:
1. Wire up `pg_query` crate to parse SQL strings
2. Walk the AST and convert to IR nodes. Handle:
   - `CREATE TABLE` (with inline constraints, column defaults)
   - `ALTER TABLE` (add column, add constraint, drop column, alter column type)
   - `CREATE INDEX` / `DROP INDEX` (concurrent flag)
   - `DROP TABLE`
   - `DO $$ ... $$` blocks → `IrNode::Unparseable`
3. Multi-statement files: split on `;`, parse each, preserve line offsets
4. Unit tests against fixture files

**Acceptance**: every fixture file parses to expected IR. Unparseable blocks don't panic.

#### 1B — Liquibase Agent

**Scope**: `src/input/liquibase_bridge.rs`, `src/input/liquibase_updatesql.rs`, `src/input/liquibase_xml.rs`, `bridge/`

**Contract**: implement `pub fn load_liquibase(config: &LiquibaseConfig, path: &Path) -> Result<Vec<MigrationUnit>>` where `MigrationUnit = { changeset_id, sql: String, source_file, source_line, run_in_transaction: bool }`.

**Tasks**:
1. **Bridge jar** (Java): Maven project, embed Liquibase dependency, read changelog, iterate changesets, emit JSON to stdout. ~100 LOC.
2. **Bridge Rust side**: shell out to `java -jar`, parse JSON into `MigrationUnit`
3. **update-sql fallback**: shell out to `liquibase update-sql`, heuristic parsing of output
4. **XML fallback**: `quick-xml` based parser for the 8 change types. Map each to raw SQL strings. Track `<changeSet>` line numbers.
5. Strategy selection: try bridge → update-sql → xml-fallback based on config + what's available
6. Unit tests for each strategy

**Acceptance**: XML fixtures with multiple changesets produce correct `MigrationUnit` sequences via all three strategies (bridge tested with Java in CI).

#### 1C — Catalog Agent

**Scope**: `src/catalog/replay.rs`, `src/catalog/mod.rs`

**Contract**: implement `pub fn replay(migrations: &[Migration]) -> Catalog` where `Migration = { units: Vec<(IrNode, SourceSpan)>, file_path }`.

**Tasks**:
1. Process IR nodes in order, build/mutate `Catalog`
2. `CreateTable` → insert `TableState` with columns, inline constraints, inline indexes
3. `AlterTable(AddColumn)` → push to table's columns
4. `AlterTable(AddConstraint)` → push to constraints, set `has_primary_key` if PK
5. `CreateIndex` → push to table's indexes (preserve column order)
6. `DropTable` → remove from catalog
7. `DropIndex` → remove from table's indexes
8. `AlterTable(AlterColumnType)` → update column type
9. `AlterTable(DropColumn)` → remove column, remove affected indexes/constraints
10. `Unparseable` referencing a known table → set `incomplete = true`
11. Unit tests: replay a sequence of fixtures and assert catalog state

**Acceptance**: catalog correctly represents schema state after replaying fixture migrations. Column types tracked. Index column order preserved.

#### 1D — Rules Agent

**Scope**: `src/rules/pgm001.rs` through `src/rules/pgm011.rs`, `src/rules/explain.rs`

**Contract**: implement the `Rule` trait for each rule. Receives IR nodes for the file being linted + the catalog at that point + a `LintContext` carrying the set of changed files.

**Tasks**:
1. Implement all 11 rules per spec §4.2
2. PGM001/002: check `CreateIndex`/`DropIndex` concurrent flag, consult changed files for "new table" detection
3. PGM003: collect FKs and indexes after full file processing, do prefix matching on column order
4. PGM004/005: check after full file, inspect `has_primary_key` and unique-not-null
5. PGM006: check `concurrent` flag + `run_in_transaction` from `MigrationUnit` context
6. PGM007: pattern match `default_expr` against volatile function list + any function call
7. PGM009: parse old/new types, check against binary-coercible allowlist
8. PGM010: `ADD COLUMN NOT NULL` without default on existing table
9. PGM011: `DROP COLUMN` on existing table → INFO
10. PGM008: down-migration severity cap (wrap any rule, cap at INFO)
11. `--explain` text for each rule: failure mode, example, fix
12. Unit tests per rule with targeted fixtures

**Acceptance**: each rule has ≥3 test cases (true positive, true negative, edge case). Suppression-aware.

#### 1E — Output Agent

**Scope**: `src/output/sarif.rs`, `src/output/sonarqube.rs`, `src/output/text.rs`, `src/suppress.rs`

**Contract**: implement `Reporter` trait for each format. Implement `pub fn parse_suppressions(source: &str) -> Suppressions` for inline comment handling.

**Tasks**:
1. **Suppression parser**: scan SQL comments for `-- pgm-lint:suppress PGMnnn` (next-statement) and `-- pgm-lint:suppress-file PGMnnn` (file-level). Return a structure the rule engine queries.
2. **SARIF 2.1.0**: proper schema with `runs[].tool`, `runs[].results[]`, `physicalLocation` with line numbers. Validate against SARIF schema.
3. **SonarQube Generic Issue Import JSON**: per spec §7.1
4. **Text**: per spec §7.3
5. Unit tests: given a set of findings, assert each format's output

**Acceptance**: SARIF output validates against the SARIF JSON schema. SonarQube JSON matches the expected structure. Suppressions correctly filter findings.

---

### Phase 2 — Integration (Overseer)

**Goal**: wire everything together in `main.rs` and validate end-to-end.

**Tasks**:
1. Implement the full pipeline in `main.rs`:
   - Load config
   - Discover and load migration files (SQL + Liquibase)
   - Parse all files to IR
   - Replay to build catalog
   - For each changed file: run all rules, apply suppressions, collect findings
   - Cap down-migration findings to INFO
   - Emit reports in configured formats
   - Exit with appropriate code based on `fail_on` threshold
2. Handle `--explain` early exit
3. Handle `--changed-files` / `--changed-files-from` parsing
4. Integration tests: end-to-end runs against fixture repos producing expected SARIF/JSON output
5. Error handling: config errors (exit 2), parse failures on individual files (warn + continue), total parse failure (exit 2)

**Exit criteria**: `cargo test` passes all unit + integration tests. `cargo build --release` produces a working binary.

---

### Phase 3 — Bridge Jar Build & Distribution (Overseer)

**Tasks**:
1. Maven build for `bridge/` producing a shaded/fat jar
2. GitHub Actions workflow:
   - Rust: `cargo build --release --target x86_64-unknown-linux-musl`
   - Java: `mvn package -f bridge/pom.xml`
   - Attach both to GitHub Release
3. Dockerfile: musl binary + JRE + bridge jar
4. CI example configs in `docs/`:
   - GitHub Actions workflow snippet using `upload-sarif`
   - Basic usage with `--changed-files`

---

### Phase 4 — Hardening (Overseer + targeted subagent dispatches)

1. Fuzz the parser with malformed SQL (the `pg_query` crate handles this, but test the IR conversion layer)
2. Test against real migration histories from your repos (anonymized if needed)
3. Performance profiling on a repo with hundreds of migration files
4. Edge cases: empty files, files with only comments, BOM markers, mixed encodings

---

## 3. Dependency Graph

```
Phase 0 (Overseer: scaffold + contracts)
    │
    ├──→ Phase 1A (Parser)      ──┐
    ├──→ Phase 1B (Liquibase)   ──┤
    ├──→ Phase 1C (Catalog)     ──┼──→ Phase 2 (Integration)
    ├──→ Phase 1D (Rules)       ──┤       │
    └──→ Phase 1E (Output)      ──┘       ├──→ Phase 3 (Distribution)
                                          └──→ Phase 4 (Hardening)
```

Phase 1 agents are fully parallel. Phase 2 blocks on all of Phase 1. Phases 3 and 4 can partially overlap.

---

## 4. Subagent Prompt Template

Each subagent receives a prompt structured as:

```
## Your Role
You are implementing the {component} for pg-migration-lint.

## Spec
{relevant spec sections, verbatim}

## Interface Contract
{trait definitions, type definitions, function signatures you must implement}

## Files You Own
{explicit list of files to create/modify}

## Files You Must Not Modify
Everything outside your owned files.

## Fixtures
{sample input files and expected outputs}

## Acceptance Criteria
{specific test cases that must pass}

## Constraints
- Do not add dependencies beyond what's in Cargo.toml
- All public functions must have doc comments
- All error paths must use thiserror, no unwrap/expect in library code
- Run cargo clippy before submitting
```

---

## 5. Risk Mitigation

| Risk | Mitigation |
|------|------------|
| IR type definitions insufficient for a rule | Overseer reviews all rules against IR before dispatching. Rules agent can flag missing IR variants → overseer adds them before other agents need them. |
| `pg_query` crate doesn't expose needed AST detail | Parser agent spikes on the hardest cases first (composite FK, column defaults with function calls). Fail fast. |
| Subagent outputs don't compile together | Overseer runs `cargo check` after merging each agent's branch. Fix interface mismatches immediately. |
| Bridge jar Liquibase version mismatch | Pin Liquibase version in `pom.xml`. Document supported range. |
| Replay ordering incorrect | Integration tests replay known migration sequences and assert catalog state at each step. |

---

## 6. Estimated Effort

| Phase | Effort |
|-------|--------|
| Phase 0 — Scaffold | 1 session |
| Phase 1A–1E — Parallel | 1–2 sessions each (run simultaneously) |
| Phase 2 — Integration | 1–2 sessions |
| Phase 3 — Distribution | 1 session |
| Phase 4 — Hardening | 1–2 sessions |
| **Total wall-clock** | **~5–6 sessions** |

"Session" = one focused working session with an agent. Parallelism in Phase 1 is the key time savings.
