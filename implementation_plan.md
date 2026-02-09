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
Cargo.toml              # workspace with dependencies: pg_query, clap, serde, toml, quick-xml, insta, proptest
src/main.rs             # CLI skeleton (clap), dispatches to pipeline
src/config.rs           # Config struct + TOML deserialization
src/parser/ir.rs        # Complete IR type definitions (§3.2 of spec)
src/catalog/types.rs    # TableState, ColumnState, IndexState, ConstraintState
src/catalog/builder.rs  # CatalogBuilder test harness (§9 of test_plan.md) — PRIORITY
src/rules/mod.rs        # Rule trait + Finding struct + Severity enum
src/output/mod.rs       # OutputFormat enum, Reporter trait
tests/fixtures/         # Fixture files per test_plan.md §4.1
tests/fixtures/repos/   # Fixture repos for integration tests (clean/, all-rules/, etc.)
```

**`CatalogBuilder` (test_plan.md §9) is a Phase 0 deliverable, not deferred.** Both the Catalog Agent and Rules Agent depend on it for every test. Build it before dispatching subagents. Target API:

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

// catalog/builder.rs — TEST HARNESS, PHASE 0 PRIORITY
// Used by Catalog Agent and Rules Agent for all component tests.
// Must be complete and documented before subagents are dispatched.
pub struct CatalogBuilder { .. }
impl CatalogBuilder {
    pub fn new() -> Self;
    pub fn table(self, name: &str, f: impl FnOnce(&mut TableBuilder)) -> Self;
    pub fn build(self) -> Catalog;
}
pub struct TableBuilder { .. }
impl TableBuilder {
    pub fn column(&mut self, name: &str, type_name: &str, nullable: bool) -> &mut Self;
    pub fn index(&mut self, name: &str, columns: &[&str], unique: bool) -> &mut Self;
    pub fn pk(&mut self, columns: &[&str]) -> &mut Self;
    pub fn fk(&mut self, name: &str, columns: &[&str], ref_table: &str, ref_columns: &[&str]) -> &mut Self;
}
```

**Exit criteria**: `cargo check` passes. All type stubs compile. `CatalogBuilder` is functional and tested. Fixture files and fixture repos exist. Phase 0 spike completed.

#### Phase 0 Spike — `pg_query` Type Canonicalization

Before dispatching subagents, verify `pg_query`'s behavior on:

1. **Type aliases**: parse `int`, `integer`, `int4`, `bool`, `boolean`, `varchar`, `character varying`, `serial`, `bigserial`. Record the canonical type name string the crate returns for each. The PGM009 binary-coercible allowlist must use these canonical forms.
2. **`serial` expansion**: confirm that `CREATE TABLE t (id serial)` parses to `integer` column + `DEFAULT nextval(...)`. Determines whether PGM007 needs `nextval` in its known volatile function list.
3. **Inline vs table-level constraints**: parse both `CREATE TABLE foo (baz int PRIMARY KEY)` and `CREATE TABLE foo (baz int, PRIMARY KEY (baz))`. Document where each lands in the AST. Same for inline FK (`REFERENCES`) vs table-level `FOREIGN KEY`, and inline `UNIQUE` vs table-level.

Record results in `docs/pg_query_spike.md`. The Parser Agent, Catalog Agent, and Rules Agent all reference this document.

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
   - `DO $ ... $` blocks → `IrNode::Unparseable`
3. Multi-statement files: split on `;`, parse each, preserve line offsets
4. Unit tests per test_plan.md §2.1 (IR Construction)
5. Property-based tests per test_plan.md §7 (`parse_sql` never panics on arbitrary input)

**Acceptance**: every fixture file parses to expected IR. Unparseable blocks don't panic. `proptest` suite passes. Inline and table-level constraint syntax both produce correct IR nodes per `docs/pg_query_spike.md`. Type names use canonical forms from the spike.

#### 1B — Liquibase Agent

**Scope**: `src/input/liquibase_bridge.rs`, `src/input/liquibase_updatesql.rs`, `src/input/liquibase_xml.rs`, `bridge/`

**Contract**: implement `pub fn load_liquibase(config: &LiquibaseConfig, path: &Path) -> Result<Vec<MigrationUnit>>` where `MigrationUnit = { changeset_id, sql: String, source_file, source_line, run_in_transaction: bool }`.

**Tasks**:
1. **Bridge jar** (Java): Maven project, embed Liquibase dependency, read changelog, iterate changesets, emit JSON to stdout. ~100 LOC.
2. **Bridge Rust side**: shell out to `java -jar`, parse JSON into `MigrationUnit`
3. **update-sql fallback**: shell out to `liquibase update-sql`, heuristic parsing of output
4. **XML fallback**: `quick-xml` based parser for the 8 change types. Map each to raw SQL strings. Track `<changeSet>` line numbers.
5. Strategy selection: try bridge → update-sql → xml-fallback based on config + what's available
6. Unit tests for each strategy per test_plan.md §6 (bridge tests gated behind `PGM_TEST_JAVA=1`)

**Acceptance**: XML fixtures with multiple changesets produce correct `MigrationUnit` sequences via all three strategies. Bridge tests pass under `PGM_TEST_JAVA=1`. Fallback gracefully activates when Java is unavailable.

#### 1C — Catalog Agent

**Scope**: `src/catalog/replay.rs`, `src/catalog/mod.rs`

**Contract**: implement `pub fn apply(catalog: &mut Catalog, unit: &MigrationUnit)` which applies a single unit's IR nodes to mutate the catalog. The pipeline calls this in a loop — no bulk replay function needed.

**Tasks**:
1. Implement `apply()`: process IR nodes in a unit sequentially, mutate `Catalog`
2. `CreateTable` → insert `TableState` with columns, inline constraints, inline indexes
3. `AlterTable(AddColumn)` → push to table's columns
4. `AlterTable(AddConstraint)` → push to constraints, set `has_primary_key` if PK
5. `CreateIndex` → push to table's indexes (preserve column order)
6. `DropTable` → remove from catalog
7. `DropIndex` → remove from table's indexes
8. `AlterTable(AlterColumnType)` → update column type
9. `AlterTable(DropColumn)` → remove column, remove affected indexes/constraints
10. `Unparseable` referencing a known table → set `incomplete = true`
11. Component tests per test_plan.md §3.1 (Catalog Replay), using `CatalogBuilder` for expected-state assertions. Tests call `apply()` in sequence and assert catalog state after each call.

**Acceptance**: catalog correctly represents schema state after applying fixture migration units in order. Column types tracked. Index column order preserved. All §3.1 test cases pass. **Inline and table-level constraint syntax produce identical `TableState`** — tested explicitly with paired fixtures (e.g., `CREATE TABLE (id int PRIMARY KEY)` vs `CREATE TABLE (id int, PRIMARY KEY (id))` must yield the same catalog entry).

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
12. Unit tests for helpers per test_plan.md §2.3 (binary-coercible cast matching) and §2.4 (FK index prefix matching)
13. Component tests per test_plan.md §3.2 (per-rule component tests), using `CatalogBuilder` to construct pre-built catalog state

**Acceptance**: each rule passes all test cases in test_plan.md §3.2. Helper functions pass all §2.3 and §2.4 cases. Suppression-aware.

#### 1E — Output Agent

**Scope**: `src/output/sarif.rs`, `src/output/sonarqube.rs`, `src/output/text.rs`, `src/suppress.rs`

**Contract**: implement `Reporter` trait for each format. Implement `pub fn parse_suppressions(source: &str) -> Suppressions` for inline comment handling.

**Tasks**:
1. **Suppression parser**: scan SQL comments for `-- pgm-lint:suppress PGMnnn` (next-statement) and `-- pgm-lint:suppress-file PGMnnn` (file-level). Return a structure the rule engine queries.
2. **SARIF 2.1.0**: proper schema with `runs[].tool`, `runs[].results[]`, `physicalLocation` with line numbers. Validate against SARIF schema.
3. **SonarQube Generic Issue Import JSON**: per spec §7.1
4. **Text**: per spec §7.3
5. Unit tests per test_plan.md §2.2 (suppression parsing) and §2.5 (output serialization)
6. Use `insta` crate for snapshot testing of serialized output against `.expected.json` / `.expected.sarif` / `.expected.txt` files
7. Property-based test per test_plan.md §7: SARIF serializer produces valid JSON for arbitrary `Finding` structs

**Acceptance**: SARIF output validates against the SARIF JSON schema. SonarQube JSON matches the expected structure. Suppressions pass all §2.2 test cases. Snapshots match expected output.

---

### Phase 2 — Integration (Overseer)

**Goal**: wire everything together in `main.rs` and validate end-to-end.

**Tasks**:
1. Implement the single-pass pipeline in `main.rs`:
   - Load config
   - Discover and load migration files (SQL + Liquibase) into ordered `MigrationHistory`
   - Parse `--changed-files` into a set
   - Single-pass loop over all units:
     - If unit is in changed files: clone catalog → apply unit → lint with (before=clone, after=mutated) → accumulate `tables_created_in_change`
     - If unit is not in changed files: apply unit only (advance catalog state)
   - Apply suppression filtering to collected findings
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

1. Fuzz testing per test_plan.md §7: `cargo-fuzz` on `parse_sql` and suppression parser. `proptest` on serializers.
2. Test against anonymized real migration histories from your repos (the fixture repo with both go-migrate and Liquibase migrations)
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
{relevant spec sections from spec.md, verbatim}

## Test Plan
{relevant sections from test_plan.md — the specific test tables your component must pass}

## Interface Contract
{trait definitions, type definitions, function signatures you must implement}

## Test Harness
You have access to CatalogBuilder (src/catalog/builder.rs) for constructing
catalog state in tests. Use it — do not construct TableState/Catalog manually.

## Files You Own
{explicit list of files to create/modify}

## Files You Must Not Modify
Everything outside your owned files.

## Fixtures
{sample input files in tests/fixtures/ and expected outputs}

## Acceptance Criteria
{specific test tables from test_plan.md that must pass — enumerate them}

## Constraints
- Do not add dependencies beyond what's in Cargo.toml
- All public functions must have doc comments
- All error paths must use thiserror, no unwrap/expect in library code
- Use insta for snapshot tests on serialized output
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