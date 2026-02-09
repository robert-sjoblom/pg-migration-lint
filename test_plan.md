# pg-migration-lint — Test Plan

## 1. Test Layers

```
┌─────────────────────────────────────┐
│  E2E: CLI binary against fixture    │  Few, slow, high confidence
│  repos. Assert exit codes + output  │
├─────────────────────────────────────┤
│  Integration: full pipeline in-     │  Moderate count, validates
│  process. Config → findings.        │  component composition
├─────────────────────────────────────┤
│  Component: one module boundary.    │  Bulk of the tests. Parser,
│  IR in → catalog/findings out.      │  catalog, rules, output.
├─────────────────────────────────────┤
│  Unit: pure functions, helpers,     │  Fast. Type parsing, prefix
│  matchers, serializers.             │  matching, suppression parsing.
└─────────────────────────────────────┘
```

---

## 2. Unit Tests

### 2.1 IR Construction (`parser/pg_query.rs`)

Each test: raw SQL string → assert specific IR node fields.

| Test | Input SQL | Assert |
|------|-----------|--------|
| Simple CREATE TABLE | `CREATE TABLE t (id int PRIMARY KEY, name text NOT NULL)` | `CreateTable` with 2 columns, PK constraint, name nullable=false |
| Composite FK inline | `CREATE TABLE t (a int, b int, FOREIGN KEY (a,b) REFERENCES p(x,y))` | `ForeignKey` with columns=["a","b"], ref_columns=["x","y"] |
| CREATE INDEX CONCURRENTLY | `CREATE INDEX CONCURRENTLY idx ON t (col)` | `CreateIndex { concurrent: true }` |
| DROP INDEX CONCURRENTLY | `DROP INDEX CONCURRENTLY idx` | `DropIndex { concurrent: true }` |
| ALTER ADD COLUMN with volatile default | `ALTER TABLE t ADD COLUMN ts timestamptz DEFAULT now()` | `AddColumn` with `default_expr` containing function call "now" |
| ALTER COLUMN TYPE | `ALTER TABLE t ALTER COLUMN x TYPE bigint` | `AlterColumnType { old_type: None, new_type: "bigint" }` (old_type requires catalog) |
| Multi-statement file | Two statements separated by `;` | Two IR nodes with correct line offsets |
| DO block | `DO $$ BEGIN ... END $$;` | `Unparseable` |
| Empty input | `""` | Empty vec |
| Comment-only file | `-- just a comment` | Empty vec |

### 2.2 Suppression Parsing (`suppress.rs`)

| Test | Input | Assert |
|------|-------|--------|
| Next-statement single rule | `-- pgm-lint:suppress PGM001\nCREATE INDEX ...` | PGM001 suppressed for line 2 only |
| Next-statement multi rule | `-- pgm-lint:suppress PGM001,PGM003` | Both rules suppressed for next statement |
| File-level | `-- pgm-lint:suppress-file PGM001` at top | PGM001 suppressed for entire file |
| File-level not at top | `CREATE TABLE ...\n-- pgm-lint:suppress-file PGM001` | NOT treated as file-level (must precede any SQL) |
| Inline with other comments | `-- this is a comment\n-- pgm-lint:suppress PGM001` | Only the suppression comment is parsed |
| Whitespace variations | `--pgm-lint:suppress PGM001`, `--  pgm-lint:suppress  PGM001` | Both recognized |
| Unknown rule ID | `-- pgm-lint:suppress PGM999` | Suppression stored but never matched (no error) |

### 2.3 Binary-Coercible Cast Matching (`rules/pgm009.rs` helper)

| Test | From | To | Safe? |
|------|------|----|-------|
| varchar widening | `varchar(50)` | `varchar(100)` | Yes |
| varchar to text | `varchar(50)` | `text` | Yes |
| varchar narrowing | `varchar(100)` | `varchar(50)` | No |
| text to varchar | `text` | `varchar(100)` | No |
| numeric widening (same scale) | `numeric(10,2)` | `numeric(12,2)` | Yes |
| numeric scale change | `numeric(10,2)` | `numeric(10,4)` | No |
| int to bigint | `int` | `bigint` | No |
| timestamp to timestamptz | `timestamp` | `timestamptz` | INFO (conditional) |
| totally different types | `int` | `text` | No |

### 2.4 FK Index Prefix Matching (`rules/pgm003.rs` helper)

| Test | FK Columns | Index Columns | Covered? |
|------|------------|---------------|----------|
| Exact match | `[a, b]` | `[a, b]` | Yes |
| Prefix match | `[a, b]` | `[a, b, c]` | Yes |
| Wrong order | `[a, b]` | `[b, a]` | No |
| Partial overlap | `[a, b]` | `[a]` | No |
| Single column match | `[a]` | `[a, b]` | Yes |
| No indexes at all | `[a]` | (none) | No |

### 2.5 Output Serialization

| Test | Assert |
|------|--------|
| SARIF: single finding | Valid SARIF 2.1.0 JSON, correct `physicalLocation`, correct `ruleId` |
| SARIF: no findings | Valid SARIF with empty `results[]` |
| SonarQube: severity mapping | `Severity::Critical` → `"CRITICAL"`, all levels correct |
| SonarQube: file path | Relative to repo root, forward slashes |
| Text: formatting | Matches spec §7.3 format exactly |

### 2.6 Config Parsing

| Test | Assert |
|------|--------|
| Minimal valid config | Defaults applied correctly |
| Missing `[migrations]` | Error with actionable message |
| Unknown keys | Ignored (forward compat) |
| `strategy = "auto"` Liquibase | Falls through bridge → update-sql → xml |
| `strategy = "xml-only"` | Skips Java entirely |
| `fail_on = "info"` | Parsed to `Severity::Info` |

---

## 3. Component Tests

### 3.1 Catalog Replay

Test the replay engine in isolation: feed it ordered IR nodes, assert catalog state.

| Test | Migration Sequence | Assert Catalog State |
|------|-------------------|---------------------|
| Create then drop | `CREATE TABLE t (...)` → `DROP TABLE t` | `t` not in catalog |
| Create, alter, add index | `CREATE TABLE t (id int)` → `ALTER TABLE t ADD COLUMN name text` → `CREATE INDEX idx ON t (name)` | `t` has 2 columns, 1 index |
| FK tracks referencing cols | `CREATE TABLE parent (id int PK)` → `CREATE TABLE child (pid int REFERENCES parent(id))` | `child` has FK constraint with columns=["pid"] |
| Unparseable marks incomplete | `CREATE TABLE t (id int)` → `Unparseable("ALTER TABLE t ...")` | `t.incomplete = true` |
| Index removal | `CREATE INDEX idx ON t(a)` → `DROP INDEX idx` | `t` has no indexes |
| Column type change tracked | `CREATE TABLE t (x int)` → `ALTER TABLE t ALTER COLUMN x TYPE bigint` | `t.columns["x"].type_name = "bigint"` |
| Drop column removes from indexes | `CREATE TABLE t (a int, b int)` → `CREATE INDEX idx ON t(a, b)` → `ALTER TABLE t DROP COLUMN b` | index removed or marked partial |
| Re-create after drop | `CREATE TABLE t (id int)` → `DROP TABLE t` → `CREATE TABLE t (id bigint)` | `t` in catalog with `bigint` column |
| Composite index column order | `CREATE INDEX idx ON t (a, b, c)` | `idx.columns = ["a", "b", "c"]` in order |

### 3.2 Rules (per-rule component tests)

Each rule tested with: IR nodes + pre-built catalog + changed files list → assert findings.

#### PGM001 — Missing CONCURRENTLY on CREATE INDEX

| Test | Setup | Expect |
|------|-------|--------|
| Index on existing table, no CONCURRENTLY | catalog has `t`, changed files: `002.sql` with `CREATE INDEX idx ON t(a)` | CRITICAL finding |
| Index on existing table, CONCURRENTLY present | same but `CREATE INDEX CONCURRENTLY` | No finding |
| Index on table created in same PR | changed files include `001.sql` with `CREATE TABLE t`, `002.sql` with `CREATE INDEX idx ON t(a)` | No finding |
| Index on table created in same file | single file: `CREATE TABLE t (...); CREATE INDEX idx ON t(a);` | No finding |
| Suppressed | `-- pgm-lint:suppress PGM001` before statement | No finding |

#### PGM003 — FK without covering index

| Test | Setup | Expect |
|------|-------|--------|
| FK added, no index | `ALTER TABLE child ADD CONSTRAINT fk FOREIGN KEY (pid) REFERENCES parent(id)` | MAJOR finding |
| FK added, index created later in same file | FK then `CREATE INDEX idx ON child(pid)` | No finding |
| FK added, wrong index column order | composite FK `(a,b)`, index exists on `(b,a)` | MAJOR finding |
| FK added, prefix-covering index exists | FK `(a,b)`, index on `(a,b,c)` | No finding |
| Inline FK in CREATE TABLE, index in same file | `CREATE TABLE child (pid int REFERENCES parent(id)); CREATE INDEX ...` | No finding |

#### PGM004/005 — Missing PK / UNIQUE NOT NULL substitute

| Test | Setup | Expect |
|------|-------|--------|
| Table with no PK, no unique | `CREATE TABLE t (id int, name text)` | PGM004 MAJOR |
| Table with PK | `CREATE TABLE t (id int PRIMARY KEY)` | No finding |
| PK added later in same file | `CREATE TABLE t (id int NOT NULL); ALTER TABLE t ADD PRIMARY KEY (id);` | No finding |
| UNIQUE NOT NULL, no PK | `CREATE TABLE t (id int NOT NULL UNIQUE, name text)` | PGM005 INFO (not PGM004) |
| Temp table, no PK | `CREATE TEMP TABLE t (id int)` | No finding |

#### PGM006 — CONCURRENTLY inside transaction

| Test | Setup | Expect |
|------|-------|--------|
| CONCURRENTLY + run_in_transaction=true | bridge reports `run_in_transaction: true`, SQL has `CREATE INDEX CONCURRENTLY` | CRITICAL finding |
| CONCURRENTLY + run_in_transaction=false | same but `false` | No finding |
| No CONCURRENTLY (already flagged by PGM001) | `run_in_transaction: true`, no CONCURRENTLY | No PGM006 finding (PGM001 handles it) |

#### PGM007 — Volatile default

| Test | Setup | Expect |
|------|-------|--------|
| `DEFAULT now()` | ADD COLUMN with `DEFAULT now()` | WARNING |
| `DEFAULT gen_random_uuid()` | inline in CREATE TABLE | WARNING |
| `DEFAULT my_function()` | unknown function | INFO |
| `DEFAULT 0` | constant | No finding |
| `DEFAULT 'active'` | string literal | No finding |

#### PGM009 — ALTER COLUMN TYPE

| Test | Setup | Expect |
|------|-------|--------|
| `varchar(50)` → `varchar(100)` | existing table in catalog | No finding (safe cast) |
| `int` → `bigint` | existing table | CRITICAL |
| `timestamp` → `timestamptz` | existing table | INFO |
| Type change on new table (same PR) | table created in changed files | No finding |

#### PGM010 — ADD COLUMN NOT NULL without default

| Test | Setup | Expect |
|------|-------|--------|
| `ADD COLUMN x int NOT NULL` | existing table, no DEFAULT | CRITICAL |
| `ADD COLUMN x int NOT NULL DEFAULT 0` | existing table | No finding |
| `ADD COLUMN x int` | nullable, no default | No finding |

#### PGM011 — DROP COLUMN

| Test | Setup | Expect |
|------|-------|--------|
| `DROP COLUMN x` on existing table | existing table | INFO |
| `DROP COLUMN x` on new table in same PR | table created in changed files | No finding |

#### PGM008 — Down migration severity cap

| Test | Setup | Expect |
|------|-------|--------|
| PGM001 trigger in `.down.sql` | same trigger as PGM001 but in down file | INFO (not CRITICAL) |
| PGM004 trigger in `.down.sql` | table without PK | INFO (not MAJOR) |

---

## 4. Integration Tests

Full pipeline, in-process. Load config + fixture repo → run pipeline → assert findings list.

### 4.1 Fixture Repos

Stored in `tests/fixtures/repos/`. Each is a self-contained directory with config + migration files.

| Fixture Repo | Contents | Expected Findings |
|-------------|----------|-------------------|
| `clean/` | All migrations are correct | 0 findings |
| `all-rules/` | One violation per rule | 11 findings, one per PGM rule |
| `suppressed/` | Same as `all-rules` but every violation has inline suppression | 0 findings |
| `file-suppressed/` | `suppress-file` at top, violations below | 0 findings |
| `multi-statement/` | Single file with many statements, some triggering, some not | Specific expected findings |
| `changed-files-filter/` | Full history is clean, changed files have violations | Findings only on changed files |
| `new-table-in-pr/` | CREATE TABLE + CREATE INDEX (no CONCURRENTLY) in same PR | 0 PGM001 findings |
| `fk-with-later-index/` | FK created then index in same file | 0 PGM003 findings |
| `replay-drop-recreate/` | Table created, dropped, recreated with different schema | Findings reflect final schema |
| `liquibase-xml/` | XML changelog with multiple changesets (fallback parser) | Correct findings with changeset-level line numbers |
| `down-migrations/` | `.down.sql` files with violations | All findings capped at INFO |
| `mixed-tools/` | Some Liquibase XML, some raw SQL | Findings from both sources |

### 4.2 Output Format Assertions

For each fixture repo, validate:

- SARIF output passes JSON Schema validation against SARIF 2.1.0 schema
- SonarQube JSON matches the Generic Issue Import schema
- Text output matches expected snapshot
- Finding count, rule IDs, severities, file paths, line numbers all match expected

---

## 5. E2E Tests

Run the compiled binary as a subprocess. These live in `tests/e2e/` and are `#[test]` functions that invoke `Command::new("target/release/pg-migration-lint")`.

| Test | Command | Assert |
|------|---------|--------|
| Exit 0 on clean repo | `--config clean/config.toml` | exit code 0, empty findings |
| Exit 1 on violations | `--config all-rules/config.toml --fail-on critical` | exit code 1 |
| Exit 0 when below threshold | `--config all-rules/config.toml --fail-on blocker` | exit code 0 (no blockers) |
| Exit 2 on bad config | `--config nonexistent.toml` | exit code 2, stderr contains error |
| `--explain PGM001` | no config needed | exit code 0, stdout contains explanation text |
| `--changed-files` filtering | pass only clean files | exit code 0 |
| `--changed-files-from` | file list in temp file | correct findings |
| `--format text` override | force text output | stdout contains text-formatted findings |
| SARIF file written | `--format sarif` | file exists at configured output path, valid JSON |

---

## 6. Liquibase Bridge Tests

Separate test suite requiring Java. Gated behind a feature flag or env var (`PGM_TEST_JAVA=1`) so the Rust-only tests run without a JRE.

| Test | Assert |
|------|--------|
| Bridge produces valid JSON | Parse output, all fields present |
| Multi-changeset changelog | Correct ordering, correct `changeset_id` per entry |
| `runInTransaction="false"` propagated | `run_in_transaction: false` in JSON |
| Includes/nested changelogs | Resolved correctly, correct `xml_file` per changeset |
| Malformed XML | Bridge exits non-zero, Rust side falls through to next strategy |
| Bridge jar not found | Falls through to update-sql or XML fallback |

---

## 7. Property-Based / Fuzz Testing

Use `proptest` or `cargo-fuzz` on the parser boundary.

| Target | Input Generation | Assert |
|--------|-----------------|--------|
| `parse_sql` | Random strings, semi-valid SQL fragments | Never panics. Returns `Ok` or `Err`, never crashes. |
| `parse_sql` | Valid SQL mutated (random byte flips) | Same: no panics. |
| Suppression parser | Random comment strings | Never panics, returns empty suppressions for garbage input. |
| SARIF serializer | Random `Finding` structs | Always produces valid JSON. |

---

## 8. CI Pipeline

```yaml
jobs:
  test:
    steps:
      - cargo fmt --check
      - cargo clippy -- -D warnings
      - cargo test                          # unit + component + integration
      - cargo test --release                # E2E (needs release binary)
  
  test-java:
    needs: test
    steps:
      - mvn -f bridge/pom.xml package
      - PGM_TEST_JAVA=1 cargo test         # bridge integration tests
  
  fuzz:
    # Nightly or weekly
    steps:
      - cargo fuzz run parse_sql -- -max_total_time=300
```

---

## 9. Test Data Management

- **Fixture files**: checked into `tests/fixtures/`. Small, focused, one concern per file. Named to indicate what they test: `create_index_no_concurrently.sql`, `fk_composite_no_index.sql`.
- **Expected output snapshots**: stored as `.expected.json` / `.expected.sarif` / `.expected.txt` alongside fixtures. Tests compare actual output against snapshots. Use `insta` crate for snapshot testing if preferred.
- **Catalog state assertions**: helper functions that build a `Catalog` from a readable DSL or builder pattern, not raw struct construction:

```rust
let catalog = CatalogBuilder::new()
    .table("orders", |t| {
        t.column("id", "int", false)
         .column("status", "text", true)
         .index("idx_status", &["status"], false)
         .pk(&["id"])
    })
    .build();
```
