# pg_query Type Canonicalization Spike

Tested with `pg_query` crate v6.1.1 (libpg_query v17.0004).

## 1. Type Aliases — Canonical Names

pg_query returns type names in `TypeName.names[]` as a list of `String` nodes. Long-form aliases get a `pg_catalog.` prefix; short-form names do not.

| User Input | names[] | Canonical (last element) | Notes |
|---|---|---|---|
| `int` | `pg_catalog.int4` | `int4` | |
| `integer` | `pg_catalog.int4` | `int4` | |
| `int4` | `int4` | `int4` | Short form, no prefix |
| `int8` | `int8` | `int8` | Short form, no prefix |
| `bigint` | `pg_catalog.int8` | `int8` | |
| `smallint` | `pg_catalog.int2` | `int2` | |
| `int2` | `int2` | `int2` | Short form, no prefix |
| `bool` | `bool` | `bool` | Short form, no prefix |
| `boolean` | `pg_catalog.bool` | `bool` | |
| `varchar` | `pg_catalog.varchar` | `varchar` | |
| `varchar(100)` | `pg_catalog.varchar` | `varchar` | Modifier: `[100]` via typmods |
| `character varying` | `pg_catalog.varchar` | `varchar` | |
| `character varying(100)` | `pg_catalog.varchar` | `varchar` | Modifier: `[100]` |
| `text` | `text` | `text` | Short form, no prefix |
| `char` | `pg_catalog.bpchar` | `bpchar` | Implicit modifier: `[1]` |
| `char(5)` | `pg_catalog.bpchar` | `bpchar` | Modifier: `[5]` |
| `character` | `pg_catalog.bpchar` | `bpchar` | Implicit modifier: `[1]` |
| `serial` | `serial` | `serial` | **NOT expanded** (see §2) |
| `bigserial` | `bigserial` | `bigserial` | **NOT expanded** (see §2) |
| `numeric` | `pg_catalog.numeric` | `numeric` | |
| `numeric(10,2)` | `pg_catalog.numeric` | `numeric` | Modifiers: `[10, 2]` |
| `decimal` | `pg_catalog.numeric` | `numeric` | Same as numeric |
| `float` | `pg_catalog.float8` | `float8` | |
| `real` | `pg_catalog.float4` | `float4` | |
| `double precision` | `pg_catalog.float8` | `float8` | |
| `timestamp` | `pg_catalog.timestamp` | `timestamp` | |
| `timestamptz` | `timestamptz` | `timestamptz` | Short form, no prefix |
| `timestamp with time zone` | `pg_catalog.timestamptz` | `timestamptz` | |
| `uuid` | `uuid` | `uuid` | Short form, no prefix |
| `jsonb` | `jsonb` | `jsonb` | Short form, no prefix |
| `json` | `pg_catalog.json` | `json` | |

### Parser Agent: Canonical Name Extraction

**Use the LAST element** of `TypeName.names[]` as the canonical type name. This normalizes all aliases automatically:
- `int`, `integer`, `int4` → all become `int4`
- `bigint`, `int8` → all become `int8`
- `boolean`, `bool` → all become `bool`
- `varchar`, `character varying` → all become `varchar`
- `float`, `double precision` → all become `float8`

### Type Modifiers (typmods)

Modifiers appear in `TypeName.typmods[]` as `AConst(Integer)` nodes. Extract the `ival` field.

- `varchar(100)` → modifiers: `[100]`
- `numeric(10,2)` → modifiers: `[10, 2]`
- `char` / `character` → implicit modifier `[1]` (location=-1 indicates synthetic)
- `char(5)` → modifier `[5]`
- Types without modifiers → empty `typmods[]`

## 2. Serial Expansion

**`serial` and `bigserial` are NOT expanded by pg_query.** They are preserved as literal type names without a `pg_catalog.` prefix and without a default expression.

```
serial    → names=[serial],    no default, is_not_null=false
bigserial → names=[bigserial], no default, is_not_null=false
```

### Implications for Parser Agent

The Parser Agent must handle `serial`/`bigserial` specially:
1. Detect `serial` → map to `TypeName { name: "int4" }` with `has_default: true`
2. Detect `bigserial` → map to `TypeName { name: "int8" }` with `has_default: true`
3. Set `default_expr: Some(FunctionCall { name: "nextval", args: vec![] })` (or just mark as volatile default)
4. **Do NOT set is_not_null** — pg_query doesn't, so NOT NULL must come from a separate constraint

This matters for:
- **PGM007** (volatile default): serial implies `nextval()` which is volatile, but since serial columns are expected, the rule should **allowlist serial/bigserial** rather than flagging them
- **PGM010** (ADD COLUMN NOT NULL without default): serial columns have an implicit default

## 3. Inline vs Table-Level Constraints

### Primary Key

**Inline** (`id int PRIMARY KEY`):
- Appears in `ColumnDef.constraints[]` as `Constraint { contype: ConstrPrimary }`
- No `keys[]` — the column it's on IS the key

**Table-level** (`PRIMARY KEY (id)`):
- Appears in `CreateStmt.table_elts[]` as a `Constraint` node (not inside a ColumnDef)
- `keys[]` contains the column name(s)

### Foreign Key

**Inline** (`customer_id int REFERENCES customers(id)`):
- Appears in `ColumnDef.constraints[]` as `Constraint { contype: ConstrForeign }`
- FK details are on the constraint: `pktable`, `pk_attrs`
- The referencing column is implicit (the column it's defined on)

**Table-level** (`FOREIGN KEY (customer_id) REFERENCES customers(id)`):
- Appears in `CreateStmt.table_elts[]` as a `Constraint` node
- `fk_attrs[]` = referencing columns
- `pktable` = referenced table
- `pk_attrs[]` = referenced columns

### Unique

**Inline** (`email text UNIQUE`):
- Appears in `ColumnDef.constraints[]` as `Constraint { contype: ConstrUnique }`

**Table-level** (`UNIQUE (email)`):
- Appears in `CreateStmt.table_elts[]` as a `Constraint` node
- `keys[]` contains column name(s)

### NOT NULL

**Inline only** (`col text NOT NULL`):
- Appears as `Constraint { contype: ConstrNotnull }` in column constraints
- **Also**: `ColumnDef.is_not_null` is `false` even with NOT NULL constraint! (the constraint is what carries the NOT NULL flag)

### CHECK

**Inline** (`col int CHECK (col > 0)`):
- Appears in `ColumnDef.constraints[]` as `Constraint { contype: ConstrCheck }`

**Table-level** (`CHECK (col > 0)`):
- Appears in `CreateStmt.table_elts[]` as `Constraint` node

### Parser Agent: Constraint Handling Strategy

The parser must normalize both forms into the same IR:
1. Walk `CreateStmt.table_elts[]` and collect both `ColumnDef` and `Constraint` nodes
2. For inline constraints on a ColumnDef: extract from `col.constraints[]`
3. For table-level constraints: extract from the `Constraint` nodes in `table_elts[]`
4. Map both to the same `TableConstraint` IR enum variants

## 4. Default Expressions

Defaults appear as `Constraint { contype: ConstrDefault }` in `ColumnDef.constraints[]`, NOT in `ColumnDef.raw_default`.

The expression is in `constraint.raw_expr`:

| Default Value | raw_expr Node | How to Detect |
|---|---|---|
| `0` | `AConst { val: Ival(0) }` | `AConst` → `Literal` |
| `'active'` | `AConst { val: Sval("active") }` | `AConst` → `Literal` |
| `TRUE` | `AConst { val: Boolval(true) }` | `AConst` → `Literal` |
| `now()` | `FuncCall { funcname: ["now"] }` | `FuncCall` → `FunctionCall` |
| `gen_random_uuid()` | `FuncCall { funcname: ["gen_random_uuid"] }` | `FuncCall` → `FunctionCall` |
| `nextval('seq'::regclass)` | `FuncCall { funcname: ["nextval"], args: [TypeCast] }` | `FuncCall` → `FunctionCall` |

### Parser Agent: Default Expression Mapping

```
AConst → DefaultExpr::Literal(value_as_string)
FuncCall → DefaultExpr::FunctionCall { name: last_funcname, args }
Everything else → DefaultExpr::Other(deparsed_sql)
```

## 5. ALTER TABLE Subtypes

| SQL | subtype enum | name field | def field |
|---|---|---|---|
| `ADD COLUMN status text` | `AtAddColumn` | `""` | `ColumnDef` |
| `DROP COLUMN old_field` | `AtDropColumn` | `"old_field"` | None |
| `ADD CONSTRAINT fk_foo ...` | `AtAddConstraint` | `""` | `Constraint` |
| `ALTER COLUMN status TYPE varchar(100)` | `AtAlterColumnType` | `"status"` | `ColumnDef` (type in type_name) |
| `ALTER COLUMN price SET NOT NULL` | `AtSetNotNull` | `"price"` | None |

### Parser Agent: AlterTableCmd Mapping

- `AtAddColumn` → `AlterTableAction::AddColumn(column_def)`
- `AtDropColumn` → `AlterTableAction::DropColumn { name }`
- `AtAddConstraint` → `AlterTableAction::AddConstraint(constraint)`
- `AtAlterColumnType` → `AlterTableAction::AlterColumnType { column_name: cmd.name, new_type }`
- `AtSetNotNull` → `AlterTableAction::Other { description: "SET NOT NULL" }`
- Everything else → `AlterTableAction::Other { description }`

## 6. CREATE/DROP INDEX

Fields map cleanly:

| Field | pg_query | IR |
|---|---|---|
| Index name | `IndexStmt.idxname` | `CreateIndex.index_name` |
| Table | `IndexStmt.relation.relname` | `CreateIndex.table_name` |
| Columns | `IndexStmt.index_params[].name` | `CreateIndex.columns` |
| Unique | `IndexStmt.unique` | `CreateIndex.unique` |
| Concurrent | `IndexStmt.concurrent` | `CreateIndex.concurrent` |

For `DROP INDEX`:
- Index name is in `DropStmt.objects[]` as `List { items: [String(name)] }`
- Concurrent flag: `DropStmt.concurrent`

## 7. DO Blocks

Parsed as `DoStmt` with the body as a string. **Treat as `IrNode::Unparseable`** since the SQL inside is opaque PL/pgSQL.

## 8. Multi-Statement Parsing

pg_query parses all statements in one call. `RawStmt.stmt_location` gives byte offset, `RawStmt.stmt_len` gives length. Use these to compute line numbers for `SourceSpan`.

## 9. Binary-Coercible Type Pairs (for PGM009)

Using canonical names from §1:

| From | To | Safe? | Notes |
|---|---|---|---|
| `varchar(N)` | `varchar(M)` M≥N | Yes | Length increase |
| `varchar(N)` | `text` | Yes | Remove length constraint |
| `numeric(p1,s)` | `numeric(p2,s)` p2≥p1 | Yes | Precision increase, same scale |
| `text` | `varchar(N)` | **No** | Could truncate data |
| `int4` | `int8` | Yes | Widening cast |
| `int2` | `int4` | Yes | Widening cast |
| `float4` | `float8` | Yes | Widening cast |

The allowlist in PGM009 must use canonical names (`int4`, `int8`, `varchar`, `numeric`, etc.), not user-facing aliases.
