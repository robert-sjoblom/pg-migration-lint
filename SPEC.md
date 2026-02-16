# pg-migration-lint — Specification v1

## 1. Overview

A Rust CLI tool that statically analyzes PostgreSQL migration files for common safety and correctness issues. It builds an internal table catalog by replaying the full migration history, then lints only new/changed files. Output is consumed by SonarQube (Generic Issue Import JSON) and GitHub Code Scanning (SARIF).

### Non-goals for v1

- SonarQube native plugin (deferred; external tool + import format)
- Jenkins integration (deferred)
- Multiple independent migration sets / monorepo support (deferred)
- Declarative rule DSL / user-authored rules (deferred)
- Incremental/cached replay (deferred; brute-force on every run)
- Single-file Liquibase changelog support (deferred; when all changesets live in one file, changed-file detection cannot distinguish new vs. existing changesets without git diffing, which is out of scope — the tool's contract is "CI tells us what changed")
- Lightweight XML fallback parser (dropped; if you use Liquibase, a JRE is available — use the bridge jar or `update-sql`)
- Built-in git integration (explicitly rejected; weakens focus)

---

## 2. Input Sources

### 2.1 Raw SQL migrations

- Individual `.up.sql` / `.down.sql` files (go-migrate convention)
- Single-file-with-many-statements (`;`-delimited)
- Standalone `.sql` files

### 2.2 Liquibase

- **Raw SQL changesets**: parsed directly

- **XML changelogs**: two strategies, tried in order:

  1. **Preferred**: `liquibase-bridge.jar` — a minimal Java CLI (~100 LOC) that embeds Liquibase as a library. Takes a changelog path, programmatically resolves all includes and preconditions, and emits a JSON mapping:
     ```json
     [
       {
         "changeset_id": "20240315-1",
         "author": "robert",
         "sql": "CREATE TABLE orders (...);",
         "xml_file": "db/changelog/20240315-create-orders.xml",
         "xml_line": 5,
         "run_in_transaction": true
       }
     ]
     ```
     Rust shells out to `java -jar liquibase-bridge.jar --changelog <path>` and parses the output. This gives exact changeset-to-SQL traceability with precise line mapping back to the XML source. Requires a JRE, which is already present in CI environments that use Liquibase.

  2. **Secondary**: invoke `liquibase update-sql` directly if the bridge jar is unavailable but the Liquibase binary exists. Less structured output (raw SQL without changeset-to-line mapping), parsed heuristically.

- Single XML files containing multiple `<changeSet>` elements are supported across both strategies.

### 2.3 Migration ordering

Configured explicitly per project (not inferred):

```toml
[migrations]
strategy = "liquibase"  # or "filename_lexicographic"
```

- `liquibase`: order derived from changelog include order
- `filename_lexicographic`: sorted by filename (go-migrate convention)

---

## 3. Architecture

### 3.1 Pipeline

```
Input Files
    │
    ▼
┌──────────┐
│  Parser  │  pg_query (libpg_query Rust bindings)
└────┬─────┘  Liquibase XML parser (fallback)
     │
     ▼
┌──────────┐
│    IR    │  Intermediate Representation (§3.2)
└────┬─────┘
     │
     ▼
┌──────────────┐
│ Replay Engine│  Builds Table Catalog (§3.3)
└────┬─────────┘
     │
     ▼
┌──────────────┐
│ Rule Engine  │  Runs rules against changed files only
└────┬─────────┘
     │
     ▼
┌──────────┐
│ Reporter │  SARIF, SonarQube JSON, text, --explain
└──────────┘
```

### 3.2 Intermediate Representation (IR)

The SQL AST from `pg_query` is transformed into a higher-level IR before rules execute. This decouples rule logic from parser internals and simplifies future rule authoring.

IR node types (non-exhaustive):

| IR Node | Source AST |
|---|---|
| `CreateTable { name, columns, constraints, temporary }` | `CreateStmt` |
| `AlterTable { name, actions[] }` | `AlterTableStmt` |
| `CreateIndex { table, columns, unique, concurrent }` | `IndexStmt` |
| `DropIndex { name, concurrent }` | `DropStmt(OBJECT_INDEX)` |
| `DropTable { name }` | `DropStmt(OBJECT_TABLE)` |
| `AddColumn { table, column }` | `AlterTableCmd(AT_AddColumn)` |
| `AddConstraint { table, constraint }` | `AlterTableCmd(AT_AddConstraint)` |

**Constraint normalization**: Postgres supports both inline (`CREATE TABLE foo (baz int PRIMARY KEY)`) and table-level (`CREATE TABLE foo (baz int, PRIMARY KEY (baz))`) syntax for PK, FK, and UNIQUE constraints. These land in different places in the `pg_query` AST (`ColumnDef.constraints` vs `CreateStmt.tableElts`). The IR preserves the distinction (`ColumnDef.is_inline_pk` vs `TableConstraint::PrimaryKey`), but the Catalog must normalize both into identical `TableState`. Rules never deal with the syntactic variant — only catalog state.

**`serial`/`bigserial` expansion**: Postgres's parser expands `serial` into `integer` + `CREATE SEQUENCE` + `DEFAULT nextval(...)`. The IR sees the expanded form. This means PGM007 may fire on `nextval()` as an unknown function call (INFO level). This is technically correct but noisy for a well-known idiom. The v1 approach: add `nextval` to the known volatile function list with a tailored message: `Column '{col}' uses a sequence default (serial/bigserial). This is standard — suppress this finding if intentional.`
| `Unparseable { raw_sql }` | Anything that fails IR conversion |

`Column` carries: `name`, `type_name`, `nullable`, `default_expr`, `is_pk`.

`Constraint` variants: `PrimaryKey`, `ForeignKey { columns, ref_table, ref_columns }`, `Unique`, `Check`.

`Unparseable` nodes are preserved in the stream so the replay engine can mark catalog gaps.

### 3.3 Table Catalog

Built by replaying all migration files in configured order. Represents the schema state at each point in the migration history.

```
Catalog {
    tables: HashMap<String, TableState>
}

TableState {
    name: String,
    columns: Vec<ColumnState>,    // name, type, nullable, default
    indexes: Vec<IndexState>,     // name, columns (ordered), unique
    constraints: Vec<ConstraintState>,  // PK, FK, unique, check
    has_primary_key: bool,
    incomplete: bool,             // true if any unparseable statement touched this table
}
```

- `CREATE TABLE` → insert into catalog
- `DROP TABLE` → remove from catalog entirely
- `ALTER TABLE` → mutate existing entry
- `CREATE INDEX` → add to table's index list
- Unparseable statements → if they reference a known table (best-effort regex on table name), mark that table `incomplete = true`; otherwise skip silently

### 3.4 Changed file detection

The tool does NOT invoke `git` directly. CI passes changed files via:

```
pg-migration-lint --changed-files file1.sql,file2.sql ...
```

Or via a file:

```
pg-migration-lint --changed-files-from changed.txt
```

If `--changed-files` is omitted, all migration files are linted (useful for full-repo scans / first adoption).

Base ref for diff is the caller's responsibility (CI script runs `git diff --name-only origin/main...HEAD`).

---

## 4. Rules

### 4.1 Rule identifiers

Format: `PGMnnn`. Stable across versions. Never reused.

- **PGM0xx**: Migration safety rules (core lint rules with `Rule` trait implementations)
- **PGM1xx**: PostgreSQL "Don't Do This" type-check rules
- **PGM9xx**: Meta-behaviors that modify how other rules operate (not standalone rules)

### 4.2 v1 Rules

#### PGM001 — Missing `CONCURRENTLY` on `CREATE INDEX`

- **Severity**: CRITICAL
- **Triggers**: `CREATE INDEX` on a table that exists in the catalog AND is not created in the same set of changed files.
- **Logic**: If the target table appears in a `CREATE TABLE` within the changed files, the index is on a new table → no finding. Otherwise, `CONCURRENTLY` is required.
- **Message**: `CREATE INDEX on existing table '{table}' should use CONCURRENTLY to avoid holding an exclusive lock.`

#### PGM002 — Missing `CONCURRENTLY` on `DROP INDEX`

- **Severity**: CRITICAL
- **Triggers**: `DROP INDEX` without `CONCURRENTLY`, where the index belongs to an existing table (same logic as PGM001).
- **Message**: `DROP INDEX on existing table should use CONCURRENTLY to avoid holding an exclusive lock.`

#### PGM003 — Foreign key without index on referencing columns

- **Severity**: MAJOR
- **Triggers**: `ADD CONSTRAINT ... FOREIGN KEY (cols) REFERENCES ...` where no index exists on the referencing table with `cols` as a prefix of the index columns.
- **Prefix matching**: FK columns `(a, b)` are covered by index `(a, b)` or `(a, b, c)` but NOT by `(b, a)` or `(a)`. Column order matters.
- **Catalog lookup**: checks indexes on the referencing table after the full file/changeset is processed (not at the point of FK creation). This avoids false positives when the index is created later in the same file/changeset.
- **Message**: `Foreign key on '{table}({cols})' has no covering index. Sequential scans on the referencing table during deletes/updates on the referenced table will cause performance issues.`

#### PGM004 — Table without primary key

- **Severity**: MAJOR
- **Triggers**: `CREATE TABLE` (non-temporary) with no `PRIMARY KEY` constraint, checked after the full file/changeset is processed (to allow `ALTER TABLE ... ADD PRIMARY KEY` later in the same file).
- **Message**: `Table '{table}' has no primary key.`

#### PGM005 — `UNIQUE NOT NULL` used instead of primary key

- **Severity**: INFO
- **Triggers**: Table has no PK but has at least one `UNIQUE` constraint where all constituent columns are `NOT NULL`.
- **Message**: `Table '{table}' uses UNIQUE NOT NULL instead of PRIMARY KEY. Functionally equivalent but PRIMARY KEY is conventional and more explicit.`

#### PGM006 — `CONCURRENTLY` inside transaction

- **Severity**: CRITICAL
- **Triggers**: `CREATE INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY` inside a context that implies transactional execution:
  - Liquibase changeset without `runInTransaction="false"`
  - go-migrate (which runs each file in a transaction by default, unless the file contains `-- +goose NO TRANSACTION` or equivalent)
- **Message**: `CONCURRENTLY cannot run inside a transaction. Set runInTransaction="false" (Liquibase) or disable transactions for this migration.`

#### PGM007 — Volatile default on column

- **Severity**: WARNING for known volatile functions (`now()`, `current_timestamp`, `random()`, `gen_random_uuid()`, `uuid_generate_v4()`, `clock_timestamp()`, `timeofday()`, `txid_current()`, `nextval()`). INFO for any other function call used as a default.
- **Triggers**: `ADD COLUMN ... DEFAULT fn()` or inline in `CREATE TABLE`.
- **Note**: On Postgres 11+, non-volatile defaults on `ADD COLUMN` don't rewrite the table. Volatile defaults always evaluate per-row at write time, which is typically intentional — but worth flagging because developers sometimes use `now()` expecting a fixed value.
- **Message (known volatile)**: `Column '{col}' on '{table}' uses volatile default '{fn}()'. Unlike non-volatile defaults, this forces a full table rewrite under an ACCESS EXCLUSIVE lock — every existing row must be physically updated with a computed value. For large tables, this causes extended downtime. Consider adding the column without a default, then backfilling with batched UPDATEs.`
- **Message (nextval/serial)**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). This is standard usage — suppress if intentional. Note: on ADD COLUMN to an existing table, this is volatile and forces a table rewrite.`
- **Message (unknown function)**: `Column '{col}' on '{table}' uses function '{fn}()' as default. If this function is volatile (the default for user-defined functions), it forces a full table rewrite under an ACCESS EXCLUSIVE lock instead of a cheap catalog-only change. Verify the function's volatility classification.`

#### PGM009 — `ALTER COLUMN TYPE` on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ALTER COLUMN ... TYPE ...` where the table exists in the catalog (not created in the same set of changed files).
- **Note**: Most type changes require a full table rewrite and `ACCESS EXCLUSIVE` lock for the duration. Binary-coercible casts do NOT rewrite. The rule maintains a hardcoded allowlist of safe casts:
  - `varchar(n)` → `varchar(m)` where `m > n` (or unbounded `varchar`/`text`)
  - `varchar(n)` → `text`
  - `numeric(p,s)` → `numeric(p2,s)` where `p2 > p` (same scale)
  - `bit(n)` → `bit(m)` where `m > n`
  - `varbit(n)` → `varbit(m)` where `m > n`
  - `timestamp` → `timestamptz` (safe in PG 15+ when session timezone is UTC; flagged as INFO instead of CRITICAL with a note to verify timezone config)
- Safe casts produce no finding. All other type changes fire as CRITICAL.
- **Message**: `Changing column type on existing table '{table}' ('{col}': {old_type} → {new_type}) rewrites the entire table under an ACCESS EXCLUSIVE lock. For large tables, this causes extended downtime. Consider creating a new column, backfilling, and swapping instead.`

#### PGM010 — `ADD COLUMN NOT NULL` without default on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a `DEFAULT` clause, where the table exists in the catalog.
- **Note**: On PG 11+, `ADD COLUMN ... NOT NULL DEFAULT <value>` is safe (no rewrite for non-volatile defaults). Without a default, the command fails outright if any rows exist. This is almost always a bug.
- **Message**: `Adding NOT NULL column '{col}' to existing table '{table}' without a DEFAULT will fail if the table has any rows. Add a DEFAULT value, or add the column as nullable and backfill.`

#### PGM011 — `DROP COLUMN` on existing table

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... DROP COLUMN` where the table exists in the catalog.
- **Note**: Postgres marks the column as dropped without rewriting the table, so this is cheap at the database level. The risk is application-level: queries referencing the column will break. This is informational to increase visibility.
- **Message**: `Dropping column '{col}' from existing table '{table}'. The DDL is cheap but ensure no application code references this column.`

#### PGM012 — `ADD PRIMARY KEY` on existing table without prior `UNIQUE` constraint

- **Severity**: MAJOR
- **Triggers**: `ALTER TABLE ... ADD PRIMARY KEY (cols)` where the table exists in the catalog (not created in the same changeset) and the target columns do NOT already have a covering unique index or `UNIQUE` constraint.
- **Logic**: Check `catalog_before` for the table. Look for a unique index or `UNIQUE` constraint whose columns match the PK columns exactly (set equality). If neither exists, fire.
- **Why**: Adding a primary key builds a unique index (not concurrently) under `ACCESS EXCLUSIVE` lock. If duplicates exist, the command fails at deploy time. The safe pattern is: build a unique index CONCURRENTLY first, then `ADD PRIMARY KEY USING INDEX`.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - The PK columns already have a covering unique index (exact column match)
  - The PK columns already have a `UNIQUE` constraint (exact column match)
- **Message**: `ADD PRIMARY KEY on existing table '{table}' requires building a unique index under ACCESS EXCLUSIVE lock. Create a UNIQUE index CONCURRENTLY first, then use ADD PRIMARY KEY USING INDEX.`

#### PGM013 — `DROP COLUMN` silently removes unique constraint

- **Severity**: WARNING
- **Status**: Implemented.
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in a `UNIQUE` constraint or unique index on the table in `catalog_before`.
- **Why**: PostgreSQL automatically drops any index or constraint that depends on the column. If the column was part of a unique constraint or unique index, the uniqueness guarantee is silently lost. This can lead to duplicate rows being inserted where they were previously impossible.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `Unique` or any `IndexState` where `is_unique` is true. If so, fire.
- **Does not fire when**:
  - The column is not part of any unique constraint or unique index
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes unique constraint '{constraint}'. Verify that the uniqueness guarantee is no longer needed.`

#### PGM014 — `DROP COLUMN` silently removes primary key

- **Severity**: MAJOR
- **Status**: Implemented.
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in the table's primary key (in `catalog_before`).
- **Why**: Dropping a PK column (with `CASCADE`) silently removes the primary key constraint. The table loses its row identity, which affects replication, ORMs, query planning, and data integrity. PGM004 catches tables *created* without a PK, but cannot tell you which specific `DROP COLUMN` *caused* the loss.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `PrimaryKey`. If so, fire.
- **Does not fire when**:
  - The column is not part of the primary key
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes the primary key. The table will have no row identity. Add a new primary key or reconsider the column drop.`

#### PGM015 — `DROP COLUMN` silently removes foreign key

- **Severity**: WARNING
- **Status**: Implemented.
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in a `FOREIGN KEY` constraint on the table in `catalog_before`.
- **Why**: Dropping a column that is part of a foreign key (with `CASCADE`) silently removes the FK constraint. The referential integrity guarantee is lost — the table can now hold values with no corresponding row in the referenced table.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `ForeignKey`. If so, fire.
- **Does not fire when**:
  - The column is not part of any foreign key constraint
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes foreign key '{constraint}' referencing '{ref_table}'. Verify that the referential integrity guarantee is no longer needed.`

#### PGM016 — `SET NOT NULL` on existing column

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ALTER COLUMN ... SET NOT NULL` on a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Acquires an `ACCESS EXCLUSIVE` lock and performs a full table scan to verify no NULL values exist. On large tables, this blocks all reads and writes for the duration of the scan. The scan is skipped if a valid `CHECK (col IS NOT NULL)` constraint already exists, but the linter cannot verify this.
- **Safe alternative**:
  ```sql
  -- Migration 1: add check constraint (instant, lightweight lock)
  ALTER TABLE orders ADD CONSTRAINT orders_status_nn
    CHECK (status IS NOT NULL) NOT VALID;
  -- Migration 2: validate (SHARE UPDATE EXCLUSIVE — allows reads & writes)
  ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;
  -- Migration 3: scan is skipped due to validated CHECK
  ALTER TABLE orders ALTER COLUMN status SET NOT NULL;
  ALTER TABLE orders DROP CONSTRAINT orders_status_nn;
  ```
- **Does not fire when**:
  - The table is created in the same set of changed files
  - The table does not exist in `catalog_before`
- **Message**: `SET NOT NULL on column '{col}' of existing table '{table}' acquires ACCESS EXCLUSIVE lock and scans the table. Add a CHECK (col IS NOT NULL) NOT VALID constraint first, validate it separately, then SET NOT NULL.`
- **IR impact**: Requires new `AlterTableAction::SetNotNull { column_name: String }` variant. Currently falls into `Other`.

#### PGM017 — `ADD FOREIGN KEY` without `NOT VALID` on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY ...` without `NOT VALID`, on a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Acquires `SHARE ROW EXCLUSIVE` lock on the table (blocking writes) and scans all existing rows to validate references. On large tables this means minutes of blocked writes.
- **Safe alternative**:
  ```sql
  -- Migration 1: add constraint without validation (instant)
  ALTER TABLE orders ADD CONSTRAINT fk_customer
    FOREIGN KEY (customer_id) REFERENCES customers(id) NOT VALID;
  -- Migration 2: validate separately (SHARE UPDATE EXCLUSIVE — allows reads & writes)
  ALTER TABLE orders VALIDATE CONSTRAINT fk_customer;
  ```
- **Does not fire when**:
  - The table is created in the same set of changed files
  - The table does not exist in `catalog_before`
  - The constraint includes `NOT VALID`
- **Interaction with PGM003**: PGM003 (missing FK index) fires independently. The rules are complementary.
- **Message**: `Adding foreign key '{constraint}' on existing table '{table}' validates all rows, blocking writes. Use NOT VALID and validate in a separate migration.`
- **IR impact**: Requires `not_valid: bool` field on `TableConstraint::ForeignKey`.

#### PGM018 — `ADD CHECK` without `NOT VALID` on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)` without `NOT VALID`, on a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Acquires `ACCESS EXCLUSIVE` lock (blocking all reads and writes) and scans all existing rows to validate the expression.
- **Safe alternative**:
  ```sql
  -- Migration 1: add constraint without validation (instant)
  ALTER TABLE orders ADD CONSTRAINT orders_status_valid
    CHECK (status IN ('pending', 'shipped', 'delivered')) NOT VALID;
  -- Migration 2: validate separately (SHARE UPDATE EXCLUSIVE — allows reads & writes)
  ALTER TABLE orders VALIDATE CONSTRAINT orders_status_valid;
  ```
- **Does not fire when**:
  - The table is created in the same set of changed files
  - The table does not exist in `catalog_before`
  - The constraint includes `NOT VALID`
- **Message**: `Adding CHECK constraint '{constraint}' on existing table '{table}' validates all rows under ACCESS EXCLUSIVE lock. Use NOT VALID and validate in a separate migration.`
- **IR impact**: Requires `not_valid: bool` field on `TableConstraint::Check`.

#### PGM019 — `RENAME TABLE`

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... RENAME TO ...` on a table that exists in `catalog_before`.
- **Why**: Renames are instant DDL (metadata-only), but silently break any application queries, views, functions, or triggers that reference the old name.
- **Replacement detection**: Does **not** fire if, within the same migration unit, a `CREATE TABLE` with the old name appears after the rename. This is a common pattern (rename old table away, create replacement with the original name).
- **Does not fire when**:
  - The table does not exist in `catalog_before`
  - A replacement table with the old name is created in the same migration unit
- **Message**: `Renaming table '{old_name}' to '{new_name}'. Ensure all application queries, views, and functions referencing the old name are updated.`
- **IR impact**: pg_query emits `RenameStmt` for this operation. Needs new IR support — either `AlterTableAction::RenameTable { new_name: String }` or a new top-level `IrNode` variant.
- **Catalog impact**: Replay should update the table name in the catalog so subsequent rules see the new name.

#### PGM020 — `RENAME COLUMN`

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... RENAME COLUMN ... TO ...` on a table that exists in `catalog_before`.
- **Why**: Column renames are instant DDL but silently break application queries that reference the old column name.
- **Does not fire when**:
  - The table does not exist in `catalog_before`
- **Message**: `Renaming column '{old_name}' to '{new_name}' on table '{table}'. Ensure all application queries, views, and functions referencing the old column name are updated.`
- **IR impact**: pg_query emits `RenameStmt` for this operation. Needs new `AlterTableAction::RenameColumn { old_name: String, new_name: String }` or a new top-level `IrNode` variant.
- **Catalog impact**: Replay should update the column name in the catalog so subsequent rules see the new name.

#### PGM021 — `ADD UNIQUE` on existing table without `USING INDEX`

- **Severity**: CRITICAL
- **Status**: Implemented.
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE (columns)` where the table exists in `catalog_before` (not created in the same set of changed files) and the target columns do NOT already have a covering unique index or `UNIQUE` constraint.
- **Why**: Adding a UNIQUE constraint inline builds a unique index under an `ACCESS EXCLUSIVE` lock, blocking all reads and writes for the duration. For large tables this can cause extended downtime. Unlike `CHECK` and `FOREIGN KEY` constraints, `NOT VALID` does NOT apply to `UNIQUE` constraints, so there is no `NOT VALID` escape hatch.
- **Logic**: Check `catalog_before` for the table. Look for a unique index or `UNIQUE` constraint whose columns match the constraint columns exactly (set equality). If neither exists, fire.
- **Safe alternative**:
  ```sql
  -- Step 1: build the unique index concurrently (no lock)
  CREATE UNIQUE INDEX CONCURRENTLY idx_orders_email ON orders (email);
  -- Step 2: attach the index as a constraint (instant)
  ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_orders_email;
  ```
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - The constraint columns already have a covering unique index (exact column match)
  - The constraint columns already have a `UNIQUE` constraint (exact column match)
- **Message**: `ADD UNIQUE on existing table '{table}' without a pre-existing unique index on column(s) [{columns}]. Create a unique index CONCURRENTLY first, then use ADD CONSTRAINT ... UNIQUE USING INDEX.`

#### PGM022 — `DROP TABLE` on existing table

- **Severity**: MINOR
- **Status**: Implemented.
- **Triggers**: `DROP TABLE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Dropping a table is intentional but destructive and irreversible in production. The DDL itself is instant — PostgreSQL does not scan the table or hold an extended lock — so this is not a downtime risk. However, all data in the table is permanently lost, and any queries, views, foreign keys, or application code referencing the table will break.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `DROP TABLE '{table}' removes an existing table. This is irreversible and all data will be lost.`

#### PGM023 — Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`

- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause.
- **Why**: Without `IF NOT EXISTS`, the statement fails if the object already exists. In migration pipelines that may be re-run (e.g., idempotent migrations, manual re-execution after partial failure), this causes hard failures. Adding `IF NOT EXISTS` makes the statement idempotent.
- **Does not fire when**:
  - The statement already includes `IF NOT EXISTS`
- **Message (CREATE TABLE)**: `CREATE TABLE '{table}' without IF NOT EXISTS will fail if the table already exists.`
- **Message (CREATE INDEX)**: `CREATE INDEX '{index}' without IF NOT EXISTS will fail if the index already exists.`
- **IR impact**: Requires `if_not_exists: bool` field on `CreateTable` and `CreateIndex`.

#### PGM024 — Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`

- **Severity**: MINOR
- **Status**: Not yet implemented.
- **Triggers**: `DROP TABLE` or `DROP INDEX` without the `IF EXISTS` clause.
- **Why**: Without `IF EXISTS`, the statement fails if the object does not exist. In migration pipelines that may be re-run, this causes hard failures. Adding `IF EXISTS` makes the statement idempotent.
- **Does not fire when**:
  - The statement already includes `IF EXISTS`
- **Message (DROP TABLE)**: `DROP TABLE '{table}' without IF EXISTS will fail if the table does not exist.`
- **Message (DROP INDEX)**: `DROP INDEX '{index}' without IF EXISTS will fail if the index does not exist.`
- **IR impact**: Requires `if_exists: bool` field on `DropTable` and `DropIndex`.

#### PGM901 — Down migration severity cap

- **All down-migration findings are capped at INFO severity**, regardless of what the rule would normally produce.
- The same rules (PGM001–PGM024) apply to `.down.sql` / rollback SQL, but findings are informational only.
- PGM901 is a meta-behavior, not a standalone lint rule. It has no `Rule` trait implementation and cannot be suppressed or disabled via inline comments. The 9xx range is reserved for meta-behaviors that modify how other rules operate.

### 4.3 PostgreSQL "Don't Do This" Rules (PGM1xx)

Rules derived from the [PostgreSQL "Don't Do This" wiki](https://wiki.postgresql.org/wiki/Don%27t_Do_This). These detect column type anti-patterns in `CREATE TABLE`, `ALTER TABLE ... ADD COLUMN`, and `ALTER TABLE ... ALTER COLUMN TYPE` statements.

All type rules share a common detection pattern: inspect `TypeName` on `ColumnDef` in `CreateTable` columns, `AddColumn` actions, and `AlterColumnType` actions.

**Type name canonicalization** (verified against `pg_query` output):

| SQL Syntax | IR `TypeName.name` | Notes |
|------------|-------------------|-------|
| `timestamp` / `timestamp without time zone` | `"timestamp"` | pg_query normalizes both |
| `timestamptz` / `timestamp with time zone` | `"timestamptz"` | |
| `char(n)` / `character(n)` | `"bpchar"` | NOT `"char"` — pg_query canonical form |
| `char` / `character` (no length) | `"bpchar"` | Implicit modifier `[1]` |
| `money` | `"money"` | |
| `serial` | `"int4"` + `nextval()` default | Parser maps and synthesizes |
| `bigserial` | `"int8"` + `nextval()` default | Parser maps and synthesizes |
| `smallserial` | `"int2"` + `nextval()` default | Parser maps and synthesizes |
| `float` / `double precision` | `"float8"` | |
| `real` | `"float4"` | |
| `varchar(n)` / `character varying(n)` | `"varchar"` | Modifiers: `[n]` |

#### PGM101 — Don't use `timestamp` (without time zone)

- **Severity**: WARNING
- **Triggers**: Column type with `TypeName.name == "timestamp"` in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`.
- **Why**: `timestamp without time zone` stores a date-time with no time zone context. The stored value is ambiguous — it could be UTC, local time, or anything else. `timestamptz` stores an absolute point in time (internally UTC) and converts on input/output based on session `timezone`, making it unambiguous.
- **Message**: `Column '{col}' on '{table}' uses 'timestamp without time zone'. Use 'timestamptz' (timestamp with time zone) instead to store unambiguous points in time.`

#### PGM102 — Don't use `timestamp(0)` or `timestamptz(0)`

- **Severity**: WARNING
- **Triggers**: Column type with `TypeName.name` in `("timestamp", "timestamptz")` and `TypeName.modifiers == [0]`.
- **Why**: Setting fractional seconds precision to 0 causes PostgreSQL to *round* (not truncate). An input of `23:59:59.9` becomes `00:00:00` of the *next day*, silently changing the date.
- **Message**: `Column '{col}' on '{table}' uses '{type}(0)'. Precision 0 causes rounding, not truncation — a value of '23:59:59.9' rounds to the next day. Use full precision and format on output instead.`

#### PGM103 — Don't use `char(n)` or `character(n)`

- **Severity**: WARNING
- **Triggers**: Column type with `TypeName.name == "bpchar"` (pg_query canonical form for SQL `char`/`character`).
- **Why**: `char(n)` pads values with spaces to exactly `n` characters. This wastes storage, causes surprising comparison behavior, and is never faster than `text` or `varchar` — PostgreSQL stores them identically on disk (as varlena), with added overhead of pad/unpad operations.
- **Message**: `Column '{col}' on '{table}' uses 'char({n})'. The char(n) type pads with spaces, wastes storage, and is no faster than text or varchar in PostgreSQL. Use text or varchar instead.`

#### PGM104 — Don't use the `money` type

- **Severity**: WARNING
- **Triggers**: Column type with `TypeName.name == "money"`.
- **Why**: The `money` type has fixed fractional precision determined by `lc_monetary` locale. Changing the locale silently reinterprets stored values. It stores no currency code, making multi-currency support impossible. Input/output depends on locale, making dumps/restores across systems dangerous. Use `numeric(p,s)` instead.
- **Message**: `Column '{col}' on '{table}' uses the 'money' type. The money type depends on the lc_monetary locale setting, making it unreliable across environments. Use numeric(p,s) instead.`

#### PGM105 — Don't use `serial` / `bigserial`

- **Severity**: INFO
- **Triggers**: Column with `DefaultExpr::FunctionCall { name: "nextval", .. }` on an `int4`, `int8`, or `int2` typed column. (pg_query expands `serial`→`int4 + nextval()`, `bigserial`→`int8 + nextval()`.)
- **Why**: The `serial` pseudo-types create an implicit sequence with several problems: the sequence ownership is weaker than identity columns, `INSERT` with explicit values doesn't advance the sequence (causing future conflicts), and grants/ownership are separate. Identity columns (`GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY`, SQL standard, PG 10+) handle all these edge cases correctly.
- **Interaction with PGM007**: Both PGM105 and PGM007 fire on `nextval()` defaults. This is intentional — PGM007 warns about the volatile default aspect, PGM105 recommends the identity column alternative.
- **Message**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). Prefer GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY for new tables (PostgreSQL 10+). Identity columns have better ownership semantics and are the SQL standard approach.`

#### PGM108 — Don't use `json` (prefer `jsonb`)

- **Severity**: WARNING
- **Triggers**: Column type with `TypeName.name == "json"` in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`.
- **Why**: The `json` type stores an exact copy of the input text and must re-parse it on every operation. `jsonb` stores a decomposed binary format that is significantly faster for queries, supports indexing (GIN), and supports containment/existence operators (`@>`, `?`, `?|`, `?&`). The only advantages of `json` are preserving exact key order and duplicate keys — both rarely needed.
- **Message**: `Column '{col}' on '{table}' uses 'json'. Use 'jsonb' instead — it's faster, smaller, indexable, and supports containment operators. Only use 'json' if you need to preserve exact text representation or key order.`

#### PGM106 — Don't use `integer` as primary key type

- **Severity**: MAJOR
- **Status**: Not yet implemented.
- **Triggers**: A primary key column with `TypeName.name` in `("int4", "int2")` — i.e., `integer`, `smallint`, or their aliases. Detected in `CREATE TABLE` (inline PK or table-level `PRIMARY KEY` constraint) and `ALTER TABLE ... ADD PRIMARY KEY`.
- **Why**: `integer` (max ~2.1 billion) is routinely exhausted in high-write tables. When it wraps, inserts fail with a unique constraint violation. Migrating from `integer` to `bigint` requires a full table rewrite under `ACCESS EXCLUSIVE` lock — one of the most dangerous DDL operations on large tables. Starting with `bigint` costs 4 extra bytes per row but avoids a future emergency migration.
- **Does not fire when**:
  - The PK column type is `int8` / `bigint`
  - The column is not part of a primary key
- **Message**: `Primary key column '{col}' on '{table}' uses '{type}'. Consider using bigint to avoid exhausting the integer range on high-write tables.`

#### Deferred "Don't Do This" Rules

The following rules are specified but deferred until per-rule enable/disable configuration is implemented. Rule IDs are assigned only when a rule is promoted to implementation.

- Don't use `varchar(n)` for arbitrary length limits (INFO). Very common pattern; needs ability to disable globally.
- Don't use `float`/`real`/`double precision` for exact values (INFO). Legitimate for many use cases; needs ability to disable.
- Don't use `INHERITS` for partitioning (MINOR). Requires IR extension to detect `CREATE TABLE ... INHERITS`.

See `docs/dont-do-this-rules.md` for full specifications of deferred rules.

### 4.4 `--explain PGMnnn`

Prints a detailed explanation of the rule: what it detects, why it's dangerous, concrete examples of the failure mode, and how to fix it. Exits 0. No file scanning.

---

## 5. Suppression

### 5.1 Inline comment suppression

**Next-statement scope:**

```sql
-- pgm-lint:suppress PGM001
CREATE INDEX idx_foo ON bar (col);
```

**File-level scope:**

```sql
-- pgm-lint:suppress-file PGM001
```

File-level suppression must appear before any SQL statements in the file.

Multiple rules in one comment: `-- pgm-lint:suppress PGM001,PGM003`

### 5.2 SonarQube suppression

SonarQube's built-in "Won't Fix" / "False Positive" workflow applies to imported findings. No special handling needed from the tool.

---

## 6. Configuration

File: project root, name TBD (e.g., `pg-migration-lint.toml`).

```toml
[migrations]
# Ordered list of migration source directories/files
paths = ["db/migrations", "db/changelog.xml"]

# Ordering strategy: "liquibase" | "filename_lexicographic"
strategy = "liquibase"

# File patterns to include
include = ["*.sql", "*.xml"]

# File patterns to exclude
exclude = ["**/test/**"]

# Default schema for unqualified table names (default: "public").
# Unqualified names are normalized to "<default_schema>.<name>" for catalog lookups.
default_schema = "public"

[liquibase]
# Path to liquibase-bridge.jar (preferred; enables exact changeset-to-SQL mapping)
bridge_jar_path = "tools/liquibase-bridge.jar"

# Path to liquibase binary (secondary; enables update-sql pre-processing)
binary_path = "/usr/local/bin/liquibase"

# Liquibase properties file (for update-sql connection info, secondary strategy only)
properties_file = "liquibase.properties"

# Strategy order: "bridge" → "update-sql"
strategy = "auto"

[rules]
# Severity overrides (future, not v1 — included for schema stability)
# [rules.PGM001]
# severity = "MAJOR"

[output]
# Formats to produce: "sarif", "sonarqube", "text"
formats = ["sarif", "sonarqube"]

# Output directory
dir = "build/reports/migration-lint"

[cli]
# Exit code threshold: "blocker", "critical", "major", "minor", "info", "none"
# Tool returns non-zero if any finding meets or exceeds this severity
fail_on = "critical"
```

---

## 7. Output Formats

### 7.1 SonarQube Generic Issue Import

```json
{
  "issues": [
    {
      "engineId": "pg-migration-lint",
      "ruleId": "PGM001",
      "severity": "CRITICAL",
      "type": "BUG",
      "primaryLocation": {
        "message": "CREATE INDEX on existing table 'orders' should use CONCURRENTLY.",
        "filePath": "db/migrations/V042__add_order_index.sql",
        "textRange": {
          "startLine": 3,
          "endLine": 3
        }
      }
    }
  ]
}
```

### 7.2 SARIF

Standard SARIF 2.1.0 schema. Upload to GitHub via `github/codeql-action/upload-sarif@v3`. This produces inline PR annotations with no API integration needed.

### 7.3 Text

Human-readable for local development:

```
CRITICAL PGM001 db/migrations/V042__add_order_index.sql:3
  CREATE INDEX on existing table 'orders' should use CONCURRENTLY.

MAJOR PGM003 db/migrations/V042__add_order_index.sql:7
  Foreign key on 'order_items(order_id)' has no covering index.
```

---

## 8. CLI Interface

```
pg-migration-lint [OPTIONS]

OPTIONS:
  --config <path>              Config file (default: ./pg-migration-lint.toml)
  --changed-files <list>       Comma-separated list of changed files
  --changed-files-from <path>  File containing changed file paths (one per line)
  --format <fmt>               Override output format (sarif|sonarqube|text)
  --fail-on <severity>         Override exit code threshold
  --explain <rule>             Print rule explanation and exit

EXIT CODES:
  0  No findings at or above threshold
  1  Findings at or above threshold
  2  Tool error (config, parse failure, etc.)
```

---

## 9. Line Number Mapping

- **Raw SQL files**: exact line numbers from `pg_query` parse positions.
- **Liquibase XML (bridge jar)**: exact line numbers. The bridge emits `xml_line` per changeset, and `pg_query` gives statement offsets within the SQL. Combined, this maps findings to precise XML source locations.
- **Liquibase XML (update-sql)**: changeset-level granularity only. Heuristic mapping from generated SQL comments back to changeset IDs.

---

## 10. Distribution

- **Primary**: statically linked Rust binary for `x86_64-unknown-linux-gnu` (musl for static linking).
- **Secondary**: OCI container image (includes JRE + bridge jar for full Liquibase support out of the box).
- **Bridge jar**: `liquibase-bridge.jar` published as a separate release artifact. Teams using Liquibase XML download both the binary and the jar.
- Build via GitHub Actions. Release binaries and jar attached to GitHub releases.

---

## 11. Project Structure (Proposed)

```
pg-migration-lint/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point (clap)
│   ├── config.rs            # TOML config parsing
│   ├── input/
│   │   ├── mod.rs
│   │   ├── sql.rs           # Raw SQL file loading
│   │   ├── liquibase_bridge.rs  # Shell out to bridge jar, parse JSON
│   │   └── liquibase_updatesql.rs # update-sql invocation
│   ├── parser/
│   │   ├── mod.rs
│   │   ├── pg_query.rs      # pg_query bindings → IR
│   │   └── ir.rs            # IR type definitions
│   ├── catalog/
│   │   ├── mod.rs
│   │   ├── replay.rs        # Migration replay engine
│   │   └── types.rs         # TableState, IndexState, etc.
│   ├── rules/
│   │   ├── mod.rs           # Rule trait, registry
│   │   ├── pgm001.rs        # One file per rule
│   │   ├── pgm002.rs
│   │   ├── ...
│   │   └── explain.rs       # --explain text per rule
│   ├── suppress.rs          # Suppression comment parsing
│   └── output/
│       ├── mod.rs
│       ├── sarif.rs
│       ├── sonarqube.rs
│       └── text.rs
├── bridge/                   # Java subproject
│   ├── pom.xml              # Maven build, shaded jar with Liquibase dependency
│   └── src/main/java/
│       └── LiquibaseBridge.java  # ~100 LOC: changelog → JSON mapping
├── docs/
│   └── pg_query_spike.md    # Phase 0 spike: canonical type names, serial expansion,
│                            # inline vs table-level constraint AST mapping
└── tests/
    ├── fixtures/             # Sample migration files
    └── integration/          # End-to-end tests
```

---

## 12. Future Work (Explicitly Deferred)

- Make meta-behavior rules (PGM9xx) disablable via `rules.disabled` config, so e.g. ignoring PGM901 skips the down-migration severity cap
- Per-rule enable/disable in config (needed for deferred "Don't Do This" rules)
- Severity overrides in config
- Config parsing unit tests (TOML validation, error paths, default handling)
- Native SonarQube plugin (Java, Plugin API)
- Jenkins PR comment integration
- Multiple independent migration sets (monorepo)
- Declarative rule DSL for user-authored rules
- Incremental replay with caching
- Fuzz testing / property-based testing of parser and catalog replay

---

## 13. Revision History

| Version | Date       | Changes |
|---------|------------|---------|
| 1.0     | 2026-02-09 | Initial specification. 11 rules (PGM001–PGM011). Rust CLI with `pg_query` parser, IR layer, replay-based table catalog. Liquibase bridge jar for exact changeset-to-SQL traceability. SARIF + SonarQube Generic Issue Import output. GitHub Actions integration via `upload-sarif`. |
| 1.1     | 2026-02-10 | Added PGM012 (ADD PRIMARY KEY without UNIQUE). Added "Don't Do This" rules PGM101–PGM105 (timestamp, timestamp(0), char(n), money, serial). Deferred PGM106 (varchar), PGM107 (float), PGM111 (INHERITS) until per-rule config. Documented pg_query type name canonicalization. Added `is_serial` to IR `ColumnDef`. Explicitly deferred single-file Liquibase changelog support and rejected built-in git integration as non-goals. |
| 1.2     | 2026-02-11 | Specified PGM013 (DROP COLUMN removes unique constraint, WARNING), PGM014 (DROP COLUMN removes primary key, MAJOR), PGM015 (DROP COLUMN removes foreign key, WARNING). Noted prerequisite catalog fix: `remove_column` must also clean up constraints, not just indexes. |
| 1.3     | 2026-02-11 | Implemented PGM013, PGM014, PGM015. Fixed `remove_column` to clean up constraints. Added schema-aware catalog with configurable `default_schema` (default: `"public"`). Unqualified table names are normalized to `<schema>.<name>` for catalog lookups, so `orders` and `public.orders` resolve to the same table. Total: 19 rules. |
| 1.4     | 2026-02-12 | Documented recursive `<includeAll>` as non-goal for XML fallback parser. Improved warning message to direct users toward bridge JAR or `liquibase update-sql` for nested directory layouts. |
| 1.5     | 2026-02-12 | Added PGM016 (SET NOT NULL on existing column, CRITICAL), PGM017 (ADD FOREIGN KEY without NOT VALID, CRITICAL), PGM018 (ADD CHECK without NOT VALID, CRITICAL), PGM019 (RENAME TABLE with replacement detection, INFO), PGM020 (RENAME COLUMN, INFO), PGM108 (Don't use json, WARNING). Target PostgreSQL 14+. IR impacts: new `SetNotNull` action, `not_valid` field on FK/CHECK constraints, rename support via `RenameStmt` mapping. |
| 1.6     | 2026-02-14 | Added PGM106 (integer primary key, MAJOR). Removed rule IDs from deferred rules — IDs are now assigned only when a rule is promoted to implementation. |
| 1.7     | 2026-02-14 | Spec sync with implementation. Added PGM021 (ADD UNIQUE without USING INDEX, CRITICAL), PGM022 (DROP TABLE on existing table, MINOR). Normalized severity vocabulary: WARNING → MINOR across all rule definitions to match 5-level scheme (INFO, MINOR, MAJOR, CRITICAL, BLOCKER). Documented `rules.disabled` config for globally disabling rules. Updated PGM008 scope to PGM001–PGM022. Added severity level reference in §4.1. |
| 1.8     | 2026-02-15 | Added PGM023 (missing IF NOT EXISTS on CREATE TABLE/CREATE INDEX, MINOR) and PGM024 (missing IF EXISTS on DROP TABLE/DROP INDEX, MINOR). IR changes: `if_not_exists` on `CreateTable`/`CreateIndex`, `if_exists` on `DropTable`/`DropIndex`. Updated PGM008 scope to PGM001–PGM024. |
| 1.9     | 2026-02-15 | Dropped lightweight XML fallback parser. Liquibase now requires a JRE — two-tier strategy: bridge jar → update-sql. Removed `liquibase_xml.rs` from project structure, removed `"xml-only"` config option. |
| 1.10    | 2026-02-16 | Fleshed out full rule definitions for PGM021 (ADD UNIQUE without USING INDEX), PGM022 (DROP TABLE), PGM023 (missing IF NOT EXISTS), PGM024 (missing IF EXISTS), PGM106 (integer primary key). Removed stale IDs from deferred rules in §4.3. Synced `docs/dont-do-this-rules.md` with current spec state. |
| 1.11    | 2026-02-16 | Renamed PGM008 → PGM901. Established 9xx range for meta-behaviors that modify how other rules operate. |
