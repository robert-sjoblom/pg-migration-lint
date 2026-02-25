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
| `CreateTable { name, columns, constraints, persistence, partition_by, partition_of }` | `CreateStmt` |
| `AlterTable { name, actions[] }` | `AlterTableStmt` (objtype = ObjectTable) |
| `CreateIndex { table, columns, unique, concurrent, only }` | `IndexStmt` |
| `DropIndex { name, concurrent }` | `DropStmt(OBJECT_INDEX)` |
| `DropTable { name }` | `DropStmt(OBJECT_TABLE)` |
| `AlterIndexAttachPartition { parent_index_name, child_index_name }` | `AlterTableStmt` (objtype = ObjectIndex, AT_AttachPartition) |
| `RenameTable { name, new_name }` | `RenameStmt` (ObjectTable) |
| `RenameColumn { table, old_name, new_name }` | `RenameStmt` (ObjectColumn) |
| `Cluster { table, index }` | `ClusterStmt` |
| `InsertInto { table_name }` | `InsertStmt` |
| `UpdateTable { table_name }` | `UpdateStmt` |
| `DeleteFrom { table_name }` | `DeleteStmt` |
| `TruncateTable { table_name, cascade }` | `TruncateStmt` |

`AlterTableAction` variants: `AddColumn`, `DropColumn`, `AddConstraint`, `AlterColumnType`, `SetNotNull`, `AttachPartition`, `DetachPartition`, `Other`.

**Constraint normalization**: Postgres supports both inline (`CREATE TABLE foo (baz int PRIMARY KEY)`) and table-level (`CREATE TABLE foo (baz int, PRIMARY KEY (baz))`) syntax for PK, FK, and UNIQUE constraints. These land in different places in the `pg_query` AST (`ColumnDef.constraints` vs `CreateStmt.tableElts`). The IR preserves the distinction (`ColumnDef.is_inline_pk` vs `TableConstraint::PrimaryKey`), but the Catalog must normalize both into identical `TableState`. Rules never deal with the syntactic variant — only catalog state.

**`serial`/`bigserial` expansion**: Postgres's parser expands `serial` into `integer` + `CREATE SEQUENCE` + `DEFAULT nextval(...)`. The IR sees the expanded form. This means PGM006 may fire on `nextval()` as an unknown function call (INFO level). This is technically correct but noisy for a well-known idiom. The v1 approach: add `nextval` to the known volatile function list with a tailored message: `Column '{col}' uses a sequence default (serial/bigserial). This is standard — suppress this finding if intentional.`

`ColumnDef` carries: `name`, `type_name`, `nullable`, `default_expr`, `is_inline_pk`, `is_serial`.

`TableConstraint` variants: `PrimaryKey { columns, using_index }`, `ForeignKey { name, columns, ref_table, ref_columns, not_valid }`, `Unique { name, columns, using_index }`, `Check { name, expression, not_valid }`.

`Unparseable` nodes are preserved in the stream so the replay engine can mark catalog gaps.

### 3.3 Table Catalog

Built by replaying all migration files in configured order. Represents the schema state at each point in the migration history.

```
Catalog {
    tables: HashMap<String, TableState>,
    index_to_table: HashMap<String, String>,       // reverse lookup: index name → table key
    partition_children: HashMap<String, Vec<String>>, // parent key → child keys
}

TableState {
    name: String,
    columns: Vec<ColumnState>,       // name, type, nullable, default
    indexes: Vec<IndexState>,        // name, entries (ordered), unique, where_clause, only
    constraints: Vec<ConstraintState>,  // PK, FK, unique, check
    has_primary_key: bool,
    incomplete: bool,                // true if any unparseable statement touched this table
    is_partitioned: bool,            // true if PARTITION BY was used
    partition_by: Option<PartitionByInfo>,  // strategy + columns
    parent_table: Option<String>,    // catalog key of parent (if PARTITION OF)
}

IndexState {
    name: String,
    entries: Vec<IndexEntry>,        // Column(name) or Expression { text, referenced_columns }
    unique: bool,
    where_clause: Option<String>,    // partial index WHERE clause
    only: bool,                      // CREATE INDEX ON ONLY (parent stub, not recursive)
}
```

- `CREATE TABLE` → insert into catalog; if `PARTITION OF`, record parent relationship
- `DROP TABLE` → remove from catalog entirely; CASCADE recursively removes partition children
- `ALTER TABLE` → mutate existing entry; `ATTACH PARTITION` / `DETACH PARTITION` update parent-child tracking
- `CREATE INDEX` → add to table's index list (preserving `only` flag)
- `ALTER INDEX ATTACH PARTITION` → flip parent index's `only` from `true` to `false`
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

- **PGM0xx**: Unsafe DDL rules — locking, rewrites, runtime failures, silent side effects
- **PGM1xx**: Type anti-pattern rules ("Don't Do This")
- **PGM2xx**: Destructive operation rules — data loss
- **PGM3xx**: DML in migrations rules — INSERT, UPDATE, DELETE on existing tables
- **PGM4xx**: Idempotency guard rules
- **PGM5xx**: Schema design & informational rules
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
- **Message (non-partitioned)**: `DROP INDEX '{index}' on existing table '{table}' should use CONCURRENTLY to avoid holding an exclusive lock.`
- **Message (partitioned parent)**: `DROP INDEX '{index}' on partitioned table '{table}' will lock all partitions. CONCURRENTLY is not supported for partitioned parent indexes.`
- **Partition behavior**:
  - **ON ONLY index on partitioned parent**: Suppressed — dropping an invalid parent-only stub is safe, no child locks taken.
  - **Recursive/attached index on partitioned parent**: Emits the partition-specific message. PostgreSQL does NOT support `DROP INDEX CONCURRENTLY` on partitioned parent indexes; the standard advice does not apply.
  - **Non-partitioned table**: Standard CONCURRENTLY advice.
  - `ALTER INDEX ... ATTACH PARTITION` flips an ON ONLY index to recursive, so the suppression vs. warning is correctly distinguished even across migrations.

#### PGM003 — `CONCURRENTLY` inside transaction

- **Severity**: CRITICAL
- **Triggers**: `CREATE INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY` inside a context that implies transactional execution:
  - Liquibase changeset without `runInTransaction="false"`
  - go-migrate (which runs each file in a transaction by default, unless the file contains `-- +goose NO TRANSACTION` or equivalent)
- **Message**: `CONCURRENTLY cannot run inside a transaction. Set runInTransaction="false" (Liquibase) or disable transactions for this migration.`

#### PGM004 — `DETACH PARTITION` without `CONCURRENTLY`

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... DETACH PARTITION child` without `CONCURRENTLY`, where the parent table exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Plain `DETACH PARTITION` acquires ACCESS EXCLUSIVE on both the parent partitioned table and the child partition for the full duration. This blocks all reads and writes on the parent (and all its partitions) until detach completes.
- **Safe alternative**: Use `DETACH PARTITION ... CONCURRENTLY` (PostgreSQL 14+), which uses SHARE UPDATE EXCLUSIVE instead, allowing concurrent reads and writes.
- **Does not fire when**:
  - `CONCURRENTLY` is present
  - The parent table is created in the same set of changed files
  - The parent table does not exist in `catalog_before`
- **Message**: `DETACH PARTITION on existing partitioned table '{table}' without CONCURRENTLY acquires ACCESS EXCLUSIVE on the entire table, blocking all reads and writes. Use DETACH PARTITION ... CONCURRENTLY (PostgreSQL 14+).`

#### PGM005 — `ATTACH PARTITION` without pre-validated CHECK

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...` where the child table exists in `catalog_before`, is not created in the same set of changed files, and has no CHECK constraint in the catalog.
- **Why**: When attaching a partition, PostgreSQL must verify that every existing row in the child satisfies the partition bound. Without a pre-validated CHECK constraint whose expression implies the bound, PostgreSQL performs a full table scan under ACCESS EXCLUSIVE lock on the child table.
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

- **Severity**: WARNING for known volatile functions (`now()`, `current_timestamp`, `random()`, `gen_random_uuid()`, `uuid_generate_v4()`, `clock_timestamp()`, `timeofday()`, `txid_current()`, `nextval()`). INFO for any other function call used as a default.
- **Triggers**: `ADD COLUMN ... DEFAULT fn()` or inline in `CREATE TABLE`.
- **Note**: On Postgres 11+, non-volatile defaults on `ADD COLUMN` don't rewrite the table. Volatile defaults always evaluate per-row at write time, which is typically intentional — but worth flagging because developers sometimes use `now()` expecting a fixed value.
- **Message (known volatile)**: `Column '{col}' on '{table}' uses volatile default '{fn}()'. Unlike non-volatile defaults, this forces a full table rewrite under an ACCESS EXCLUSIVE lock — every existing row must be physically updated with a computed value. For large tables, this causes extended downtime. Consider adding the column without a default, then backfilling with batched UPDATEs.`
- **Message (nextval/serial)**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). This is standard usage — suppress if intentional. Note: on ADD COLUMN to an existing table, this is volatile and forces a table rewrite.`
- **Message (unknown function)**: `Column '{col}' on '{table}' uses function '{fn}()' as default. If this function is volatile (the default for user-defined functions), it forces a full table rewrite under an ACCESS EXCLUSIVE lock instead of a cheap catalog-only change. Verify the function's volatility classification.`

#### PGM007 — `ALTER COLUMN TYPE` on existing table

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

#### PGM008 — `ADD COLUMN NOT NULL` without default on existing table

- **Severity**: CRITICAL
- **Triggers**: `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a `DEFAULT` clause, where the table exists in the catalog.
- **Note**: On PG 11+, `ADD COLUMN ... NOT NULL DEFAULT <value>` is safe (no rewrite for non-volatile defaults). Without a default, the command fails outright if any rows exist. This is almost always a bug.
- **Message**: `Adding NOT NULL column '{col}' to existing table '{table}' without a DEFAULT will fail if the table has any rows. Add a DEFAULT value, or add the column as nullable and backfill.`

#### PGM009 — `DROP COLUMN` on existing table

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... DROP COLUMN` where the table exists in the catalog.
- **Note**: Postgres marks the column as dropped without rewriting the table, so this is cheap at the database level. The risk is application-level: queries referencing the column will break. This is informational to increase visibility.
- **Message**: `Dropping column '{col}' from existing table '{table}'. The DDL is cheap but ensure no application code references this column.`

#### PGM016 — `ADD PRIMARY KEY` on existing table without prior `UNIQUE` constraint

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

#### PGM010 — `DROP COLUMN` silently removes unique constraint

- **Severity**: WARNING
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in a `UNIQUE` constraint or unique index on the table in `catalog_before`.
- **Why**: PostgreSQL automatically drops any index or constraint that depends on the column. If the column was part of a unique constraint or unique index, the uniqueness guarantee is silently lost. This can lead to duplicate rows being inserted where they were previously impossible.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `Unique` or any `IndexState` where `is_unique` is true. If so, fire.
- **Does not fire when**:
  - The column is not part of any unique constraint or unique index
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes unique constraint '{constraint}'. Verify that the uniqueness guarantee is no longer needed.`

#### PGM011 — `DROP COLUMN` silently removes primary key

- **Severity**: MAJOR
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in the table's primary key (in `catalog_before`).
- **Why**: Dropping a PK column (with `CASCADE`) silently removes the primary key constraint. The table loses its row identity, which affects replication, ORMs, query planning, and data integrity. PGM502 catches tables *created* without a PK, but cannot tell you which specific `DROP COLUMN` *caused* the loss.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `PrimaryKey`. If so, fire.
- **Does not fire when**:
  - The column is not part of the primary key
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes the primary key. The table will have no row identity. Add a new primary key or reconsider the column drop.`

#### PGM012 — `DROP COLUMN` silently removes foreign key

- **Severity**: WARNING
- **Triggers**: `ALTER TABLE ... DROP COLUMN col` where `col` participates in a `FOREIGN KEY` constraint on the table in `catalog_before`.
- **Why**: Dropping a column that is part of a foreign key (with `CASCADE`) silently removes the FK constraint. The referential integrity guarantee is lost — the table can now hold values with no corresponding row in the referenced table.
- **Logic**: On `AlterTableAction::DropColumn`, look up the table in `catalog_before`. Check if the dropped column appears in any `ConstraintState` of kind `ForeignKey`. If so, fire.
- **Does not fire when**:
  - The column is not part of any foreign key constraint
  - The table does not exist in `catalog_before`
- **Message**: `Dropping column '{col}' from table '{table}' silently removes foreign key '{constraint}' referencing '{ref_table}'. Verify that the referential integrity guarantee is no longer needed.`

#### PGM013 — `SET NOT NULL` on existing column

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

#### PGM014 — `ADD FOREIGN KEY` without `NOT VALID` on existing table

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
- **Interaction with PGM501**: PGM501 (missing FK index) fires independently. The rules are complementary.
- **Message**: `Adding foreign key '{constraint}' on existing table '{table}' validates all rows, blocking writes. Use NOT VALID and validate in a separate migration.`

#### PGM015 — `ADD CHECK` without `NOT VALID` on existing table

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

#### PGM017 — `ADD UNIQUE` on existing table without `USING INDEX`

- **Severity**: CRITICAL
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

#### PGM018 — `CLUSTER` on existing table

- **Severity**: CRITICAL
- **Triggers**: `CLUSTER table_name [USING index_name]` where the table exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `CLUSTER` rewrites the entire table and all its indexes in a new physical order, holding an `ACCESS EXCLUSIVE` lock for the full duration of the rewrite. Unlike `VACUUM FULL`, there is no online alternative. On large tables this causes complete unavailability (all reads and writes blocked) for the duration — typically minutes to hours. It is almost never appropriate in an online migration.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `CLUSTER on table '{table}' [USING '{index}'] rewrites the entire table under ACCESS EXCLUSIVE lock for the full duration. All reads and writes are blocked. This is rarely appropriate in an online migration.`

#### PGM201 — `DROP TABLE` on existing table

- **Severity**: MINOR
- **Triggers**: `DROP TABLE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Dropping a table is intentional but destructive and irreversible in production. The DDL itself is instant — PostgreSQL does not scan the table or hold an extended lock — so this is not a downtime risk. However, all data in the table is permanently lost, and any queries, views, foreign keys, or application code referencing the table will break.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `DROP TABLE '{table}' removes an existing table. This is irreversible and all data will be lost.`

#### PGM202 — `DROP TABLE CASCADE` on existing table

- **Severity**: MAJOR
- **Triggers**: `DROP TABLE ... CASCADE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `CASCADE` silently drops all dependent objects — foreign keys, views, triggers, and rules that reference the dropped table. Unlike a plain `DROP TABLE` (which fails if dependencies exist), `CASCADE` succeeds silently, potentially breaking other tables and application code without any warning at migration time.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - `DROP TABLE` without `CASCADE` (handled by PGM201)
- **Message (no known FK deps)**: `DROP TABLE CASCADE on '{table}' will silently drop all dependent objects (views, foreign keys, triggers). Review dependencies before proceeding.`
- **Message (with FK deps)**: `DROP TABLE CASCADE on '{table}' will silently drop dependent objects. Known FK dependencies from: {dep_tables}.`

#### PGM203 — `TRUNCATE TABLE` on existing table

- **Severity**: MINOR
- **Triggers**: `TRUNCATE TABLE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `TRUNCATE` is instant DDL that does not scan rows and does not fire row-level `ON DELETE` triggers. All data in the table is permanently destroyed.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
- **Message**: `TRUNCATE TABLE '{table}' removes all rows from an existing table. This is irreversible and does not fire ON DELETE triggers.`

#### PGM204 — `TRUNCATE TABLE ... CASCADE` on existing table

- **Severity**: MAJOR
- **Triggers**: `TRUNCATE TABLE ... CASCADE` where the target table exists in `catalog_before` (not created in the same set of changed files).
- **Why**: `TRUNCATE CASCADE` automatically extends the truncate to all tables with FK references to the target table, recursively. The developer may not be aware of the full cascade chain.
- **Does not fire when**:
  - Table is new (in `tables_created_in_change`)
  - Table doesn't exist in `catalog_before`
  - `TRUNCATE` without `CASCADE` (handled by PGM203)
- **Message (no known FK deps)**: `TRUNCATE TABLE '{table}' CASCADE silently extends to all tables with foreign key references to '{table}', and recursively to their dependents. Verify the full cascade chain is intentionally truncated.`
- **Message (with FK deps)**: `TRUNCATE TABLE '{table}' CASCADE silently extends to all tables with foreign key references to '{table}', and recursively to their dependents. Known FK dependencies from: {dep_tables}.`

#### PGM301 — `INSERT INTO` existing table in migration

- **Severity**: INFO
- **Triggers**: `INSERT INTO` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Inserting into an existing table in a migration is often intentional seed or reference data, but bulk `INSERT ... SELECT` or large `VALUES` lists hold row locks for the full statement duration and can cause replication lag. The rule fires informational to prompt the author to confirm row volume is bounded.
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `INSERT INTO existing table '{table}' in a migration. Ensure this is intentional seed data and that row volume is bounded.`

#### PGM302 — `UPDATE` on existing table in migration

- **Severity**: MINOR
- **Triggers**: `UPDATE` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Unbatched `UPDATE` in a migration holds row-level locks on every matched row for the full statement duration. On large tables this blocks concurrent reads and writes, causes replication lag, and can cascade into lock queues.
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `UPDATE on existing table '{table}' in a migration. Unbatched updates hold row locks for the full statement duration. Verify row volume and consider batched execution.`

#### PGM303 — `DELETE FROM` existing table in migration

- **Severity**: MINOR
- **Triggers**: `DELETE FROM` targeting a table that exists in `catalog_before` (not created in the same set of changed files).
- **Why**: Unbatched `DELETE` in a migration holds row-level locks on every matched row for the full statement duration. On large tables this blocks concurrent writes, generates significant WAL, and causes replication lag.
- **Does not fire when**:
  - The target table is created in the same set of changed files.
  - The table does not exist in `catalog_before`.
- **Message**: `DELETE FROM existing table '{table}' in a migration. Unbatched deletes hold row locks and generate significant WAL. Verify row volume and consider batched execution.`

#### PGM402 — Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`

- **Severity**: MINOR
- **Triggers**: `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause.
- **Why**: Without `IF NOT EXISTS`, the statement fails if the object already exists. In migration pipelines that may be re-run (e.g., idempotent migrations, manual re-execution after partial failure), this causes hard failures. Adding `IF NOT EXISTS` makes the statement idempotent.
- **Does not fire when**:
  - The statement already includes `IF NOT EXISTS`
- **Message (CREATE TABLE)**: `CREATE TABLE '{table}' without IF NOT EXISTS will fail if the table already exists.`
- **Message (CREATE INDEX)**: `CREATE INDEX '{index}' without IF NOT EXISTS will fail if the index already exists.`

#### PGM403 — `CREATE TABLE IF NOT EXISTS` for already-existing table

- **Severity**: MINOR
- **Triggers**: `CREATE TABLE IF NOT EXISTS` targeting a table that already exists in `catalog_before` at that point in the migration history.
- **Why**: `IF NOT EXISTS` makes the statement a silent no-op when the table already exists. If the column definitions in the `CREATE TABLE` differ from the actual table state (built up from the original `CREATE TABLE` plus subsequent `ALTER TABLE` statements), the migration author may believe the table has the shape described in the statement, when in reality PostgreSQL ignores it entirely. The migration chain is ambiguous — two competing definitions of the same table exist in the history, and only the first one (plus its alterations) is truth. This is especially common with Liquibase 4.26+, which supports `ifNotExists="true"` on `<createTable>`.
- **Does not fire when**:
  - The table does not already exist in the catalog (the statement genuinely creates it).
  - `IF NOT EXISTS` is absent (a duplicate `CREATE TABLE` without the guard would fail at runtime, which is a different problem).
- **Message**: `CREATE TABLE IF NOT EXISTS '{table}' is a no-op — the table already exists in the migration history. The definition in this statement is silently ignored by PostgreSQL. If the column definitions differ from the actual table state, this migration is misleading.`

#### PGM401 — Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`

- **Severity**: MINOR
- **Triggers**: `DROP TABLE` or `DROP INDEX` without the `IF EXISTS` clause.
- **Why**: Without `IF EXISTS`, the statement fails if the object does not exist. In migration pipelines that may be re-run, this causes hard failures. Adding `IF EXISTS` makes the statement idempotent.
- **Does not fire when**:
  - The statement already includes `IF EXISTS`
- **Message (DROP TABLE)**: `DROP TABLE '{table}' without IF EXISTS will fail if the table does not exist.`
- **Message (DROP INDEX)**: `DROP INDEX '{index}' without IF EXISTS will fail if the index does not exist.`

#### PGM501 — Foreign key without index on referencing columns

- **Severity**: MAJOR
- **Triggers**: `ADD CONSTRAINT ... FOREIGN KEY (cols) REFERENCES ...` where no index exists on the referencing table with `cols` as a prefix of the index columns.
- **Prefix matching**: FK columns `(a, b)` are covered by index `(a, b)` or `(a, b, c)` but NOT by `(b, a)` or `(a)`. Column order matters.
- **Catalog lookup**: checks indexes on the referencing table after the full file/changeset is processed (not at the point of FK creation). This avoids false positives when the index is created later in the same file/changeset.
- **Index exclusions**: Partial indexes (with WHERE clause) and ON ONLY indexes (`only: true`) are excluded from coverage checks — partial indexes only cover a subset of rows, and ON ONLY indexes are invalid parent stubs that don't provide real FK coverage.
- **Partition behavior**:
  - **Partitioned parent tables**: Checks `has_covering_index` normally. A recursive index (not ON ONLY) satisfies coverage. An ON ONLY index does not.
  - **Partition children**: Checks the child's own indexes first. If none found, delegates to the parent table's indexes via `parent_table`. If the parent is not in the catalog, suppresses conservatively (common in incremental CI where the parent was created outside tracked migrations).
  - `ALTER INDEX ... ATTACH PARTITION` flips `only` to `false`, so after all children are attached, the parent index correctly satisfies FK coverage.
- **Message**: `Foreign key on '{table}({cols})' has no covering index. Sequential scans on the referencing table during deletes/updates on the referenced table will cause performance issues.`

#### PGM502 — Table without primary key

- **Severity**: MAJOR
- **Triggers**: `CREATE TABLE` (non-temporary) with no `PRIMARY KEY` constraint, checked after the full file/changeset is processed (to allow `ALTER TABLE ... ADD PRIMARY KEY` later in the same file).
- **Message**: `Table '{table}' has no primary key.`

#### PGM503 — `UNIQUE NOT NULL` used instead of primary key

- **Severity**: INFO
- **Triggers**: Table has no PK but has at least one `UNIQUE` constraint where all constituent columns are `NOT NULL`.
- **Message**: `Table '{table}' uses UNIQUE NOT NULL instead of PRIMARY KEY. Functionally equivalent but PRIMARY KEY is conventional and more explicit.`

#### PGM504 — `RENAME TABLE`

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... RENAME TO ...` on a table that exists in `catalog_before`.
- **Why**: Renames are instant DDL (metadata-only), but silently break any application queries, views, functions, or triggers that reference the old name.
- **Replacement detection**: Does **not** fire if, within the same migration unit, a `CREATE TABLE` with the old name appears after the rename. This is a common pattern (rename old table away, create replacement with the original name).
- **Does not fire when**:
  - The table does not exist in `catalog_before`
  - A replacement table with the old name is created in the same migration unit
- **Message**: `Renaming table '{old_name}' to '{new_name}'. Ensure all application queries, views, and functions referencing the old name are updated.`

#### PGM505 — `RENAME COLUMN`

- **Severity**: INFO
- **Triggers**: `ALTER TABLE ... RENAME COLUMN ... TO ...` on a table that exists in `catalog_before`.
- **Why**: Column renames are instant DDL but silently break application queries that reference the old column name.
- **Does not fire when**:
  - The table does not exist in `catalog_before`
- **Message**: `Renaming column '{old_name}' to '{new_name}' on table '{table}'. Ensure all application queries, views, and functions referencing the old column name are updated.`

#### PGM506 — `CREATE UNLOGGED TABLE`

- **Severity**: INFO
- **Triggers**: `CREATE TABLE ... UNLOGGED` for any table.
- **Why**: Unlogged tables are not written to the WAL. This means: (1) all data is truncated on crash recovery, (2) they are not streamed to standby replicas via streaming replication, and (3) they are excluded from logical replication slots. In most production environments, unlogged tables are unsuitable for data that needs to survive a crash or be replicated.
- **Does not fire when**:
  - The `UNLOGGED` keyword is absent (permanent or temporary tables).
- **Message**: `CREATE UNLOGGED TABLE '{table}'. Unlogged tables are truncated on crash recovery and not replicated to standbys. Confirm this is intentional.`

#### PGM901 — Down migration severity cap

- **All down-migration findings are capped at INFO severity**, regardless of what the rule would normally produce.
- The same rules run on down migrations, but findings are informational only.
- PGM901 is a meta-behavior, not a standalone lint rule. It has no `Rule` trait implementation and cannot be suppressed or disabled via inline comments. The 9xx range is reserved for meta-behaviors that modify how other rules operate.
- **Scope**: Down migration detection relies on filename patterns (`.down.sql` / `_down.sql` suffixes). Liquibase `<rollback>` blocks are not currently detected as down migrations (see §2.2).

### 4.3 Type Anti-pattern Rules (PGM1xx)

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
- **Interaction with PGM006**: Both PGM105 and PGM006 fire on `nextval()` defaults. This is intentional — PGM006 warns about the volatile default aspect, PGM105 recommends the identity column alternative.
- **Message**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). Prefer GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY for new tables (PostgreSQL 10+). Identity columns have better ownership semantics and are the SQL standard approach.`

#### PGM106 — Don't use `json` (prefer `jsonb`)

- **Severity**: WARNING
- **Triggers**: Column type with `TypeName.name == "json"` in `CREATE TABLE`, `ADD COLUMN`, or `ALTER COLUMN TYPE`.
- **Why**: The `json` type stores an exact copy of the input text and must re-parse it on every operation. `jsonb` stores a decomposed binary format that is significantly faster for queries, supports indexing (GIN), and supports containment/existence operators (`@>`, `?`, `?|`, `?&`). The only advantages of `json` are preserving exact key order and duplicate keys — both rarely needed.
- **Message**: `Column '{col}' on '{table}' uses 'json'. Use 'jsonb' instead — it's faster, smaller, indexable, and supports containment operators. Only use 'json' if you need to preserve exact text representation or key order.`

#### Don't use `integer` as primary key type (ID unassigned)

- **Severity**: MAJOR
- **Status**: Not yet implemented. Rule ID unassigned (PGM106 is now used by the json rule).
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

Multiple rules in one comment: `-- pgm-lint:suppress PGM001,PGM501`

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

MAJOR PGM501 db/migrations/V042__add_order_index.sql:7
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
| 1.15    | 2026-02-25 | Spec sync with implementation. Added PGM004 (DETACH PARTITION without CONCURRENTLY, CRITICAL) and PGM005 (ATTACH PARTITION without CHECK, CRITICAL). Added partition support: `AlterIndexAttachPartition` IR node, `only` field on `IndexState`, partition-aware behavior for PGM002 and PGM501. Updated IR table with all implemented nodes (Cluster, RenameTable, RenameColumn, AlterIndexAttachPartition) and fields (partition_by, partition_of on CreateTable; only on CreateIndex). Updated `TableConstraint` to reflect `not_valid`, `using_index`, `name` fields. Updated `ColumnDef` to reflect `is_inline_pk`, `is_serial` fields. Updated Catalog/TableState/IndexState structs with partition fields. Removed all stale "Status: Implemented/Not yet implemented" markers and "IR impact" notes — all described rules and IR changes are now implemented. |
