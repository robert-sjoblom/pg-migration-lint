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
- `_down.sql` suffix convention (e.g., `V001__create_users_down.sql`)
- Single-file-with-many-statements (`;`-delimited)
- Standalone `.sql` files

Down migrations are detected by filename suffix: the stem (filename minus `.sql` extension) must end with `.down` or `_down`. Files that merely contain "down" elsewhere in the name (e.g., `downtown_orders.sql`) are not treated as down migrations.

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

- **Limitation — rollback blocks**: Liquibase `<rollback>` elements inside changesets are not detected as down migrations. Both Liquibase loaders emit `is_down: false` for all changesets. SQL extracted from rollback blocks will be linted at full severity rather than being capped to INFO by PGM901.

- **Limitation — `update-sql` rejects duplicate changeset includes**: If a master changelog `<include>`s the same file more than once (duplicate `<include>` directives), `liquibase update-sql` fails validation with "changesets had duplicate identifiers". The bridge jar handles this correctly. In production Liquibase, duplicates are silently skipped via the DATABASECHANGELOG tracking table, so these changelogs are valid and will apply without error. This is a known fidelity gap: `update-sql` runs without a database and applies stricter validation than the real Liquibase runtime. When the bridge jar is available, prefer it for this reason.

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
└────┬─────┘
     │
     ▼
┌──────────┐
│    IR    │  Intermediate Representation (§3.2)
└────┬─────┘
     │
     ▼
┌───────────┐
│ Normalize │  Resolves names against default_schema (§3.2)
└────┬──────┘
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
┌──────────────┐
│   Grouping   │  Merges repeated findings per rule and file (§7.4)
└────┬─────────┘
     │
     ▼
┌──────────┐
│ Reporter │  SARIF, SonarQube JSON, text, --explain
└──────────┘
```

Suppression comments (§5) and per-unit dedup run on each unit's findings before grouping.

### 3.2 Intermediate Representation (IR)

The SQL AST from `pg_query` is transformed into a higher-level IR before rules execute. This decouples rule logic from parser internals and simplifies future rule authoring.

IR node types (non-exhaustive):

| IR Node | Source AST |
|---|---|
| `CreateTable { name, columns, constraints, persistence, partition_by, partition_of }` | `CreateStmt` |
| `AlterTable { name, actions[] }` | `AlterTableStmt` (objtype = ObjectTable) |
| `CreateIndex { index_name, table_name, columns, unique, concurrent, if_not_exists, where_clause, only, access_method }` | `IndexStmt` |
| `DropIndex { index_name, concurrent, if_exists }` | `DropStmt(OBJECT_INDEX)` |
| `DropTable { name, if_exists, cascade }` | `DropStmt(OBJECT_TABLE)` |
| `DropSchema { schema_name, cascade, if_exists }` | `DropStmt(OBJECT_SCHEMA)` |
| `AlterIndexAttachPartition { parent_index_name, child_index_name }` | `AlterTableStmt` (objtype = ObjectIndex, AT_AttachPartition) |
| `RenameTable { name, new_name }` | `RenameStmt` (ObjectTable) |
| `RenameColumn { table, old_name, new_name }` | `RenameStmt` (ObjectColumn) |
| `RenameIndex { index, new_name }` | `RenameStmt` (ObjectIndex) |
| `RenameConstraint { table, old_name, new_name }` | `RenameStmt` (ObjectTabconstraint) |
| `Cluster { table, index }` | `ClusterStmt` |
| `VacuumFull { table }` | `VacuumStmt` (FULL option) |
| `Reindex { kind, target, concurrent }` | `ReindexStmt` |
| `InsertInto { table_name }` | `InsertStmt` |
| `UpdateTable { table_name }` | `UpdateStmt` |
| `DeleteFrom { table_name }` | `DeleteStmt` |
| `TruncateTable { table_name, cascade }` | `TruncateStmt` |
| `TransactionControl(Begin \| Commit \| Rollback)` | `TransactionStmt` (BEGIN/START, COMMIT/END, ROLLBACK/ABORT) |

`AlterTableAction` variants: `AddColumn`, `DropColumn`, `AddConstraint { constraint, origin }`, `AlterColumnType`, `SetNotNull`, `DropNotNull`, `SetDefault`, `DropDefault`, `DropConstraint`, `ValidateConstraint`, `AttachPartition`, `DetachPartition`, `DisableTrigger`, `Other`.

`AddConstraint` carries a `ConstraintOrigin` naming the subcommand the constraint was written in: `AddColumn` for a column constraint written inline in an `ADD COLUMN` (`ADD COLUMN b int UNIQUE`), `AddConstraint` for an `ADD CONSTRAINT` subcommand (`ADD UNIQUE (b)`). A constraint with origin `AddColumn` directly follows the `AddColumn` it was written in, with that column's other inline constraints, in written order. Rules treat both origins alike. Replay uses the origin because Postgres names the constraints of one `ALTER TABLE` in pass order (`USING INDEX` constraints, then other index-backed ones, then CHECK and FOREIGN KEY), and within a pass names the constraints of each `ADD COLUMN` in turn (its CHECKs before its FOREIGN KEYs) before any `ADD CONSTRAINT`'s, which decides which of two colliding generated names gets the numeric suffix (`verify/tests/naming/collision_order.sql`).

**Constraint normalization**: Postgres supports both inline (`CREATE TABLE foo (baz int PRIMARY KEY)`) and table-level (`CREATE TABLE foo (baz int, PRIMARY KEY (baz))`) syntax for PK, FK, UNIQUE and CHECK constraints. These land in different places in the `pg_query` AST (`ColumnDef.constraints` vs `CreateStmt.tableElts`). The parser promotes every inline constraint to a `TableConstraint`: into `CreateTable.constraints`, in written order among the table-level ones, or, for `ALTER TABLE ... ADD COLUMN`, into `AddConstraint` actions with origin `AddColumn` right after the column's `AddColumn`. An inline PK is therefore in both places: `ColumnDef.is_inline_pk` flags it, and it is also a `TableConstraint::PrimaryKey`. Replay and the rules read only the `TableConstraint`, so both syntaxes produce identical `TableState`, and rules never deal with the syntactic variant.

**`serial`/`bigserial` expansion**: Postgres's parser expands `serial` into `integer` + `CREATE SEQUENCE` + `DEFAULT nextval(...)`. The IR sees the expanded form. This means PGM006 may fire on `nextval()` as an unknown function call (INFO level). This is technically correct but noisy for a well-known idiom. The v1 approach: add `nextval` to the known volatile function list with a tailored message: `Column '{col}' uses a sequence default (serial/bigserial). This is standard — suppress this finding if intentional.`

`ColumnDef` carries: `name`, `type_name`, `nullable`, `default_expr`, `is_inline_pk`, `is_serial`.

`TableConstraint` variants: `PrimaryKey { name, columns, include_columns, using_index }`, `ForeignKey { name, columns, ref_table, ref_columns, not_valid }`, `Unique { name, columns, include_columns, using_index }`, `Check { name, expression, referenced_columns, not_valid }`, `Exclude { name, elements, include_columns }`.

`TransactionControl` marks where a transaction block starts or ends: `Begin` for `BEGIN` and `START TRANSACTION`, `Commit` for `COMMIT` and `END`, `Rollback` for `ROLLBACK` and `ABORT`. `COMMIT AND CHAIN` and `ROLLBACK AND CHAIN` (also `END AND CHAIN`, `ABORT AND CHAIN`) produce two nodes with the statement's span, the end followed by `Begin`, because they start the next transaction at once. Savepoints (`SAVEPOINT`, `RELEASE`, `ROLLBACK TO`) and two-phase commit (`PREPARE TRANSACTION`, `COMMIT PREPARED`, `ROLLBACK PREPARED`) stay `Ignored`. Replay treats the node as a no-op, so a `ROLLBACK` does not undo the statements before it.

`Unparseable` nodes are preserved in the stream so the replay engine can mark catalog gaps.

Relation names in the IR start as `QualifiedName { schema: Option<String>, name }`, as the parser read them. Normalization resolves each against `default_schema` into a `ResolvedName`, which holds the catalog key (`RelationName { schema, name }`) and remembers whether the SQL wrote the schema, so messages show the name as the user wrote it. Replay and rules only see resolved names.

### 3.3 Table Catalog

Built by replaying all migration files in configured order. Represents the schema state at each point in the migration history.

```
Catalog {
    tables: HashMap<RelationName, TableState>,     // RelationName { schema, name }; schema always set
    index_to_table: HashMap<RelationName, RelationName>, // reverse lookup: index → table key; an index is in its table's schema
}

TableState {
    name: RelationName,
    columns: Vec<ColumnState>,       // name, type, nullable, default
    indexes: Vec<IndexState>,        // name, entries (ordered), unique, where_clause, only
    constraints: Vec<ConstraintState>,  // PK, FK, unique, check
    has_primary_key: bool,
    incomplete: bool,                // true if any unparseable statement touched this table
    is_partitioned: bool,            // true if PARTITION BY was used
    partition_by: Option<PartitionByInfo>,  // strategy + columns
    parent_table: Option<RelationName>,  // catalog key of parent (if PARTITION OF)
}

IndexState {
    name: String,
    entries: Vec<IndexColumn>,       // Column(name) or Expression { text, referenced_columns }
    unique: bool,
    where_clause: Option<String>,    // partial index WHERE clause
    only: bool,                      // CREATE INDEX ON ONLY (parent stub, not recursive)
    access_method: String,           // "btree" (default), "gin", "gist", "hash", "brin"
}
```

- `CREATE TABLE` → insert into catalog; if `PARTITION OF`, record parent relationship
- `DROP TABLE` → remove from catalog entirely; dropping a partitioned parent recursively removes its partitions, with or without CASCADE, as PostgreSQL does
- `DROP SCHEMA ... CASCADE` → remove every table in the schema; without CASCADE → no change, as PostgreSQL rejects dropping a non-empty schema
- `ALTER TABLE` → mutate existing entry; `ATTACH PARTITION` / `DETACH PARTITION` update parent-child tracking
- `CREATE INDEX` → add to table's index list (preserving `only` flag); `CREATE INDEX IF NOT EXISTS` whose name a relation in the table's schema already has (an index on any table, a table, or an index-backed constraint) → no change, as PostgreSQL skips it
- `DROP INDEX` → remove the index from its table; `DROP INDEX` of the index behind a PRIMARY KEY, UNIQUE or EXCLUDE constraint → no change, as PostgreSQL rejects it (2BP01)
- `ALTER INDEX ATTACH PARTITION` → flip parent index's `only` from `true` to `false`
- Unparseable statements → if they reference a known table (best-effort regex on table name), mark that table `incomplete = true`; otherwise skip silently
- Every constraint and index has a name. One created without a name gets the name PostgreSQL generates from the bare table name: `{table}_pkey`, `{table}_{cols}_key`, `{table}_{cols}_fkey`, `{table}_{col}_check` (exactly one referenced column) or `{table}_check`, `{table}_{elements}_excl`, `{table}_{elements}_idx`, cut to 63 bytes as PostgreSQL does (measured in `verify/tests/naming/`). A generated name already taken in its schema gets a numbered label, as in PostgreSQL (`{table}_{cols}_key1`, `_key2`, ...), with the parts cut again to fit 63 bytes. CHECK and FOREIGN KEY names avoid every constraint name in the schema; PRIMARY KEY, UNIQUE and EXCLUDE names also avoid relation names (tables, and the indexes the catalog records); an unnamed `CREATE INDEX` avoids relation names only. Within one statement, names are chosen in the order PostgreSQL creates the objects, and each avoids the names created before it: `CREATE TABLE` names CHECKs, then the PRIMARY KEY, then UNIQUE and EXCLUDE, then FOREIGN KEYs; `ALTER TABLE` runs its drops first, then names `USING INDEX` constraints, then the other index-backed ones, then CHECKs and FOREIGN KEYs. The catalog still adds a statement's constraints in textual order. A PRIMARY KEY or UNIQUE constraint and its backing index share one name: `ADD PRIMARY KEY USING INDEX idx` names the constraint `idx`, and `ADD CONSTRAINT c PRIMARY KEY USING INDEX idx` renames the index to `c`. Renaming a table or column keeps every name. Renaming a PRIMARY KEY, UNIQUE or EXCLUDE constraint (`RENAME CONSTRAINT`) renames its index, and renaming the index (`ALTER INDEX ... RENAME`) renames the constraint; a CHECK or FOREIGN KEY constraint renames alone. A rename onto a name already taken changes nothing, since PostgreSQL fails the statement (`verify/tests/naming/rename_taken.sql`): a table, an index or an index-backed constraint needs the name free among the relations of its schema, and a constraint, or an index backing one, needs it free among the other constraints of its table. `DROP CONSTRAINT` matches the stored name only, and drops the index of the same name when the constraint is index-backed.

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

- **PGM0xx**: Unsafe DDL rules — locking, rewrites, runtime failures, silent side effects
- **PGM1xx**: Type anti-pattern rules ("Don't Do This")
- **PGM2xx**: Destructive operation rules — data loss
- **PGM3xx**: DML in migrations rules — INSERT, UPDATE, DELETE on existing tables
- **PGM4xx**: Idempotency guard rules
- **PGM5xx**: Schema design & informational rules
- **PGM9xx**: Meta-behaviors that modify how other rules operate (not standalone rules)

### 4.2 v1 Rules

Rule sections define behaviour: severity, what triggers the rule, what does not, and the finding message. The rationale, worked examples and recommended fix for each rule live in exactly one place, `src/rules/docs/pgmXXX.md`, which is the rule's module doc, the body that `--explain` prints, and the source of its section in `docs/rules.md`. Each heading title here must equal the rule's `DESCRIPTION` and each Severity bullet must start with its `DEFAULT_SEVERITY`; `cargo test --features docgen` checks both.

#### PGM001 — Missing `CONCURRENTLY` on `CREATE INDEX`

- **Severity**: CRITICAL
- **Triggers**: `CREATE INDEX` on a table that exists in the catalog AND is not created in the same set of changed files.
- **Logic**: If the target table appears in a `CREATE TABLE` within the changed files, the index is on a new table → no finding. Otherwise, `CONCURRENTLY` is required.
- **Message**: `CREATE INDEX on existing table '{table}' should use CONCURRENTLY to avoid holding an exclusive lock.`

#### PGM002 — Missing `CONCURRENTLY` on `DROP INDEX`

- **Severity**: CRITICAL
- **Triggers**: `DROP INDEX` without `CONCURRENTLY`, where the index belongs to an existing table (same logic as PGM001).
- **Message (non-partitioned)**: `DROP INDEX '{index}' on existing table '{table}' should use CONCURRENTLY to avoid holding an exclusive lock.`
- **Message (partitioned parent)**: `DROP INDEX '{index}' on partitioned table '{table}' will lock all partitions. CONCURRENTLY is not supported for partitioned parent indexes.`
- **Partition behavior**:
  - **ON ONLY index on partitioned parent**: Suppressed — dropping an invalid parent-only stub is safe, no child locks taken.
  - **Recursive/attached index on partitioned parent**: Emits the partition-specific message. PostgreSQL does NOT support `DROP INDEX CONCURRENTLY` on partitioned parent indexes; the standard advice does not apply.
  - **Non-partitioned table**: Standard CONCURRENTLY advice.
  - `ALTER INDEX ... ATTACH PARTITION` flips an ON ONLY index to recursive, so the suppression vs. warning is correctly distinguished even across migrations.
- **Does not fire when**:
  - The statement names the index behind a PRIMARY KEY or UNIQUE constraint, alone or with other indexes (`DROP INDEX a, b`), as PGM027 judges it. PostgreSQL rejects the whole statement and drops nothing; PGM027 reports it.

#### PGM003 — `CONCURRENTLY` inside transaction

- **Severity**: CRITICAL
- **Triggers**: `CREATE INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY`, `ALTER TABLE ... DETACH PARTITION ... CONCURRENTLY`, or `REINDEX ... CONCURRENTLY` inside a context that implies transactional execution:
  - Liquibase changeset without `runInTransaction="false"`
  - Plain SQL file while `[migrations] run_in_transaction` is true (the default)
- **Does not fire when**:
  - `DROP INDEX CONCURRENTLY` names the index behind a PRIMARY KEY or UNIQUE constraint, as PGM027 judges it. PostgreSQL rejects it in or out of a transaction, and PGM027 reports it; its fix, `ALTER TABLE ... DROP CONSTRAINT`, needs no `CONCURRENTLY`.
- **Message**: `CONCURRENTLY cannot run inside a transaction. Set runInTransaction="false" (Liquibase) or disable transactions for this migration.`

#### PGM004 — `DETACH PARTITION` on existing table without `CONCURRENTLY`

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... DETACH PARTITION child` without `CONCURRENTLY`, where the parent table exists in `catalog_before` (not created in the same set of changed files).
- **Safe alternative**: Use `DETACH PARTITION ... CONCURRENTLY` (PostgreSQL 14+), which uses SHARE UPDATE EXCLUSIVE instead, allowing concurrent reads and writes.
- **Does not fire when**:
  - `CONCURRENTLY` is present
  - The parent table is created in the same set of changed files
  - The parent table does not exist in `catalog_before`
- **Message**: `DETACH PARTITION on existing partitioned table '{table}' without CONCURRENTLY acquires ACCESS EXCLUSIVE on the entire table, blocking all reads and writes. Use DETACH PARTITION ... CONCURRENTLY (PostgreSQL 14+).`

#### PGM005 — `ATTACH PARTITION` of existing table without pre-validated `CHECK`

- **Severity**: MAJOR
- **Triggers**: `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...` where the child table exists in `catalog_before`, is not created in the same set of changed files, and has no CHECK constraint in the catalog.
- **Safe alternative** (3-step pattern):
  ```sql
  -- Step 1: add CHECK mirroring partition bound (NOT VALID)
  ALTER TABLE orders_2024 ADD CONSTRAINT orders_2024_bound_check
    CHECK (created_at >= '2024-01-01' AND created_at < '2025-01-01') NOT VALID;
  -- Step 2: validate separately (SHARE UPDATE EXCLUSIVE)
  ALTER TABLE orders_2024 VALIDATE CONSTRAINT orders_2024_bound_check;
  -- Step 3: attach (scan skipped)
  ALTER TABLE orders_partitioned ATTACH PARTITION orders_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
  ```
- **Does not fire when**:
  - The child table has at least one CHECK constraint (used as a proxy for partition-bound-matching CHECK)
  - The child is not in `catalog_before` (new table)
  - The child is created in the same set of changed files
  - The parent is created in the same set of changed files
- **Known limitation**: The rule uses presence of any CHECK as a proxy for a partition-bound-matching CHECK. It does not verify expression semantics.
- **Message**: `ATTACH PARTITION of existing table '{child}' to '{parent}' will scan the entire child table under ACCESS EXCLUSIVE lock to verify the partition bound. Add a CHECK constraint mirroring the partition bound, validate it separately, then attach.`

#### PGM006 — Volatile default on column

- **Severity**: MINOR for known volatile functions (`now()`, `current_timestamp`, `random()`, `gen_random_uuid()`, `uuid_generate_v4()`, `clock_timestamp()`, `timeofday()`, `txid_current()`, `nextval()`). INFO for any other function call used as a default.
- **Triggers**: `ADD COLUMN ... DEFAULT fn()` or inline in `CREATE TABLE`.
- **Note**: On Postgres 11+, non-volatile defaults on `ADD COLUMN` don't rewrite the table. Volatile defaults always evaluate per-row at write time, which is typically intentional — but worth flagging because developers sometimes use `now()` expecting a fixed value.
- **Message (known volatile)**: `Column '{col}' on '{table}' uses volatile default '{fn}()'. Unlike non-volatile defaults, this forces a full table rewrite under an ACCESS EXCLUSIVE lock — every existing row must be physically updated with a computed value. For large tables, this causes extended downtime. Consider adding the column without a default, then backfilling with batched UPDATEs.`
- **Message (nextval/serial)**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). This is standard usage — suppress if intentional. Note: on ADD COLUMN to an existing table, this is volatile and forces a table rewrite.`
- **Message (unknown function)**: `Column '{col}' on '{table}' uses function '{fn}()' as default. If this function is volatile (the default for user-defined functions), it forces a full table rewrite under an ACCESS EXCLUSIVE lock instead of a cheap catalog-only change. Verify the function's volatility classification.`

#### PGM007 — `ALTER COLUMN TYPE` on existing table causes table rewrite

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

#### PGM008 — `ADD COLUMN NOT NULL` without `DEFAULT` on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a `DEFAULT` clause, where the table exists in the catalog.
- **Note**: On PG 11+, `ADD COLUMN ... NOT NULL DEFAULT <value>` is safe (no rewrite for non-volatile defaults). Without a default, the command fails outright if any rows exist. This is almost always a bug.
- **Message**: `Adding NOT NULL column '{col}' to existing table '{table}' without a DEFAULT will fail if the table has any rows. Add a DEFAULT value, or add the column as nullable and backfill.`

#### PGM009 — `DROP COLUMN` on existing table

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... DROP COLUMN` where the table exists in the catalog.
- **Note**: Postgres marks the column as dropped without rewriting the table, so this is cheap at the database level. The risk is application-level: queries referencing the column will break. This is informational to increase visibility.
- **Message**: `Dropping column '{col}' from existing table '{table}'. The DDL is cheap but ensure no application code references this column.`

#### PGM016 — `ADD PRIMARY KEY` on existing table without `USING INDEX`

- **Severity**: MAJOR
- **Triggers**: `ALTER TABLE ... ADD PRIMARY KEY` on an existing table that does not use `USING INDEX`, or where the referenced index does not exist, is not UNIQUE, is not btree, or covers nullable columns. The index and its columns are judged as they stand just before the statement, as PGM027 judges them: `catalog_before` with the earlier statements of the unit replayed on top. An index created, dropped or renamed earlier in the unit counts, and so does a column made NOT NULL or nullable there.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - `USING INDEX` references a valid UNIQUE btree index and all PK columns are NOT NULL, also when the index was created, or the columns made NOT NULL, earlier in the same unit
  - `USING INDEX` names an index of another table in the table's schema. PostgreSQL rejects the statement and builds nothing; PGM027 reports it.
- **Message (no USING INDEX)**: `ADD PRIMARY KEY on existing table '{table}' without USING INDEX on column(s) [{columns}]. Create a UNIQUE index CONCURRENTLY first, then use ADD PRIMARY KEY USING INDEX.`
- **Message (index not found)**: `ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': referenced index does not exist.`
- **Message (index not unique)**: `ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': referenced index is not UNIQUE.`
- **Message (non-btree)**: `ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': referenced index uses access method '{method}', but only btree indexes can back a PRIMARY KEY constraint.`
- **Message (nullable columns)**: `ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': column(s) [{cols}] are nullable. PostgreSQL will implicitly SET NOT NULL under ACCESS EXCLUSIVE lock. Run ALTER COLUMN ... SET NOT NULL with a CHECK constraint first.`

#### PGM010 — `DROP COLUMN` silently removes unique constraint

- **Severity**: MINOR
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in a `UNIQUE` constraint or unique index on the table in `catalog_before`.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `Unique` or any `IndexState` where `is_unique` is true. If so, fire.
- **Does not fire when**:
  - The column is not part of any unique constraint or unique index
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes unique constraint '{constraint}'. Verify that the uniqueness guarantee is no longer needed.`

#### PGM011 — `DROP COLUMN` silently removes primary key

- **Severity**: MAJOR
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in the table's primary key (in `catalog_before`).
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `PrimaryKey`. If so, fire.
- **Does not fire when**:
  - The column is not part of the primary key
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes the primary key. The table will have no row identity. Add a new primary key or reconsider the column drop.`

#### PGM012 — `DROP COLUMN` silently removes foreign key

- **Severity**: MINOR
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in a `FOREIGN KEY` constraint on the table in `catalog_before`.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `ForeignKey`. If so, fire.
- **Does not fire when**:
  - The column is not part of any foreign key constraint
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes foreign key '{constraint}' referencing '{ref_table}'. Verify that the referential integrity guarantee is no longer needed.`

#### PGM013 — `SET NOT NULL` on existing table requires `ACCESS EXCLUSIVE` lock

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ALTER COLUMN ... SET NOT NULL` on a table that exists in `catalog_before` (not created in the same set of changed files).
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

#### PGM014 — `ADD FOREIGN KEY` on existing table without `NOT VALID`

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY ...` without `NOT VALID`, on a table that exists in `catalog_before` (not created in the same set of changed files).
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
- **Interaction with PGM501**: PGM501 (missing FK index) fires independently. The rules are complementary.
- **Message**: `Adding foreign key '{constraint}' on existing table '{table}' validates all rows, blocking writes. Use NOT VALID and validate in a separate migration.`

#### PGM015 — `ADD CHECK` on existing table without `NOT VALID`

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)` without `NOT VALID`, on a table that exists in `catalog_before` (not created in the same set of changed files).
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

#### PGM017 — `ADD UNIQUE` on existing table without `USING INDEX`

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD [CONSTRAINT ...] UNIQUE` on an existing table that does not use `USING INDEX`, or where the referenced index does not exist, is not UNIQUE, or is not btree. Without `USING INDEX` the rule fires even when a unique index or `UNIQUE` constraint on the same columns already exists, since PostgreSQL builds a new index anyway. The index is judged as it stands just before the statement, as PGM027 judges it: `catalog_before` with the earlier statements of the unit replayed on top. An index created, dropped or renamed earlier in the unit counts.
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
  - `USING INDEX` references a UNIQUE btree index of the table, also when the index was created earlier in the same unit
  - `USING INDEX` names an index of another table in the table's schema. PostgreSQL rejects the statement and builds nothing; PGM027 reports it.
- **Message (no USING INDEX)**: `ADD UNIQUE on existing table '{table}' without USING INDEX on column(s) [{columns}]. Create a unique index CONCURRENTLY first, then use ADD CONSTRAINT ... UNIQUE USING INDEX.`
- **Message (index not found)**: `ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': referenced index does not exist.`
- **Message (index not unique)**: `ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': referenced index is not UNIQUE.`
- **Message (non-btree)**: `ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': referenced index uses access method '{method}', but only btree indexes can back a UNIQUE constraint.`

#### PGM018 — `CLUSTER` on existing table

- **Severity**: CRITICAL
- **Triggers**: `CLUSTER table_name [USING index_name]` where the table exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `CLUSTER on table '{table}' [USING '{index}'] rewrites the entire table under ACCESS EXCLUSIVE lock for the full duration. All reads and writes are blocked. This is rarely appropriate in an online migration.`

#### PGM019 — `ADD EXCLUDE` constraint on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE (...)` where the table exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - The `EXCLUDE` constraint is part of a `CREATE TABLE` statement (only `ALTER TABLE` triggers)
- **Message**: `Adding EXCLUDE constraint on existing table '{table}' acquires ACCESS EXCLUSIVE lock and scans all rows. There is no online alternative — consider scheduling this during a maintenance window.`

#### PGM020 — `DISABLE TRIGGER` on table suppresses FK enforcement

- **Severity**: MINOR on existing tables, INFO on all other tables (new or unknown).
- **Triggers**: `ALTER TABLE ... DISABLE TRIGGER` (named trigger, `ALL`, or `USER`) on any table.
- **IR**: `AlterTableAction::DisableTrigger { scope: TriggerDisableScope }` where `TriggerDisableScope` is `Named(String)`, `All`, or `User`.
- **pg_query**: `AT_DisableTrig`, `AT_DisableTrigAll`, `AT_DisableTrigUser`.
- **Message**: Varies by trigger scope — includes scope label (ALL, USER, or trigger name) and scope-specific detail about what guarantees are lost.

#### PGM021 — `VACUUM FULL` on existing table

- **Severity**: CRITICAL
- **Triggers**: `VACUUM FULL` targeting a table that exists in `catalog_before`.
- **Does not fire when**:
  - `VACUUM` without `FULL` (regular vacuum does not block reads)
  - Table doesn't exist in `catalog_before`
  - Table is new (in `tables_created_in_change`)
- **IR**: `IrNode::VacuumFull(VacuumFull { table: Option<ResolvedName> })`.
- **Message**: `VACUUM FULL on existing table '{table}' rewrites the entire table under an ACCESS EXCLUSIVE lock. Use pg_repack or pg_squeeze for online compaction.`

#### PGM022 — Missing `CONCURRENTLY` on `REINDEX`

- **Severity**: CRITICAL
- **Triggers**: `REINDEX TABLE|INDEX|SCHEMA|DATABASE|SYSTEM` without the `CONCURRENTLY` option.
- **Does not fire when**:
  - `REINDEX ... CONCURRENTLY` is used
- **IR**: `IrNode::Reindex(Reindex { kind: ReindexObjectKind, target: ReindexTarget, concurrent: bool })`.
- **Note**: `REINDEX CONCURRENTLY` cannot run inside a transaction block. See PGM003.
- **Message**: `REINDEX {kind} '{target}' should use CONCURRENTLY to avoid holding an ACCESS EXCLUSIVE lock. Use REINDEX {kind} CONCURRENTLY '{target}' (PostgreSQL 12+).`

#### PGM023 — Multiple `ALTER TABLE` statements on the same table can be combined

- **Severity**: MINOR
- **Triggers**: Two or more `ALTER TABLE` statements targeting the same pre-existing table within a single migration file, where all statements operate at the same lock level.
- **Lock level classification**:
  - All actions are `VALIDATE CONSTRAINT` → `SHARE UPDATE EXCLUSIVE`
  - Any other action → `ACCESS EXCLUSIVE`
  - Statements with different lock levels are tracked independently.
- **Chain breaking**: Any intervening statement that references the same table and is not an `ALTER TABLE` action list (e.g., `CREATE INDEX`, `INSERT INTO`, or an `ALTER TABLE ... RENAME TO`, `RENAME COLUMN` or `RENAME CONSTRAINT`, none of which can join an action list) breaks the chain for that table.
- **Does not fire when**:
  - Single `ALTER TABLE` per table
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - Two `ALTER TABLE` statements with different lock levels
  - Statements on the same table are separated by a chain-breaking statement
- **Message**: `Table '{table}' has multiple ALTER TABLE statements with the same lock level in this migration (preceding statement at line {line}). Combine them into a single ALTER TABLE to reduce lock contention.`

#### PGM024 — `DROP TABLE` or `CREATE TABLE PARTITION OF` locks the parent partitioned table

- **Severity**: CRITICAL
- **Triggers**: Either of two shapes, where in both cases the parent exists in `catalog_before` and is not created in the same set of changed files:
  1. `DROP TABLE child`, where `child` is a partition of the parent (per the catalog's `parent_table`).
  2. `CREATE TABLE child PARTITION OF parent`.
- **Safe alternative (`DROP TABLE`)**: `DETACH PARTITION child CONCURRENTLY` (PostgreSQL 14+) first — see PGM004 — then drop the now-standalone table.
- **Safe alternative (`CREATE TABLE ... PARTITION OF`)**: There is no `CONCURRENTLY` form of this statement. Create the table standalone, then `ATTACH PARTITION` it — see PGM005 for the pre-validated `CHECK` constraint that avoids a full scan of the child during attach. `ATTACH PARTITION` takes only `SHARE UPDATE EXCLUSIVE` on the parent.
- **Does not fire when**:
  - The dropped table has no `parent_table` recorded (not a partition child)
  - The parent is created in the same set of changed files
  - The parent does not exist in `catalog_before`
  - `CREATE TABLE` does not use `PARTITION OF` (a plain standalone table)
  - The same migration unit `DETACH`es the child before dropping it (the safe pattern PGM004 recommends — see "Known limitation" below)
- **Known limitation**: a same-unit `ATTACH PARTITION child` immediately followed by `DROP TABLE child` is not specially detected either way — this rule (like PGM201) reads `catalog_before`, which is fixed before the whole unit is replayed, so it cannot see a same-unit `ATTACH`'s effect on its own catalog state. This is a narrow, unusual shape.
- **Message (`DROP TABLE`)**: `DROP TABLE '{child}' is a partition of '{parent}': this acquires ACCESS EXCLUSIVE on '{parent}' for the duration, blocking all reads and writes routed through it until commit — and the data is permanently lost. DETACH PARTITION '{child}' CONCURRENTLY first, then drop the now-standalone table.`
- **Message (`CREATE TABLE ... PARTITION OF`)**: `CREATE TABLE '{child}' PARTITION OF '{parent}' acquires ACCESS EXCLUSIVE on '{parent}' for the duration, blocking all reads and writes routed through it until commit. Create '{child}' as a standalone table and ATTACH PARTITION it instead — ATTACH only takes SHARE UPDATE EXCLUSIVE on the parent.`

#### PGM025 — `DROP COLUMN` silently removes EXCLUDE constraint

- **Severity**: MINOR
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in an `EXCLUDE` constraint on the table in `catalog_before`.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `Exclude`. If so, fire.
- **Does not fire when**:
  - The column is not part of any EXCLUDE constraint
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes EXCLUDE constraint '{constraint}'. Verify that the exclusion guarantee is no longer needed.`

#### PGM026 — One transaction locks several partitioned parent tables

- **Severity**: CRITICAL
- **Triggers**: One transaction whose statements lock two or more distinct partitioned parents, through `DROP TABLE child` where `child` is a partition (per the catalog's `parent_table`), `CREATE TABLE child PARTITION OF parent`, or `ALTER TABLE parent DETACH PARTITION child` without `CONCURRENTLY`, where the parent exists in `catalog_before` (for `DETACH`, as a partitioned table) and is not created in the same set of changed files. Each of the three takes ACCESS EXCLUSIVE on the parent (`verify/lock-tests/tests/partition_ddl_lock_matrix.rs`), and a transaction holds the locks of all of them together until it ends (`verify/lock-tests/tests/partition_multi_parent_txn.rs`). PGM024 flags the same `DROP TABLE` and `CREATE TABLE ... PARTITION OF` statements (one finding per parent in the unit), and PGM004 each plain `DETACH PARTITION`. A transaction is one of the unit's transaction windows (`LintContext::transaction_windows`): the whole unit under `run_in_transaction`, a `BEGIN` through the `COMMIT` or `ROLLBACK` that ends it, or one SQL statement outside a transaction block. `DROP TABLE a, b` is one statement, so it triggers alone when `a` and `b` are partitions of different parents: it holds ACCESS EXCLUSIVE on `a`'s parent while it waits for `b`'s (`verify/lock-tests/tests/partition_multi_parent_txn.rs`).
- **One finding per transaction**: at the first statement that locks a parent, naming each parent once, in the order the transaction first locks them.
- **Does not fire when**:
  - The transaction locks one parent, however many of its statements lock it
  - Each parent is locked in a different transaction: statements outside a transaction block without `run_in_transaction`, or separate `BEGIN ... COMMIT` blocks
  - A parent is created in the same set of changed files or is not in `catalog_before`, or, for a plain `DETACH PARTITION`, is not a partitioned table there; it does not count
  - A statement on a parent is `ATTACH PARTITION` or `DETACH PARTITION ... CONCURRENTLY`; it does not count. Both take only SHARE UPDATE EXCLUSIVE on the parent (`verify/lock-tests/tests/partition_ddl_lock_matrix.rs`, `verify/lock-tests/tests/pgm004_detach_partition_lock.rs`), and `DETACH ... CONCURRENTLY` cannot run inside a transaction block (SQLSTATE 25001, `verify/lock-tests/tests/pgm004_detach_partition_lock.rs`)
  - The dropped child is `DETACH`ed in the same migration unit (as for PGM024); the drop does not count, but a plain `DETACH` before it does
- **Other rules**: PGM024 flags the same `DROP TABLE` and `CREATE TABLE ... PARTITION OF` statements (one finding per parent in the unit), and PGM004 each plain `DETACH PARTITION`; they fire alongside this rule, which fires once per transaction.
- **Known limitation**: under `run_in_transaction`, the statements after a `COMMIT` or `ROLLBACK` in the file count as autocommit, one transaction each, as `psql --single-transaction` runs them. JDBC runners with autocommit off (Flyway, Liquibase) run them in a new transaction, and golang-migrate's single `Exec` of the file runs them as a new implicit transaction, so this rule can miss parents that such a runner locks together after the file's `COMMIT`. With `run_in_transaction = false`, golang-migrate's default single `Exec` runs the whole file as one implicit transaction, which this rule does not model: it counts each statement outside a `BEGIN` block as a transaction of its own, so it misses two parents locked in such a file.
- **Message**: `One transaction takes ACCESS EXCLUSIVE on {count} partitioned parents ('{parent}', '{parent}', ...) and holds each lock until it ends. While it waits for the next parent's lock, all reads and writes on the parents it already locked are blocked, and if a lock times out, the work on every parent rolls back. Change one parent per transaction, for example one migration per parent.`

#### PGM027 — Statement fails against the schema from earlier migrations

- **Severity**: MAJOR
- **Triggers**: A statement PostgreSQL rejects, given the schema replayed from earlier migrations. Each statement is judged against `catalog_before` with the earlier statements of its unit replayed on top, so a name an earlier statement of the unit frees or takes counts. Fires only on positive evidence, in these cases:
  1. `ALTER TABLE t ADD [CONSTRAINT c] PRIMARY KEY USING INDEX idx` or `... UNIQUE USING INDEX idx`, where an index `idx` is registered in `t`'s schema and belongs to another table. PostgreSQL fails it with 55000 (`verify/tests/other/pgm027_using_index_of_another_table.sql`). `t` itself need not be in the catalog.
  2. `ALTER TABLE ... RENAME TO`, `ALTER INDEX ... RENAME TO` or `ALTER TABLE ... RENAME CONSTRAINT` onto a taken name, as `Catalog::table_rename_clash`, `index_rename_clash` and `constraint_rename_clash` judge it (`verify/tests/naming/rename_taken.sql`): a table, an index or an index-backed constraint needs the name free among the relations of its schema (tables, indexes, and the indexes of index-backed constraints), and a constraint, or an index backing one, needs it free among the other constraints of its table. Renaming onto the current name counts.
  3. `DROP INDEX idx`, with or without `CONCURRENTLY` or `IF EXISTS`, where `idx` is a registered index backing a PRIMARY KEY or UNIQUE constraint of its table. PostgreSQL fails it with 2BP01 (`verify/tests/other/pgm027_drop_constraint_index.sql`). An EXCLUDE constraint's index counts once the catalog registers one.
- **Does not fire when**:
  - The `USING INDEX` index, the renamed table, index or constraint, or the dropped index is not in the catalog
  - The `USING INDEX` index belongs to the altered table, or only another schema has an index of that name
  - The new name is taken only in another schema, or, for a CHECK or FOREIGN KEY constraint, only by a relation or by another table's constraint
  - An earlier statement of the unit freed the name, by dropping or renaming away the object that had it
  - An earlier statement of the unit dropped the constraint behind the index
- **One finding per statement**: `DROP INDEX a, b` reports only the first index that fails, as PostgreSQL stops there.
- **Message (USING INDEX)**: `ADD {PRIMARY KEY|UNIQUE} USING INDEX '{index}' on table '{table}' fails: the index belongs to table '{owner}'.`
- **Message (rename)**: `Renaming table '{table}' to '{new_name}' fails: {reason}.`, `Renaming index '{index}' to '{new_name}' fails: {reason}.` or `Renaming constraint '{constraint}' on table '{table}' to '{new_name}' fails: {reason}.`, where `{reason}` is `table '{holder}' already has that name`, `index '{new_name}' on table '{holder}' already has that name` or `table '{holder}' already has a constraint named '{new_name}'`. An index backing a constraint, renamed onto a constraint name of its table, adds `the index backs a constraint that is renamed with it, and ` before the reason; an index-backed constraint renamed onto a relation name adds `the constraint's index is renamed with it, and `.
- **Message (DROP INDEX)**: `DROP INDEX '{index}' fails: constraint '{constraint}' on table '{table}' requires it. Drop the constraint instead.`
- **Other rules**: PGM504 does not fire on a rename this rule reports. PGM016 and PGM017 do not fire on a `USING INDEX` of another table's index, and PGM107 does not read that index's columns. PGM002, PGM003, PGM401 and PGM501 do not fire on a `DROP INDEX` this rule reports, including the other indexes it names.

#### PGM201 — `DROP TABLE` on existing table

- **Severity**: MINOR
- **Triggers**: `DROP TABLE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `DROP TABLE '{table}' removes an existing table. This is irreversible and all data will be lost.`

#### PGM202 — `DROP TABLE CASCADE` on existing table

- **Severity**: MAJOR
- **Triggers**: `DROP TABLE ... CASCADE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - `DROP TABLE` without `CASCADE` (handled by PGM201)
- **Message (no known FK deps)**: `DROP TABLE CASCADE on '{table}' will silently drop all dependent objects (views, foreign keys, triggers). Review dependencies before proceeding.`
- **Message (with FK deps)**: `DROP TABLE CASCADE on '{table}' will silently drop dependent objects. Known FK dependencies from: {dep_tables}.`

#### PGM203 — `TRUNCATE TABLE` on existing table

- **Severity**: MINOR
- **Triggers**: `TRUNCATE TABLE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `TRUNCATE TABLE '{table}' removes all rows from an existing table. This is irreversible and does not fire ON DELETE triggers.`

#### PGM204 — `TRUNCATE TABLE CASCADE` on existing table

- **Severity**: MAJOR
- **Triggers**: `TRUNCATE TABLE ... CASCADE` where the target table exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - `TRUNCATE` without `CASCADE` (handled by PGM203)
- **Message (no known FK deps)**: `TRUNCATE TABLE '{table}' CASCADE silently extends to all tables with foreign key references to '{table}', and recursively to their dependents. Verify the full cascade chain is intentionally truncated.`
- **Message (with FK deps)**: `TRUNCATE TABLE '{table}' CASCADE silently extends to all tables with foreign key references to '{table}', and recursively to their dependents. Known FK dependencies from: {dep_tables}.`

#### PGM205 — `DROP SCHEMA CASCADE`

- **Severity**: CRITICAL
- **Triggers**: `DROP SCHEMA ... CASCADE` regardless of catalog state.
- **Does not fire when**:
  - `DROP SCHEMA` without `CASCADE` (PostgreSQL errors at runtime if the schema is non-empty)
- **Message (no known tables)**: `DROP SCHEMA '{schema}' CASCADE drops every object in the schema — tables, views, sequences, functions, and types. This is irreversible.`
- **Message (with known tables)**: `DROP SCHEMA '{schema}' CASCADE drops every object in the schema — tables, views, sequences, functions, and types. This is irreversible. Known affected tables: {table_list}.`

#### PGM301 — `INSERT INTO` existing table in migration

- **Severity**: INFO
- **Triggers**: `INSERT INTO` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `INSERT INTO existing table '{table}' in a migration. Ensure this is intentional seed data and that row volume is bounded.`

#### PGM302 — `UPDATE` on existing table in migration

- **Severity**: MINOR
- **Triggers**: `UPDATE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `UPDATE on existing table '{table}' in a migration. Unbatched updates hold row locks for the full statement duration. Verify row volume and consider batched execution.`

#### PGM303 — `DELETE FROM` existing table in migration

- **Severity**: MINOR
- **Triggers**: `DELETE FROM` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `DELETE FROM existing table '{table}' in a migration. Unbatched deletes hold row locks and generate significant WAL. Verify row volume and consider batched execution.`

#### PGM402 — Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`

- **Severity**: MINOR
- **Triggers**: `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause.
- **Does not fire when**:
  - The statement already includes `IF NOT EXISTS`
  - The `CREATE INDEX` has no name. PostgreSQL generates one that is free in the schema (`{table}_{cols}_idx`, then `_idx1`, `_idx2`, ...; `verify/tests/naming/collisions.sql`), so the statement cannot fail on an existing index, and `IF NOT EXISTS` requires a name
- **Message (CREATE TABLE)**: `CREATE TABLE '{table}' without IF NOT EXISTS will fail if the table already exists.`
- **Message (CREATE INDEX)**: `CREATE INDEX '{index}' without IF NOT EXISTS will fail if the index already exists.`

#### PGM403 — `CREATE TABLE IF NOT EXISTS` for already-existing table is a misleading no-op

- **Severity**: MINOR
- **Triggers**: `CREATE TABLE IF NOT EXISTS` targeting a table that already exists in `catalog_before` at that point in the migration history.
- **Does not fire when**:
  - The table does not already exist in the catalog (the statement genuinely creates it).
  - `IF NOT EXISTS` is absent (a duplicate `CREATE TABLE` without the guard would fail at runtime, which is a different problem).
- **Message**: `CREATE TABLE IF NOT EXISTS '{table}' is a no-op — the table already exists in the migration history. The definition in this statement is silently ignored by PostgreSQL. If the column definitions differ from the actual table state, this migration is misleading.`

#### PGM401 — Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`

- **Severity**: MINOR
- **Triggers**: `DROP TABLE` or `DROP INDEX` without the `IF EXISTS` clause.
- **Does not fire when**:
  - The statement already includes `IF EXISTS`
  - `DROP INDEX` names the index behind a PRIMARY KEY or UNIQUE constraint, as PGM027 judges it. PostgreSQL rejects it with or without `IF EXISTS`; PGM027 reports it.
- **Message (DROP TABLE)**: `DROP TABLE '{table}' without IF EXISTS will fail if the table does not exist.`
- **Message (DROP INDEX)**: `DROP INDEX '{index}' without IF EXISTS will fail if the index does not exist.`

#### PGM501 — Foreign key without covering index on referencing columns

- **Severity**: MAJOR
- **Triggers**:
  1. A foreign key on `cols` added by `CREATE TABLE` (a `FOREIGN KEY (cols) REFERENCES ...` table constraint, or a column's inline `REFERENCES`) or by `ALTER TABLE` (`ADD CONSTRAINT ... FOREIGN KEY (cols) REFERENCES ...`, or `ADD COLUMN ... REFERENCES`), where no usable index (B-tree or hash, non-partial, not ON ONLY) on the referencing table contains any of `cols`.
  2. A statement that drops a usable index covering a foreign key that exists before and after the unit, so the key is covered in `catalog_before` and not in `catalog_after`: `DROP INDEX [CONCURRENTLY]`; `ALTER TABLE ... DROP CONSTRAINT` of a PRIMARY KEY or UNIQUE constraint, which drops its index; `ALTER TABLE ... DROP COLUMN` of any column of the index, which drops the index while a key on its other columns stays. The key is matched by table and constraint name. A partition child's key covered by its parent's recursive index loses that coverage when the parent's index is dropped. When several statements in the unit drop covering indexes of the same key, the one finding goes to the last of them.
- **Column matching**: FK columns `(a, b)` are covered by any usable index containing at least one of `a` or `b`, in any position — e.g. `(a, b)`, `(b, a)`, `(a)`, or `(c, b)` all count. An index covering only some FK columns still avoids a genuine sequential scan (Index Scan with a Filter, or Bitmap Heap Scan with a Recheck), so it counts as coverage even though a fully covering index performs better. Column order does not matter here (contrast with PGM508's prefix-based redundant-index check, which does care about order).
- **Catalog lookup**: checks indexes on the referencing table after the full file/changeset is processed (not at the point of FK creation). This avoids false positives when the index is created later in the same file/changeset.
- **Index exclusions**: Partial indexes (with WHERE clause) and ON ONLY indexes (`only: true`) are excluded from coverage checks — partial indexes only cover a subset of rows, and ON ONLY indexes are invalid parent stubs that don't provide real FK coverage. Only B-tree and hash indexes count: GIN, GiST, BRIN and other access methods don't serve the equality lookups FK enforcement needs.
- **Partition behavior**:
  - **Partitioned parent tables**: Checks `has_indexed_fk_column` normally. A recursive index (not ON ONLY) satisfies coverage. An ON ONLY index does not.
  - **Partition children**: Checks the child's own indexes first. If none found, delegates to the parent table's indexes via `parent_table`. If the parent is not in the catalog, suppresses conservatively (common in incremental CI where the parent was created outside tracked migrations).
  - `ALTER INDEX ... ATTACH PARTITION` flips `only` to `false`, so after all children are attached, the parent index correctly satisfies FK coverage.
- **Does not fire (trigger 2) when**:
  - The foreign key is dropped in the same unit, with `DROP CONSTRAINT`, by dropping one of its columns, or with its table.
  - Another usable index still contains one of the key's columns.
  - The unit adds a foreign key on the same table and columns, including one dropped and added back under its name. Trigger 1 reports it once.
  - The statement is `DROP INDEX` of the index behind a PRIMARY KEY, UNIQUE or EXCLUDE constraint, which PostgreSQL rejects. Replay keeps the index, and PGM027 reports the statement.
  - The statement is one PostgreSQL rejects, as PGM027 judges it, such as `DROP INDEX` of a covering index together with the index behind a PRIMARY KEY or UNIQUE constraint (`DROP INDEX covering_idx, orders_pkey`). It drops nothing, though replay drops the covering index; PGM027 reports it.
  - The dropped index was created in the same unit, so the key was not covered by it before.
  - The dropped constraint is an EXCLUDE constraint. The catalog records no index for it, so it never counts as coverage.
- **Message (trigger 1)**: `Foreign key on '{table}({cols})' has no covering index. Queries joining or filtering on these columns, and referential integrity checks, cause sequential scans on the referencing table.`
- **Message (trigger 2, DROP INDEX)**: `Dropping index '{index}' leaves foreign key '{fk}' on '{table}({cols})' without a covering index. Queries joining or filtering on these columns, and referential integrity checks, cause sequential scans on the referencing table.`
- **Message (trigger 2, DROP CONSTRAINT)**: `Dropping constraint '{constraint}' drops its index, which leaves foreign key '{fk}' on '{table}({cols})' without a covering index. Queries joining ...` (same closing sentence)
- **Message (trigger 2, DROP COLUMN)**: `Dropping column '{column}' drops index '{index}', which leaves foreign key '{fk}' on '{table}({cols})' without a covering index. Queries joining ...` (same closing sentence; `drops indexes 'a', 'b'` when it drops more than one that covered the key)
- **Accepted false negative**: an FK whose only indexed column is low-cardinality (e.g. 10 distinct values over 1M rows) produces no warning, even though the RI check still degrades to a Bitmap Heap Scan touching a large fraction of the table. This cannot be distinguished from a genuinely selective index using shape alone — the catalog has no column statistics, and requiring the covering index to also be UNIQUE does not rescue it, since the false-negative case is not unique in the referencing table either.

#### PGM502 — Table without primary key

- **Severity**: MAJOR
- **Triggers**: `CREATE TABLE` (non-temporary) with no `PRIMARY KEY` constraint, checked after the full file/changeset is processed (to allow `ALTER TABLE ... ADD PRIMARY KEY` later in the same file). An `ADD PRIMARY KEY USING INDEX` of another table's index adds no key, as PostgreSQL rejects it and PGM027 reports it; one of an index the catalog does not have counts.
- **Message**: `Table '{table}' has no primary key.`

#### PGM503 — `UNIQUE NOT NULL` used instead of `PRIMARY KEY`

- **Severity**: INFO
- **Triggers**: Table has no PK but has at least one `UNIQUE` constraint where all constituent columns are `NOT NULL`.
- **Message**: `Table '{table}' uses UNIQUE NOT NULL instead of PRIMARY KEY. Functionally equivalent but PRIMARY KEY is conventional and more explicit.`

#### PGM504 — `RENAME TABLE` on existing table

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... RENAME TO ...` on a table that exists in `catalog_before`.
- **Replacement detection**: Does **not** fire if, within the same migration unit, a `CREATE TABLE` with the old name appears after the rename. This is a common pattern (rename old table away, create replacement with the original name).
- **Does not fire when**:
  - The table does not exist in `catalog_before`
  - A replacement table with the old name is created in the same migration unit
  - The new name is taken (`Catalog::table_rename_clash`, judged against the state just before the statement as PGM027 judges it). PostgreSQL rejects the rename, so nothing breaks; PGM027 reports it.
- **Message**: `Renaming table '{old_name}' to '{new_name}'. Ensure all application queries, views, and functions referencing the old name are updated.`

#### PGM505 — `RENAME COLUMN` on existing table

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... RENAME COLUMN ... TO ...` on a table that exists in `catalog_before`.
- **Does not fire when**:
  - The table does not exist in `catalog_before`
- **Message**: `Renaming column '{old_name}' to '{new_name}' on table '{table}'. Ensure all application queries, views, and functions referencing the old column name are updated.`

#### PGM506 — `CREATE UNLOGGED TABLE`

- **Severity**: INFO
- **Triggers**: `CREATE TABLE ... UNLOGGED` for any table.
- **Does not fire when**:
  - The `UNLOGGED` keyword is absent (permanent or temporary tables).
- **Message**: `CREATE UNLOGGED TABLE '{table}'. Unlogged tables are truncated on crash recovery and not replicated to standbys. Confirm this is intentional.`

#### PGM507 — `DROP NOT NULL` on existing table allows NULL values

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... ALTER COLUMN ... DROP NOT NULL` on a table that exists in `catalog_before` (not created in the same set of changed files).
- **Does not fire when**:
  - The table is created in the same set of changed files
  - The table does not exist in `catalog_before`
- **Message**: `DROP NOT NULL on column '{col}' of existing table '{table}' allows NULL values where the application may assume non-NULL. Verify that all code paths handle NULLs.`

#### PGM508 — Duplicate or redundant index detected (prefix of another index)

- **Severity**: INFO
- **Triggers**: `CREATE INDEX` in the changed file where, after applying the migration (`catalog_after`), a non-unique index on a table is a column prefix of another index on the same table. Fires in three directions:
  1. The new index is an exact duplicate of an existing index (same columns, same access method).
  2. The new index is redundant (its columns are a prefix of an existing index).
  3. The new index makes an existing non-unique index redundant (existing index's columns are a prefix of the new one).
- **Does not fire when**:
  - The shorter (potentially redundant) index is a UNIQUE index — it enforces a constraint that the longer index does not.
  - Either index is a partial index (has a WHERE clause) — partial indexes serve different query patterns.
  - Either index has expression entries — expression indexes are not directly comparable by column name.
  - The indexes use different access methods (e.g., btree vs GIN) — different access methods serve fundamentally different query types.
  - The statement is `CREATE INDEX IF NOT EXISTS` and, at that point in the migration history, a relation in the table's schema already has its name: an index on any table, a table, or an index-backed constraint. PostgreSQL skips the statement with a notice and creates no index, and the catalog replay does the same.
  - More than one index on the table has the new index's name, so which one the statement created is unknown. A generated name gets a collision suffix instead (§3.3), so this takes an explicit name that repeats one the table already has, which PostgreSQL rejects.
- **Message (exact duplicate)**: `Index '{new_idx}' on '{table}' ({cols}) is an exact duplicate of index '{existing_idx}'.`
- **Message (new is prefix)**: `Index '{shorter_idx}' on '{table}' ({shorter_cols}) is redundant — index '{longer_idx}' ({longer_cols}) covers the same prefix.`
- **Message (new makes existing redundant)**: `Index '{new_idx}' on '{table}' ({new_cols}) makes existing index '{existing_idx}' ({existing_cols}) redundant — the new index covers the same prefix.`

#### PGM509 — Mixed-case identifier or reserved word requires double-quoting

- **Severity**: INFO
- **Triggers**: Table or column names that require perpetual double-quoting. Detected in `CREATE TABLE` (table name and all column names), `ALTER TABLE ... ADD COLUMN` (column name), `RENAME TABLE` (new name), and `RENAME COLUMN` (new name). A name requires quoting if it contains uppercase characters or is a PostgreSQL reserved word.
- **Detection strategy**: `pg_query` (libpg_query) lowercases unquoted identifiers and preserves case for quoted ones. If a name contains uppercase characters, it was necessarily quoted in the DDL. If a name is a reserved word and the parse succeeded, it was necessarily quoted.
- **Does not fire when**:
  - The identifier is all-lowercase and not a PostgreSQL reserved word.
  - The identifier is a schema name or index name (only table and column names are checked).
- **Message (table)**: `Table '{name}' requires double-quoting ({reason}).`
- **Message (column)**: `Column '{col}' on table '{table}' requires double-quoting ({reason}).`
- **Message (renamed table)**: `Table '{new_name}' (renamed from '{old_name}') requires double-quoting ({reason}).`

#### PGM901 — Down migration severity cap

- **All down-migration findings are capped at INFO severity**, regardless of what the rule would normally produce.
- The same rules run on down migrations, but findings are informational only.
- PGM901 is a meta-behavior, not a standalone lint rule. It has no `Rule` trait implementation and cannot be suppressed or disabled via inline comments. The 9xx range is reserved for meta-behaviors that modify how other rules operate.
- **Scope**: Down migration detection relies on filename patterns (`.down.sql` / `_down.sql` suffixes). Liquibase `<rollback>` blocks are not currently detected as down migrations (see §2.2).

### 4.3 Type Anti-pattern Rules (PGM1xx)

Rules derived from the [PostgreSQL "Don't Do This" wiki](https://wiki.postgresql.org/wiki/Don%27t_Do_This). These detect column type anti-patterns in `CREATE TABLE`, `ALTER TABLE ... ADD COLUMN`, and `ALTER TABLE ... ALTER COLUMN TYPE` statements.

PGM101–PGM104, PGM106, PGM108 and PGM109 share a common detection pattern: inspect `TypeName` on `ColumnDef` in `CreateTable` columns, `AddColumn` actions, and `AlterColumnType` actions.

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

#### PGM101 — Column uses `timestamp` without time zone

- **Severity**: MINOR
- **Triggers**: Column type with `TypeName.name == "timestamp"` in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`.
- **Message**: `Column '{col}' on '{table}' uses 'timestamp without time zone'. Use 'timestamptz' (timestamp with time zone) instead to store unambiguous points in time.`

#### PGM102 — Column uses `timestamp` or `timestamptz` with precision 0

- **Severity**: MINOR
- **Triggers**: Column type with `TypeName.name` in `("timestamp", "timestamptz")` and `TypeName.modifiers == [0]`.
- **Message**: `Column '{col}' on '{table}' uses '{type}(0)'. Precision 0 causes rounding, not truncation — a value of '23:59:59.9' rounds to the next day. Use full precision and format on output instead.`

#### PGM103 — Column uses `char(n)` type

- **Severity**: MINOR
- **Triggers**: Column type with `TypeName.name == "bpchar"` (pg_query canonical form for SQL `char`/`character`).
- **Message**: `Column '{col}' on '{table}' uses 'char({n})'. The char(n) type pads with spaces, wastes storage, and is no faster than text or varchar in PostgreSQL. Use text or varchar instead.`

#### PGM104 — Column uses the `money` type

- **Severity**: MINOR
- **Triggers**: Column type with `TypeName.name == "money"`.
- **Message**: `Column '{col}' on '{table}' uses the 'money' type. The money type depends on the lc_monetary locale setting, making it unreliable across environments. Use numeric(p,s) instead.`

#### PGM105 — Column uses `serial`/`bigserial` instead of identity column

- **Severity**: INFO
- **Triggers**: Column with `DefaultExpr::FunctionCall { name: "nextval", .. }` on an `int4`, `int8`, or `int2` typed column. (pg_query expands `serial`→`int4 + nextval()`, `bigserial`→`int8 + nextval()`.)
- **Interaction with PGM006**: Both PGM105 and PGM006 fire on `nextval()` defaults. This is intentional — PGM006 warns about the volatile default aspect, PGM105 recommends the identity column alternative.
- **Message**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). Prefer GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY for new tables (PostgreSQL 10+). Identity columns have better ownership semantics and are the SQL standard approach.`

#### PGM106 — Column uses `json` type instead of `jsonb`

- **Severity**: MINOR
- **Triggers**: Column type with `TypeName.name == "json"` in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`.
- **Message**: `Column '{col}' on '{table}' uses 'json'. Use 'jsonb' instead — it's faster, smaller, indexable, and supports containment operators. Only use 'json' if you need to preserve exact text representation or key order.`

#### PGM107 — Primary key column uses `integer` or `smallint` instead of `bigint`

- **Severity**: MAJOR
- **Triggers**: A primary key column with `TypeName.name` in `("int4", "int2")` — i.e., `integer`, `smallint`, or their aliases. Detected in `CREATE TABLE` (inline PK or table-level `PRIMARY KEY` constraint), whose own column definitions give the types, and `ALTER TABLE ... ADD PRIMARY KEY`, which reads the types from the table as it stands just before the statement (`catalog_before` with the earlier statements of the unit replayed on top), so a column added or retyped earlier in the unit counts with its new type. For `ADD PRIMARY KEY USING INDEX`, resolves PK columns from the referenced index as it stands then too.
- **Does not fire when**:
  - The PK column type is `int8` / `bigint`
  - The column is not part of a primary key
  - `ALTER TABLE ... ADD PRIMARY KEY` on a table not in the catalog just before the statement
  - `ALTER TABLE ... ADD PRIMARY KEY (cols)` after an earlier statement of the unit changed the columns to `bigint`
  - `ADD PRIMARY KEY USING INDEX` names an index that is missing just before the statement or belongs to another table (PostgreSQL rejects the latter; PGM027 reports it)
- **Message**: `Primary key column '{col}' on '{table}' uses {type}. Consider using bigint to avoid exhausting the integer range on high-write tables.`

#### PGM108 — Column uses `varchar(n)` instead of `text`

- **Severity**: INFO
- **Triggers**: Column type with `TypeName.name == "varchar"` and non-empty `TypeName.modifiers` (i.e., `varchar(n)` with a length) in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`. Bare `varchar` without a length is not flagged.
- **Message**: `Column '{col}' on '{table}' uses varchar({n}). Prefer text — varchar(n) has no performance benefit in PostgreSQL and adds an artificial limit that may require future schema changes.`

#### PGM109 — Column uses floating-point type instead of `numeric`

- **Severity**: MINOR
- **Triggers**: Column type with `TypeName.name` in `("float4", "float8")` in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`.
- **Message**: `Column '{col}' on '{table}' uses '{type}'. Floating-point types have precision issues (0.1 + 0.2 ≠ 0.3). Use numeric for exact values.`

#### Deferred "Don't Do This" Rules

The following rules are specified but deferred until per-rule enable/disable configuration is implemented. Rule IDs are assigned only when a rule is promoted to implementation.

- Don't use `INHERITS` for partitioning (MINOR). Requires IR extension to detect `CREATE TABLE ... INHERITS`.

### 4.4 `--explain PGMnnn`

Prints a detailed explanation of the rule: what it detects, why it's dangerous, concrete examples of the failure mode, and how to fix it. The header lines (rule, severity, enabled by default, description) are followed by the rule's `src/rules/docs/pgmXXX.md` printed verbatim as CommonMark. Exits 0. No file scanning.

### 4.5 `--list-rules [enabled|disabled]`

Lists every non-meta rule with its ID, default severity, and enabled-by-default status, one per line. Omit the filter to list all rules; `enabled`/`disabled` narrow the listing to rules matching that `default_enabled()` state. Exits 0. An unrecognized filter value is a CLI parse error (exit 2). No file scanning.

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

Multiple rules in one comment: `-- pgm-lint:suppress PGM001,PGM501`

Suppressions apply before grouping (§7.4). A next-statement suppression removes that one statement from its group and from the count that decides whether the rule's findings are grouped; a file-level suppression removes the whole group.

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
# Unqualified names resolve to the schema <default_schema> for catalog lookups.
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
# Rule IDs to disable globally. Findings from disabled rules are not emitted.
# Invalid rule IDs cause a config-load error (exit 2).
disabled = []

# Rule IDs to force-enable, overriding a rule's default-enabled state.
# Some rules are opt-in and only run when listed here -- run
# `--explain <RULE>` to check whether a given rule is enabled by default.
# Listing the same rule in both `enabled` and `disabled` is a config error.
enabled = []

# Per-rule severity overrides (future, not v1 — included for schema stability)
# [rules.PGM001]
# severity = "MAJOR"

[output]
# Formats to produce: "sarif", "sonarqube", "text"
formats = ["sarif", "sonarqube"]

# Output directory
dir = "build/reports/migration-lint"

# Group repeated findings (§7.4): once one rule reports at least this many
# findings at one severity in one file, report them as one finding.
# 1 = one finding per rule, severity and file; 0 = never group. Default: 5.
group_threshold = 5

[cli]
# Exit code threshold: "blocker", "critical", "major", "minor", "info", "none"
# Tool returns non-zero if any finding meets or exceeds this severity
fail_on = "critical"
```

---

## 7. Output Formats

### 7.1 SonarQube Generic Issue Import

SonarQube 10.3+ format: rule metadata (`cleanCodeAttribute`, `type`, `impacts`) lives in a top-level `rules` array that lists only the rules that fired, and each issue carries `ruleId`, `effortMinutes` and `primaryLocation`. A grouped finding (§7.4) is one issue: its lowest-line finding is the `primaryLocation` and the other findings are `secondaryLocations`, each with its own message (several can share a line when the rule fires per column). Its `effortMinutes` is the rule's effort times the number of findings in the group, so grouping does not lower the technical debt SonarQube adds up.

```json
{
  "rules": [
    {
      "id": "PGM401",
      "name": "Missing IF EXISTS on DROP TABLE / DROP INDEX",
      "description": "Missing IF EXISTS on DROP TABLE / DROP INDEX. See https://robert-sjoblom.github.io/pg-migration-lint/rules#pgm401",
      "engineId": "pg-migration-lint",
      "cleanCodeAttribute": "COMPLETE",
      "type": "CODE_SMELL",
      "severity": "MINOR",
      "impacts": [{ "softwareQuality": "MAINTAINABILITY", "severity": "MEDIUM" }]
    }
  ],
  "issues": [
    {
      "ruleId": "PGM401",
      "effortMinutes": 30,
      "primaryLocation": {
        "message": "DROP TABLE 'report_a' without IF EXISTS will fail if the table does not exist.",
        "filePath": "db/migrations/V043__drop_reports.sql",
        "textRange": { "startLine": 1, "endLine": 1 }
      },
      "secondaryLocations": [
        {
          "message": "DROP TABLE 'report_b' without IF EXISTS will fail if the table does not exist.",
          "filePath": "db/migrations/V043__drop_reports.sql",
          "textRange": { "startLine": 2, "endLine": 2 }
        },
        {
          "message": "DROP TABLE 'report_c' without IF EXISTS will fail if the table does not exist.",
          "filePath": "db/migrations/V043__drop_reports.sql",
          "textRange": { "startLine": 3, "endLine": 3 }
        }
      ]
    }
  ]
}
```

An ungrouped finding has no `secondaryLocations`.

### 7.2 SARIF

Standard SARIF 2.1.0 schema. Upload to GitHub via `github/codeql-action/upload-sarif@v3`. This produces inline PR annotations with no API integration needed.

A grouped finding (§7.4) is one result. Its `locations` hold one location per finding, lowest line first, and each location carries that finding's own `message`. GitHub Code Scanning shows only the first location, so the result message is the first finding's message followed by the other lines (§7.4), for example `Also at lines 2, 3.` GitHub rejects an upload in which one result has more than 1,000 locations, so a group of more than 1,000 findings becomes one result per 1,000 findings, each built the same way.

### 7.3 Text

Human-readable for local development:

```
CRITICAL PGM001 db/migrations/V042__add_order_index.sql:3
  CREATE INDEX on existing table 'orders' should use CONCURRENTLY.

MAJOR PGM501 db/migrations/V042__add_order_index.sql:7
  Foreign key on 'order_items(order_id)' has no covering index.
```

A grouped finding (§7.4) is reported at its lowest line, with one more line naming the other lines (§7.4):

```
MINOR PGM401 db/migrations/V043__drop_reports.sql:1
  DROP TABLE 'report_a' without IF EXISTS will fail if the table does not exist.
  Also at lines 2, 3.
```

### 7.4 Grouped findings

When one rule fires many times in one file (for example 192 `DROP TABLE` statements without `IF EXISTS`, each a PGM401 finding), the tool reports one finding for them that lists every location.

- **When**: once a rule reports at least `output.group_threshold` findings at the same severity in one file (default 5). Findings at different severities group separately, so a group never changes a finding's severity: a rule that reports some statements at a lower severity (PGM006, PGM007, PGM020) forms one group per severity. PGM901's INFO cap applies per unit before grouping, and down migrations are files of their own. `group_threshold = 1` makes one finding per rule, severity and file; `group_threshold = 0` turns grouping off.
- **Order**: suppression comments (§5) and per-unit dedup apply first, so a suppressed statement is never a member and does not count toward the threshold. Grouping runs once over every changed unit's findings, so the findings of one file group together even when the file holds several units (Liquibase changesets).
- **Shape**: the group's lowest-line finding is its primary location, with that finding's message. The other findings are its additional locations, ordered by line, each with the message the rule gave it. A rule that fires per column (PGM101, PGM105, PGM108) can put several findings on one statement, so several locations can share a line. The group takes the place of the first of its findings in the output order; other findings keep their order.
- **Naming the other lines**: where a reporter shows one location per finding (text, the SARIF result message, `github-review`), it follows the message with the start lines of the group's other locations, each named once and leaving out the line the finding is shown at: `Also at lines 15, 22.` When the group has more findings than the lines named, the sentence counts them: `Also at lines 15, 22 (5 more findings).`, or `Also 2 more findings at this line.` when they all share the shown line.
- **Counting**: a group is one finding everywhere findings are counted: the `pg-migration-lint: N finding(s)` summary, the `github-review` severity table and `findings-count` output. `--fail-on` compares each finding's severity with the threshold, and a group keeps the severity of its members, so grouping never changes the exit code.
- **Reporters**: SARIF (§7.2), SonarQube (§7.1) and text (§7.3) as described above. `github-review` posts one inline comment for the group, anchored at its lowest-line location that lies in the pull request's diff. The comment shows that location's message and names the other lines, earlier ones included, in a second paragraph. In a modified file the group's lowest line can be old code outside the diff while a later member is new, so only a group with no location in the diff is listed in the summary comment ("outside this pull request's diff"), with the other lines named after its message.
- **Single-file changelogs**: Code Scanning and SonarQube track an alert or issue by its primary location. When one file keeps growing (a single-file Liquibase changelog, or a long-lived file edited in place), a new violation of a rule that already has a group in that file joins the existing group instead of showing up as a new alert or issue. Such layouts may want `group_threshold = 0`.

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

## 11. Project Structure

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
│   │   ├── mod.rs           # Rule trait, dispatch
│   │   ├── pgm001.rs        # One file per rule
│   │   ├── pgm002.rs
│   │   ├── ...
│   │   └── docs/            # One markdown file per rule: module doc, --explain body, docs/rules.md body
│   │       ├── pgm001.md
│   │       └── ...
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

- **Upgrade SonarQube output to 10.3+ Generic Issue Import format.** The current reporter emits the deprecated pre-10.3 format where each issue carries `engineId`, `ruleId`, `severity`, and `type`. The 10.3+ format moves rule metadata to a top-level `rules` array (with `cleanCodeAttribute`, `type`, `impacts`) and slims issues down to `ruleId` + `primaryLocation`. Upgrading gives proper control over clean-code attributes and software-quality impacts in the SonarQube UI. Requires injecting `RuleInfo` into `SonarQubeReporter` at construction time. See `PLAN_SONARQUBE_UPGRADE.md` for full implementation plan.
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
| 1.12    | 2026-02-17 | Renumbered PGM024 (missing IF EXISTS) → PGM008 (slot freed by PGM008 → PGM901 rename). Updated PGM901 scope to PGM001–PGM023. |
| 1.13    | 2026-02-18 | Promoted PGM1403 → PGM403 (CREATE TABLE IF NOT EXISTS for already-existing table, MINOR). No IR or catalog changes required. |
| 1.14    | 2026-02-18 | Added PGM3xx DML-in-migrations category: PGM301 (INSERT INTO, INFO), PGM302 (UPDATE, MINOR), PGM303 (DELETE FROM, MINOR). Added PGM506 (CREATE UNLOGGED TABLE, INFO). IR changes: replaced `temporary: bool` on `CreateTable` with `TablePersistence` enum (Permanent/Unlogged/Temporary); added `InsertInto`, `UpdateTable`, `DeleteFrom` IR nodes. |
| 1.15    | 2026-02-25 | Spec sync with implementation. Added PGM004 (DETACH PARTITION without CONCURRENTLY, CRITICAL) and PGM005 (ATTACH PARTITION without CHECK, CRITICAL). Added partition support: `AlterIndexAttachPartition` IR node, `only` field on `IndexState`, partition-aware behavior for PGM002 and PGM501. Renumbered old PGM016–PGM020 (v1.5) to their current IDs: PGM013 (SET NOT NULL), PGM014 (ADD FK NOT VALID), PGM015 (ADD CHECK NOT VALID), PGM504 (RENAME TABLE), PGM505 (RENAME COLUMN). Added PGM018 (CLUSTER on existing table, CRITICAL) and PGM019 (ADD EXCLUDE constraint, CRITICAL) in freed 0xx slots. Updated IR table with all implemented nodes (Cluster, RenameTable, RenameColumn, AlterIndexAttachPartition) and fields (partition_by, partition_of on CreateTable; only on CreateIndex). Updated `TableConstraint` to reflect `not_valid`, `using_index`, `name` fields. Updated `ColumnDef` to reflect `is_inline_pk`, `is_serial` fields. Updated Catalog/TableState/IndexState structs with partition fields. Removed all stale "Status: Implemented/Not yet implemented" markers and "IR impact" notes — all described rules and IR changes are now implemented. |
| 1.16    | 2026-02-27 | Added PGM020 (DISABLE TRIGGER on table, MINOR/INFO). Added PGM205 (DROP SCHEMA CASCADE, CRITICAL). New `DropSchema` IR node mapped from `DropStmt(OBJECT_SCHEMA)`. Catalog replay removes all tables with matching schema prefix on CASCADE. Rule always fires on CASCADE regardless of catalog state — lists known affected tables for context. |
| 1.17    | 2026-03-02 | Added PGM021 (VACUUM FULL on existing table, CRITICAL) and PGM022 (REINDEX without CONCURRENTLY, CRITICAL). New IR nodes: `VacuumFull`, `Reindex`. |
| 1.18    | 2026-03-02 | Spec sync with implementation. Added PGM107 (integer PK, MAJOR), PGM108 (prefer text over varchar(n), INFO), PGM109 (floating-point type, MINOR) — promoted from deferred "Don't Do This" rules. Added PGM507 (DROP NOT NULL, INFO), PGM508 (duplicate/redundant index, INFO), PGM509 (mixed-case identifiers or reserved words, INFO). Updated PGM016 definition to match implementation (USING INDEX focused, with nullable-column and non-btree checks). Updated IR table: `CreateIndex` gains `index_name`, `if_not_exists`, `where_clause`, `access_method`; `DropIndex` gains `if_exists`; `DropTable` gains `if_exists`, `cascade`. Added `access_method` to `IndexState`. Added missing `AlterTableAction` variants: `DropNotNull`, `SetDefault`, `DropDefault`, `DropConstraint`, `ValidateConstraint`. Removed stale XML fallback reference from pipeline diagram. Removed "(Proposed)" from §11 heading. Total: 52 rules. |
| 1.19    | 2026-09-10 | Added PGM024 (DROP TABLE or CREATE TABLE PARTITION OF locking a pre-existing partition parent, CRITICAL). No IR or catalog changes required — reads the existing `parent_table`/`partition_of` fields. Added `LintContext::parent_display_name` helper. |
| 1.20    | 2026-09-17 | Rule text has a single source: `src/rules/docs/pgmXXX.md` is the module doc, the `--explain` body (printed verbatim as CommonMark) and the `docs/rules.md` body. Rule sections in §4.2/§4.3 now define behaviour only: removed every per-rule **Why** bullet, aligned 29 headings with `DESCRIPTION`, and aligned Severity bullets with `DEFAULT_SEVERITY` (7 stale `WARNING` to `MINOR`, PGM005 `CRITICAL` to `MAJOR`, PGM006 leads with `MINOR`). A `docgen`-gated test enforces heading, severity and the absence of **Why** bullets. |
| 1.21    | 2026-09-18 | Added PGM025 (DROP COLUMN silently removes EXCLUDE constraint, MINOR). Same shape as PGM010/011/012: shares `drop_column_check::check_drop_column_constraints`, matches `ConstraintState::Exclude`. No IR or catalog changes required — `ConstraintState::Exclude` and its `involves_column`/`remove_column` handling already existed. |
| 1.22    | 2026-11-17 | Added PGM027 (statement fails against the schema from earlier migrations, MAJOR): `USING INDEX` of another table's index, renames onto a taken name, and `DROP INDEX` of a constraint's index. Each statement is judged against the catalog just before it (`catalog_before` with the unit's earlier statements replayed, `replay::map_with_state_before`). PGM504 skips a rename PGM027 reports; PGM016/017 skip, and PGM107 ignores, a `USING INDEX` of another table's index. Replay keeps the index on a rejected `DROP INDEX`. |
| 1.23    | 2026-11-18 | Grouped findings (§7.4): once one rule reports `output.group_threshold` (default 5) findings at one severity in one file, they are reported as one finding with `additional_locations`, after suppression and per-unit dedup. SARIF renders a group as one result with every finding in `locations` (split per 1,000 locations, GitHub's limit), SonarQube as one issue with `secondaryLocations`, text and `github-review` as one finding naming each other line once; `github-review` anchors a group at its first location in the diff. §7.1 example updated to the 10.3+ format the reporter emits. |
| 1.24    | 2026-11-18 | Added PGM026 (one transaction locks several partitioned parent tables, CRITICAL): counts the distinct pre-existing parents that `DROP TABLE` of a partition and `CREATE TABLE ... PARTITION OF` lock in each transaction window (`LintContext::transaction_windows`), and fires once per window with two or more, at the first statement that locks one. Shares `partition_parent_lock::ParentLocks` with PGM024, which flags the same statements. No IR or catalog changes required. |
| 1.25    | 2026-11-18 | PGM026 also counts the parent a plain `ALTER TABLE ... DETACH PARTITION` locks (not `CONCURRENTLY`; the parent must be partitioned in `catalog_before`). `ParentLockingStatement` gains `PlainDetachPartition`, which PGM024 skips, as PGM004 reports it. The PGM026 lock claims are now measured on PostgreSQL 14 to 19 in `verify/lock-tests/`: `DROP TABLE a, b` holds `a`'s parent while it waits for `b`'s, plain `DETACH` holds its parent's ACCESS EXCLUSIVE until the transaction ends, and `DETACH ... CONCURRENTLY` takes only SHARE UPDATE EXCLUSIVE on the parent and cannot run inside a transaction block. |
