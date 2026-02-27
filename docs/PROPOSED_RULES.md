# Proposed Rules

Proposed rules use a `PGM1XXX` prefix indicating their target **range**, not a reserved slot. The leading `1` denotes "proposed"; the remaining digits identify the category (e.g., `PGM1506` targets the 5xx range). When promoted to implementation, a rule takes the next available ID in its range — so if `PGM1508` is promoted before `PGM1507`, it becomes `PGM506` (not `PGM508`). See `PLANNED_SCHEMA_CHANGES.md` for the full numbering scheme.

---

## 2xx — Destructive operations

### PGM1205 — `DROP SCHEMA ... CASCADE`

- **Range**: 2xx (DROP SCHEMA)
- **Severity**: CRITICAL
- **Status**: Not yet implemented.
- **Triggers**: `DROP SCHEMA ... CASCADE` where the schema contains at least one table in `catalog_before`.
- **Why**: `DROP SCHEMA CASCADE` drops every object in the schema — tables, views, sequences, functions, types, and indexes — in a single statement. It is the most destructive single DDL statement in PostgreSQL and cannot be selectively undone. There is no analog to `DROP TABLE CASCADE` that limits the scope; the entire schema is destroyed.
- **Does not fire when**:
  - The schema contains no tables in `catalog_before` (empty schema, no data at risk).
  - `CASCADE` is absent (`DROP SCHEMA` without `CASCADE` fails if the schema is non-empty, which is a safe default).
- **Message**: `DROP SCHEMA '{schema}' CASCADE drops every object in the schema. This is irreversible and destroys all tables, views, sequences, functions, and types within it.`
- **IR impact**: Requires a new top-level `IrNode` variant `DropSchema { name: String, cascade: bool, if_exists: bool }`. `pg_query` emits `DropStmt(OBJECT_SCHEMA)`.

---

## 5xx — Schema design & informational

### PGM1507 — `CREATE OR REPLACE FUNCTION` / `PROCEDURE` (maybe not?)

- **Range**: 5xx (Informational)
- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE OR REPLACE FUNCTION` or `CREATE OR REPLACE PROCEDURE`.
- **Why**: PostgreSQL prevents signature changes (return type, argument names/types) via `CREATE OR REPLACE` — attempting this produces an error. The risk is logic regression: `OR REPLACE` silently overwrites the existing function body with no "already exists" safety check. A developer reverting a function or deploying a buggy version has no friction — the migration succeeds and the regression is invisible until the function is called in production.
- **Does not fire when**:
  - `OR REPLACE` is absent (plain `CREATE FUNCTION` / `CREATE PROCEDURE` fails explicitly if the function already exists, forcing intentional action).
- **Message**: `CREATE OR REPLACE FUNCTION '{name}' silently overwrites the existing function body. It cannot change signatures, but it can introduce logic regressions with no warning. Verify the replacement is intentional.`
- **IR impact**: Requires a new top-level `IrNode` variant `CreateOrReplaceFunction { name: String }`. `pg_query` emits `CreateFunctionStmt` with `replace: bool`. Only the name and `replace` flag need to be extracted for v1.

---

### PGM1508 — `CREATE OR REPLACE VIEW` (maybe not?)

- **Range**: 5xx (Informational)
- **Severity**: INFO
- **Status**: Not yet implemented.
- **Triggers**: `CREATE OR REPLACE VIEW`.
- **Why**: PostgreSQL prevents removing columns or changing existing column types via `CREATE OR REPLACE VIEW` — attempting this produces an error. However it does permit adding new columns at the end, which silently affects any caller using `SELECT *` positional access. The primary risk is logic regression: `OR REPLACE` silently overwrites the view query with no "already exists" safety check. A developer reverting a view or deploying a buggy version has no friction. Dependent views, rules, or `WITH CHECK OPTION` constraints may also behave differently under the replacement query without any warning at migration time.
- **Does not fire when**:
  - `OR REPLACE` is absent (plain `CREATE VIEW` fails explicitly if the view already exists, forcing intentional action).
- **Message**: `CREATE OR REPLACE VIEW '{name}' silently overwrites the existing view query. New columns added at the end affect SELECT * callers. Verify the replacement is intentional and check dependent views and rules.`
- **IR impact**: Requires a new top-level `IrNode` variant `CreateOrReplaceView { name: String }`. `pg_query` emits `ViewStmt` with `replace: bool`. Only the name and `replace` flag need to be extracted for v1.

---

## Revision notes

These rules extend the current rule set. Proposed rules use the `PGM1XXX` prefix during the proposal phase, where the leading `1` means "proposed" and the remaining digits identify the target range. When a rule is promoted to implementation, it drops the leading `1` and takes the **next available ID** in its range — the exact slot is determined at promotion time, not by the proposal number.

Changes to existing spec sections required:

- **§4.2**: Add promoted rules to the rule table.
- **§3.2 IR node table**: Add `DropSchema`, `CreateOrReplaceFunction`, `CreateOrReplaceView`.
- **§11 Project structure**: Add rule files to `src/rules/` as rules are promoted.
- **PGM901 scope**: Update to cover all promoted rules.
