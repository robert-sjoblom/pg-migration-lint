# New Lint Rules from PostgreSQL "Don't Do This" Wiki

Source: [https://wiki.postgresql.org/wiki/Don%27t_Do_This](https://wiki.postgresql.org/wiki/Don%27t_Do_This)

This document specifies lint rules derived from the PostgreSQL "Don't Do This" wiki page.
Only DDL-detectable anti-patterns are included. Query-level patterns (e.g., "don't use
NOT IN with nullable columns", "don't use BETWEEN for timestamp ranges") are omitted
because they do not appear in migration files.

Rule IDs use the PGM1XX series to distinguish from core rules (PGM001-PGM011).

---

## Category 1: Column Type Anti-Patterns

These rules inspect `TypeName { name, modifiers }` on `ColumnDef` in `CreateTable` and
`AlterTableAction::AddColumn` / `AlterTableAction::AlterColumnType` IR nodes.

### PGM101 -- Don't use `timestamp` (without time zone)

- **Detects**: Column type `timestamp` or `timestamp without time zone` used in
  `CREATE TABLE`, `ALTER TABLE ... ADD COLUMN`, or `ALTER TABLE ... ALTER COLUMN TYPE`.
- **Why it's bad**: `timestamp without time zone` (spelled `timestamp` in SQL) stores a
  date-time with no time zone context. PostgreSQL does not convert it to or from any time
  zone. This means the stored value is ambiguous -- it could be UTC, local time, or
  anything else. Application bugs arise when the server, client, or session timezone
  changes, because the same timestamp value is interpreted differently. The `timestamptz`
  type stores an absolute point in time (internally as UTC) and converts on input/output
  based on the session `timezone` setting, making it unambiguous.
- **Severity**: WARNING
- **IR detectability**: Fully detectable. The `TypeName.name` field will be `"timestamp"`
  or `"timestamp without time zone"` (lowercased). Note: `pg_query` normalizes
  `timestamp without time zone` to `"timestamp"` in the AST. The rule should match on
  `TypeName.name` being exactly `"timestamp"`.
- **Example bad SQL**:
  ```sql
  CREATE TABLE events (
      id bigint PRIMARY KEY,
      created_at timestamp NOT NULL DEFAULT now()
  );

  ALTER TABLE events ADD COLUMN updated_at timestamp;
  ```
- **Example fix**:
  ```sql
  CREATE TABLE events (
      id bigint PRIMARY KEY,
      created_at timestamptz NOT NULL DEFAULT now()
  );

  ALTER TABLE events ADD COLUMN updated_at timestamptz;
  ```
- **Message**: `Column '{col}' on '{table}' uses 'timestamp without time zone'. Use 'timestamptz' (timestamp with time zone) instead to store unambiguous points in time.`

---

### PGM102 -- Don't use `timestamp(0)` or `timestamptz(0)`

- **Detects**: Column type `timestamp` or `timestamptz` with a precision modifier of `0`,
  i.e., `TypeName.modifiers == [0]`.
- **Why it's bad**: Setting fractional seconds precision to 0 causes PostgreSQL to
  *round* (not truncate) the value. An input of `23:59:59.9` becomes `00:00:00` of the
  *next day*, silently changing the date. This is almost never the intended behavior.
  If sub-second precision is not needed, store full precision and truncate on output.
- **Severity**: WARNING
- **IR detectability**: Fully detectable. Check `TypeName.name` is `"timestamp"` or
  `"timestamptz"` (or their long forms) and `TypeName.modifiers == [0]`.
- **Example bad SQL**:
  ```sql
  CREATE TABLE events (
      id bigint PRIMARY KEY,
      created_at timestamptz(0) NOT NULL DEFAULT now()
  );
  ```
- **Example fix**:
  ```sql
  CREATE TABLE events (
      id bigint PRIMARY KEY,
      created_at timestamptz NOT NULL DEFAULT now()
  );
  ```
- **Message**: `Column '{col}' on '{table}' uses '{type}(0)'. Precision 0 causes rounding, not truncation -- a value of '23:59:59.9' rounds to the next day. Use full precision and format on output instead.`

---

### PGM103 -- Don't use `char(n)` or `character(n)`

- **Detects**: Column type `char`, `character`, `char(n)`, or `character(n)`.
  Note: unqualified `char` is `char(1)` in PostgreSQL.
- **Why it's bad**: `char(n)` pads values with spaces to exactly `n` characters. This
  wastes storage (padded values are larger), causes surprising comparison behavior
  (trailing spaces are semantically significant in some contexts but ignored in others),
  and is *never* faster than `text` or `varchar` -- PostgreSQL stores them identically
  on disk (as varlena), just with the added overhead of pad/unpad operations. The SQL
  standard `char(n)` type is a relic with no performance benefit in PostgreSQL.
- **Severity**: WARNING
- **IR detectability**: Fully detectable. Match `TypeName.name == "bpchar"`. pg_query
  normalizes both `char(n)` and `character(n)` to the canonical name `"bpchar"`. The
  internal single-byte type `"char"` (with quotes in SQL) is a distinct pg_catalog type
  and will NOT match. Unqualified `char` / `character` get synthetic modifier `[1]`.
- **Example bad SQL**:
  ```sql
  CREATE TABLE countries (
      code char(2) PRIMARY KEY,
      name varchar(100) NOT NULL
  );
  ```
- **Example fix**:
  ```sql
  CREATE TABLE countries (
      code text PRIMARY KEY,
      name text NOT NULL
  );
  -- Or use varchar(2) if you want a length check, though a CHECK constraint
  -- is more explicit: CHECK (length(code) = 2)
  ```
- **Message**: `Column '{col}' on '{table}' uses 'char({n})'. The char(n) type pads with spaces, wastes storage, and is no faster than text or varchar in PostgreSQL. Use text or varchar instead.`

---

### PGM104 -- Don't use the `money` type

- **Detects**: Column type `money`.
- **Why it's bad**: The `money` type has a fixed fractional precision determined by the
  database's `lc_monetary` locale setting. Changing `lc_monetary` silently reinterprets
  stored values with different precision or formatting. It does not store a currency
  code, so multi-currency support is impossible. Rounding behavior is locale-dependent.
  Input/output depends on locale settings, making dumps and restores between systems
  with different locales dangerous. Use `numeric` (or `decimal`) for monetary values.
- **Severity**: WARNING
- **IR detectability**: Fully detectable. Match `TypeName.name == "money"`.
- **Example bad SQL**:
  ```sql
  CREATE TABLE invoices (
      id bigint PRIMARY KEY,
      total money NOT NULL
  );
  ```
- **Example fix**:
  ```sql
  CREATE TABLE invoices (
      id bigint PRIMARY KEY,
      total numeric(12,2) NOT NULL
  );
  ```
- **Message**: `Column '{col}' on '{table}' uses the 'money' type. The money type depends on the lc_monetary locale setting, making it unreliable across environments. Use numeric(p,s) instead.`

---

### PGM105 -- Don't use `serial` / `bigserial`

- **Detects**: Column type `serial`, `bigserial`, `serial4`, `serial8`, or `smallserial`
  (`serial2`).
- **Why it's bad**: The `serial` pseudo-types create an implicit sequence and set a
  `DEFAULT nextval(...)` expression. This has several problems: (1) the sequence is not
  tied to the column via `pg_depend` in the same way as identity columns, so `DROP TABLE`
  may not cascade to the sequence; (2) grants/ownership on the sequence are separate;
  (3) `INSERT` with an explicit value does not advance the sequence, causing future
  conflicts; (4) identity columns (SQL standard since SQL:2003, supported since PG 10)
  are the modern replacement and handle all these edge cases correctly.
- **Severity**: INFO
- **IR detectability**: Partially detectable. PostgreSQL's parser expands `serial` into
  `integer` + sequence + `DEFAULT nextval(...)` before `pg_query` sees it, so the IR
  will contain the expanded form, not the literal `serial` keyword. However, the
  presence of a `DefaultExpr::FunctionCall { name: "nextval", .. }` on an
  `integer`/`bigint`/`smallint` column is a strong heuristic signal. The rule can flag
  `nextval()` defaults with a suggestion to use identity columns instead. This overlaps
  with PGM007 (volatile default) but has a different message and rationale.
  Alternatively, if the parser is enhanced to detect `serial` before expansion (by
  inspecting the raw SQL or the `pg_query` AST for `is_serial`), this becomes fully
  detectable.
- **Example bad SQL**:
  ```sql
  CREATE TABLE orders (
      id serial PRIMARY KEY,
      amount numeric(10,2)
  );
  ```
- **Example fix**:
  ```sql
  CREATE TABLE orders (
      id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
      amount numeric(10,2)
  );
  -- Or GENERATED BY DEFAULT AS IDENTITY if you need to specify values explicitly.
  ```
- **Message**: `Column '{col}' on '{table}' uses a sequence default (serial/bigserial). Prefer GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY for new tables (PostgreSQL 10+). Identity columns have better ownership semantics and are the SQL standard approach.`
- **Notes**: This rule should not fire on `ALTER TABLE ... ADD COLUMN` to existing tables
  where changing to identity is harder. Consider suppressing or lowering severity in that
  context. Also coordinate with PGM007 to avoid duplicate noise -- if PGM105 fires, PGM007
  should not also fire for the same `nextval()` default.

---

### PGM106 -- Don't use `varchar(n)` for arbitrary length limits

- **Detects**: Column type `varchar(n)` or `character varying(n)` where a modifier is
  present (i.e., `TypeName.name == "varchar"` and `TypeName.modifiers.len() > 0`).
- **Why it's bad**: In PostgreSQL, `varchar(n)` provides no performance benefit over
  `text`. Both are stored identically as varlena. The only effect of `(n)` is a length
  check constraint that requires `ALTER TABLE ... ALTER COLUMN TYPE` (a full table
  rewrite) to change. When business requirements change and the limit needs to increase,
  this forces a dangerous DDL operation on production. A `CHECK` constraint
  (`CHECK(length(col) <= n)`) can be added or modified without a rewrite, or the column
  can simply be `text` with application-level validation.
- **Severity**: INFO
- **IR detectability**: Fully detectable. Match `TypeName.name == "varchar"` or
  `"character varying"` with non-empty `modifiers`.
- **Example bad SQL**:
  ```sql
  CREATE TABLE users (
      id bigint PRIMARY KEY,
      email varchar(255) NOT NULL,
      name varchar(100)
  );
  ```
- **Example fix**:
  ```sql
  CREATE TABLE users (
      id bigint PRIMARY KEY,
      email text NOT NULL,
      name text
  );
  -- Add CHECK constraints if length validation is truly needed:
  -- ALTER TABLE users ADD CONSTRAINT chk_email_len CHECK (length(email) <= 255);
  ```
- **Message**: `Column '{col}' on '{table}' uses 'varchar({n})'. In PostgreSQL, varchar(n) has no performance benefit over text, and the length limit requires a table rewrite to change. Consider using text instead, with a CHECK constraint if length validation is needed.`
- **Notes**: This is deliberately INFO severity because `varchar(n)` is extremely common
  and widely considered acceptable. Many teams prefer the self-documenting nature of
  explicit length limits. The rule serves as an informational nudge, not a hard
  recommendation. Teams that disagree should suppress this rule globally.

---

### PGM107 -- Don't use `float` / `real` / `double precision` for exact values

- **Detects**: Column type `float`, `float4`, `float8`, `real`, or `double precision`.
- **Why it's bad**: Floating-point types (`real` = `float4`, `double precision` = `float8`)
  use IEEE 754 binary representation, which cannot exactly represent most decimal
  fractions. This causes silent rounding errors in arithmetic: `0.1 + 0.2` does not
  equal `0.3`. For financial data, measurements requiring exact decimal representation,
  or any context where exact equality comparisons are needed, `numeric` should be used
  instead. Floating-point types are appropriate for scientific computations where
  approximate results and maximum performance are acceptable.
- **Severity**: INFO
- **IR detectability**: Fully detectable. Match `TypeName.name` being one of `"float"`,
  `"float4"`, `"float8"`, `"real"`, `"double precision"`.
- **Example bad SQL**:
  ```sql
  CREATE TABLE products (
      id bigint PRIMARY KEY,
      price float NOT NULL,
      weight double precision
  );
  ```
- **Example fix**:
  ```sql
  CREATE TABLE products (
      id bigint PRIMARY KEY,
      price numeric(10,2) NOT NULL,
      weight numeric(8,3)
  );
  ```
- **Message**: `Column '{col}' on '{table}' uses floating-point type '{type}'. Floating-point types have inexact representation. Use numeric for exact decimal values (especially monetary amounts). Floating-point is only appropriate for scientific/approximate computations.`
- **Notes**: INFO severity because floating-point columns are legitimate for many use
  cases (geospatial coordinates, sensor data, statistical computations). The rule is a
  reminder, not a mandate.

---

## Category 2: Constraint and Schema Anti-Patterns

### PGM108 -- Don't use `text` for enumerated values without a CHECK constraint

This rule is **not recommended for implementation** at this time.

- **Rationale**: Detecting that a column "should" have a CHECK constraint requires
  semantic understanding of the data model that is beyond static analysis. A `text`
  column storing status codes is indistinguishable from a `text` column storing
  free-form notes. Flagging every text column would be too noisy. This is better
  addressed through code review practices or application-level validation patterns.

---

### PGM109 -- Don't use SQL key words as identifiers

This rule is **not recommended for implementation** at this time.

- **Rationale**: While the wiki advises against using SQL reserved words as table or
  column names, detecting this requires maintaining a complete list of reserved words
  across PostgreSQL versions. The IR stores names as strings without quoting information,
  so distinguishing `"user"` (quoted, intentional) from `user` (unquoted, accidental) is
  not possible from the current IR. Additionally, many commonly used column names
  (`name`, `type`, `value`, `key`) are reserved words and flagging all of them would be
  extremely noisy.

---

## Category 3: Implicit Cast and Precision Anti-Patterns

### PGM110 -- Don't use `integer` as primary key type for large tables

This rule is **not recommended for implementation** at this time.

- **Rationale**: Whether `integer` (max ~2.1 billion) is sufficient depends on the
  table's expected row count, which is a domain-specific judgment. The wiki recommends
  `bigint` for new tables, but flagging every `integer` PK would be extremely noisy.
  This is better handled as a team convention through a configurable rule that defaults
  to off.

---

## Category 4: Additional DDL Anti-Patterns from the Wiki

### PGM111 -- Don't use `INHERITS` for table partitioning

- **Detects**: `CREATE TABLE ... INHERITS (parent)` syntax.
- **Why it's bad**: Old-style inheritance-based partitioning (pre-PG10) does not enforce
  partition constraints automatically, does not route inserts, and has poor query
  planning compared to native declarative partitioning (`PARTITION BY`). Declarative
  partitioning (available since PG 10) is superior in all respects.
- **Severity**: WARNING
- **IR detectability**: **Not currently detectable.** The `CreateTable` IR node does not
  include an `inherits` field. The parser would need to be extended to capture
  `INHERITS` clauses from the `pg_query` AST (`CreateStmt.inhRelations`). Alternatively,
  this could be flagged by matching on the raw SQL as an `Unparseable` node if the
  parser does not model it.
- **Example bad SQL**:
  ```sql
  CREATE TABLE events_2024 (
      CHECK (created_at >= '2024-01-01' AND created_at < '2025-01-01')
  ) INHERITS (events);
  ```
- **Example fix**:
  ```sql
  -- Use declarative partitioning instead:
  CREATE TABLE events (
      id bigint GENERATED ALWAYS AS IDENTITY,
      created_at timestamptz NOT NULL,
      data jsonb
  ) PARTITION BY RANGE (created_at);

  CREATE TABLE events_2024 PARTITION OF events
      FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
  ```
- **Message**: `Table '{table}' uses INHERITS-based inheritance. Use declarative partitioning (PARTITION BY) instead, which has better constraint enforcement, automatic insert routing, and query planning.`
- **Implementation note**: Requires IR extension. Add `inherits: Option<Vec<QualifiedName>>`
  to `CreateTable`, or add a new `IrNode::CreateTableInherits` variant.

---

### PGM112 -- Don't create unlogged tables without understanding the implications

This rule is **not recommended for implementation** at this time.

- **Rationale**: `UNLOGGED` tables are a deliberate performance trade-off. They are
  appropriate for ephemeral/staging data. Flagging them unconditionally would generate
  false positives. This is better addressed through code review.

---

## Summary Table

| Rule ID | Anti-Pattern | Severity | IR Match | Status |
|---------|-------------|----------|----------|--------|
| PGM101 | `timestamp` without time zone | WARNING | `name == "timestamp"` | **Implement** |
| PGM102 | `timestamp(0)` / `timestamptz(0)` precision | WARNING | `name in (timestamp, timestamptz) && modifiers == [0]` | **Implement** |
| PGM103 | `char(n)` / `character(n)` | WARNING | `name == "bpchar"` | **Implement** |
| PGM104 | `money` type | WARNING | `name == "money"` | **Implement** |
| PGM105 | `serial` / `bigserial` (prefer identity) | INFO | `nextval()` default on `int4`/`int8`/`int2` | **Implement** |
| PGM106 | `varchar(n)` (prefer text) | INFO | `name == "varchar" && modifiers non-empty` | Deferred (needs per-rule config) |
| PGM107 | `float` / `real` / `double precision` | INFO | `name in ("float4", "float8")` | Deferred (needs per-rule config) |
| PGM111 | `INHERITS`-based partitioning | WARNING | Not in IR | Deferred (needs IR extension) |

Rules not recommended: PGM108 (text without CHECK), PGM109 (reserved-word identifiers),
PGM110 (integer PK), PGM112 (unlogged tables).

---

## Implementation Notes

### Shared Detection Logic

Rules PGM101-PGM107 all follow the same pattern: inspect column type names in column
definitions. They should share a helper function:

```rust
fn check_column_type(
    table_name: &QualifiedName,
    col: &ColumnDef,
    span: &SourceSpan,
    file: &PathBuf,
) -> Vec<Finding> {
    let mut findings = vec![];
    // Dispatch to individual type checks...
    findings
}
```

This helper is called from `CreateTable` (iterating `columns`), `AlterTableAction::AddColumn`,
and `AlterTableAction::AlterColumnType` (checking `new_type`).

### Type Name Normalization

PostgreSQL has many aliases for the same type. The parser should normalize to canonical
names (lowercased) for reliable matching. Known aliases to handle:

| SQL Syntax | Canonical `TypeName.name` |
|------------|--------------------------|
| `timestamp` | `"timestamp"` |
| `timestamp without time zone` | `"timestamp"` |
| `timestamptz` | `"timestamptz"` |
| `timestamp with time zone` | `"timestamptz"` |
| `char(n)` | `"bpchar"` |
| `character(n)` | `"bpchar"` |
| `character varying(n)` | `"varchar"` |
| `varchar(n)` | `"varchar"` |
| `float` | `"float8"` (PG default) |
| `real` | `"float4"` |
| `double precision` | `"float8"` |
| `serial` | expanded to `"integer"` + `nextval()` |
| `bigserial` | expanded to `"bigint"` + `nextval()` |
| `smallserial` | expanded to `"smallint"` + `nextval()` |
| `int` | `"integer"` |
| `int4` | `"integer"` |
| `int8` | `"bigint"` |
| `decimal` | `"numeric"` |

The actual canonical names depend on `pg_query`'s AST output. Verify by parsing sample
DDL and inspecting the AST. The rule implementations should match against all known
aliases for each type.

### Interaction with Existing Rules

- **PGM105 vs PGM007**: Both fire on `nextval()` defaults. This is intentional — PGM007
  warns about the volatile default aspect (table rewrite risk), PGM105 recommends the
  identity column alternative. Both findings are relevant and neither suppresses the other.
- **PGM101 vs PGM009**: If someone uses `ALTER COLUMN TYPE` to change from `timestamptz`
  to `timestamp`, both PGM009 (type change on existing table) and PGM101 (bad type) can
  fire. This is correct behavior -- both findings are relevant (one is about the
  dangerous operation, the other about the bad target type).
- **PGM106 vs PGM009**: Changing `varchar(100)` to `varchar(200)` is already handled by
  PGM009's safe-cast allowlist. PGM106 firing on the new type is additive and correct.

### Configuration

Consider making PGM105, PGM106, and PGM107 disabled by default or configurable, as they
represent style preferences more than safety concerns. The existing `[rules]` config
section (reserved for future severity overrides) could be extended to support
enable/disable:

```toml
[rules.PGM106]
enabled = false  # Team prefers varchar(n)
```

---

## Non-DDL Anti-Patterns (Excluded)

The following items from the PostgreSQL "Don't Do This" wiki are **not detectable** in
migration files because they relate to query patterns, application logic, or runtime
behavior:

| Wiki Item | Reason Not Included |
|-----------|-------------------|
| Don't use `NOT IN` with nullable columns | Query pattern, not DDL |
| Don't use `BETWEEN` for timestamp ranges | Query pattern, not DDL |
| Don't use `upper()`/`lower()` for case-insensitive comparison | Query pattern; use `citext` or `ILIKE` |
| Don't use `count(*)` to check existence | Query pattern, not DDL |
| Don't use `LIMIT` without `ORDER BY` | Query pattern, not DDL |
| Don't use `SELECT *` | Query pattern, not DDL |
| Don't use `!=` (use `<>`) | Query pattern, style preference |
| Don't use `GRANT ALL` | Could be flagged but `GRANT` maps to `IrNode::Ignored` |
| Don't use `trust` authentication | `pg_hba.conf` setting, not DDL |
| Don't use `psql -W` | CLI usage, not DDL |

---

## Revision History

| Version | Date       | Changes |
|---------|------------|---------|
| 1.0     | 2025-02-10 | Initial draft. 8 rules proposed (PGM101-PGM107, PGM111). 4 rejected with rationale. |
