//! Intermediate Representation (IR) for SQL statements
//!
//! The IR layer decouples the parser from the rule engine. It represents
//! only the information needed for linting, not the full PostgreSQL AST.

use crate::names::{RelationName, ResolvedName};
use std::fmt;

/// A parsed SQL statement mapped to a high-level operation.
/// Each variant carries only the fields rules need — not the full AST.
///
/// Generic over the type of its relation names. The parser produces
/// `IrNode<QualifiedName>`, whose schemas are optional; normalization
/// resolves that into the default `IrNode<ResolvedName>`, which is what
/// replay and the rules see. Bare names (`RENAME TO` targets, index names in
/// `CREATE INDEX`, constraint and column names) stay `String`.
#[derive(Debug, Clone, PartialEq)]
pub enum IrNode<N = ResolvedName> {
    CreateTable(CreateTable<N>),
    AlterTable(AlterTable<N>),
    CreateIndex(CreateIndex<N>),
    DropIndex(DropIndex<N>),
    DropTable(DropTable<N>),
    DropSchema(DropSchema),
    TruncateTable(TruncateTable<N>),
    /// DML: INSERT INTO a table.
    InsertInto(InsertInto<N>),
    /// DML: UPDATE rows in a table.
    UpdateTable(UpdateTable<N>),
    /// DML: DELETE FROM a table.
    DeleteFrom(DeleteFrom<N>),
    /// CLUSTER rewrites the table under ACCESS EXCLUSIVE lock.
    Cluster(Cluster<N>),
    /// `VACUUM FULL` rewrites the table under ACCESS EXCLUSIVE lock.
    VacuumFull(VacuumFull<N>),
    /// `REINDEX` without `CONCURRENTLY` acquires ACCESS EXCLUSIVE lock.
    Reindex(Reindex<N>),
    /// `ALTER INDEX parent ATTACH PARTITION child` — attaches a child index
    /// to a parent ON ONLY index, making it valid (recursive).
    AlterIndexAttachPartition {
        parent_index_name: N,
        child_index_name: N,
    },
    /// Rename an existing table. pg_query emits `RenameStmt`, not `AlterTableStmt`.
    /// `new_name` is bare: the grammar allows no schema, and a rename never
    /// moves a table to another schema.
    RenameTable {
        name: N,
        new_name: String,
    },
    /// Rename a column on an existing table. pg_query emits `RenameStmt`.
    RenameColumn {
        table: N,
        old_name: String,
        new_name: String,
    },
    /// Rename an existing index. pg_query emits `RenameStmt`.
    RenameIndex {
        index: N,
        new_name: String,
    },
    /// SQL that parsed successfully but has no IR mapping (e.g., GRANT, COMMENT ON).
    /// Not an error — just not relevant to linting.
    Ignored {
        raw_sql: String,
    },
    /// SQL that failed to parse or is inherently opaque (DO $$ blocks, dynamic SQL).
    /// The replay engine uses `table_hint` to mark affected tables as incomplete.
    Unparseable {
        raw_sql: String,
        table_hint: Option<N>,
    },
}

/// Partition strategy for `CREATE TABLE ... PARTITION BY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionStrategy {
    Range,
    List,
    Hash,
}

/// Partition key specification from `CREATE TABLE ... PARTITION BY strategy (columns)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionBy {
    pub strategy: PartitionStrategy,
    pub columns: Vec<String>,
}

/// Table persistence mode, mapping 1:1 to PostgreSQL's `relpersistence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablePersistence {
    /// Regular permanent table (relpersistence = 'p', default).
    Permanent,
    /// Unlogged table — not WAL-logged, truncated on crash recovery,
    /// not replicated to standbys (relpersistence = 'u').
    Unlogged,
    /// Temporary table — session-local, dropped at end of session or
    /// transaction (relpersistence = 't').
    Temporary,
}

/// Scope of a `DISABLE TRIGGER` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerDisableScope {
    /// `DISABLE TRIGGER name` — disables a specific trigger.
    Named(String),
    /// `DISABLE TRIGGER ALL` — disables all triggers including system/FK triggers.
    All,
    /// `DISABLE TRIGGER USER` — disables only user-defined triggers, not FK triggers.
    User,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable<N = ResolvedName> {
    pub name: N,
    pub columns: Vec<ColumnDef>,
    pub constraints: Vec<TableConstraint<N>>,
    pub persistence: TablePersistence,
    pub if_not_exists: bool,
    /// `PARTITION BY strategy (columns)` clause, if present.
    pub partition_by: Option<PartitionBy>,
    /// Parent table for `CREATE TABLE child PARTITION OF parent ...`.
    pub partition_of: Option<N>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterTable<N = ResolvedName> {
    pub name: N,
    pub actions: Vec<AlterTableAction<N>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlterTableAction<N = ResolvedName> {
    AddColumn(ColumnDef),
    DropColumn {
        name: String,
    },
    AddConstraint(TableConstraint<N>),
    AlterColumnType {
        column_name: String,
        new_type: TypeName,
        /// Only available if catalog provides it — not from the SQL itself.
        /// Rules that need old_type must look it up in the catalog.
        old_type: Option<TypeName>,
    },
    /// SET NOT NULL on an existing column (requires ACCESS EXCLUSIVE lock).
    SetNotNull {
        column_name: String,
    },
    /// DROP NOT NULL on an existing column — allows NULLs.
    DropNotNull {
        column_name: String,
    },
    /// SET DEFAULT on an existing column — assigns a new default value.
    SetDefault {
        column_name: String,
        default_expr: DefaultExpr,
    },
    /// DROP DEFAULT on an existing column — removes the default value.
    DropDefault {
        column_name: String,
    },
    /// DROP CONSTRAINT by name.
    DropConstraint {
        constraint_name: String,
    },
    /// VALIDATE CONSTRAINT by name — clears the `not_valid` flag on FK/CHECK.
    ValidateConstraint {
        constraint_name: String,
    },
    /// `ALTER TABLE parent ATTACH PARTITION child FOR VALUES ...`
    AttachPartition {
        child: N,
    },
    /// `ALTER TABLE parent DETACH PARTITION child [CONCURRENTLY]`
    DetachPartition {
        child: N,
        concurrent: bool,
    },
    /// `ALTER TABLE ... DISABLE TRIGGER [name | ALL | USER]`
    DisableTrigger {
        scope: TriggerDisableScope,
    },
    /// Catch-all for ALTER TABLE actions we parse but don't model.
    Other {
        description: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndex<N = ResolvedName> {
    pub index_name: Option<String>,
    pub table_name: N,
    pub columns: Vec<IndexColumn>,
    /// Non-key columns from `INCLUDE (...)`, in written order.
    pub include_columns: Vec<String>,
    pub unique: bool,
    pub concurrent: bool,
    pub if_not_exists: bool,
    /// Deparsed WHERE clause for partial indexes, e.g. `"active = true"`.
    pub where_clause: Option<String>,
    /// `CREATE INDEX ON ONLY parent_table` — index only on the parent,
    /// not propagated to partitions.
    pub only: bool,
    /// Index access method: `"btree"` (default), `"gin"`, `"gist"`, `"hash"`, `"brin"`.
    pub access_method: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropIndex<N = ResolvedName> {
    pub index_name: N,
    pub concurrent: bool,
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropTable<N = ResolvedName> {
    pub name: N,
    pub if_exists: bool,
    pub cascade: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropSchema {
    pub schema_name: String,
    pub if_exists: bool,
    pub cascade: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TruncateTable<N = ResolvedName> {
    pub name: N,
    pub cascade: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertInto<N = ResolvedName> {
    pub table_name: N,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateTable<N = ResolvedName> {
    pub table_name: N,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteFrom<N = ResolvedName> {
    pub table_name: N,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cluster<N = ResolvedName> {
    pub table: N,
    pub index: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VacuumFull<N = ResolvedName> {
    /// Target table, or `None` for `VACUUM FULL;` (all tables in the database).
    pub table: Option<N>,
}

/// The kind of object targeted by `REINDEX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReindexObjectKind {
    Table,
    Index,
    Schema,
    Database,
    System,
}

impl fmt::Display for ReindexObjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Table => write!(f, "TABLE"),
            Self::Index => write!(f, "INDEX"),
            Self::Schema => write!(f, "SCHEMA"),
            Self::Database => write!(f, "DATABASE"),
            Self::System => write!(f, "SYSTEM"),
        }
    }
}

/// Target of a `REINDEX` statement.
#[derive(Debug, Clone, PartialEq)]
pub enum ReindexTarget<N = ResolvedName> {
    /// TABLE or INDEX — uses a schema-qualifiable relation reference.
    Relation(N),
    /// SCHEMA or DATABASE — a plain name (not schema-qualified).
    Named(String),
}

/// `REINDEX [TABLE|INDEX|SCHEMA|DATABASE] [CONCURRENTLY] name`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reindex<N = ResolvedName> {
    pub kind: ReindexObjectKind,
    pub target: ReindexTarget<N>,
    pub concurrent: bool,
}

// --- Supporting types ---

/// A relation name as written in the SQL. `schema` is `None` for unqualified
/// references.
///
/// This is the parser's name type. Normalization turns it into a
/// [`ResolvedName`] with [`resolve`](Self::resolve).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedName {
    pub schema: Option<String>,
    pub name: String,
}

impl QualifiedName {
    /// A name written without a schema.
    pub fn unqualified(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    /// A name written as `schema.name`.
    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// Resolve this name against `default_schema`, which fills in the schema
    /// of an unqualified name. The result remembers whether the schema was
    /// filled in, so messages can omit it.
    pub fn resolve(self, default_schema: &str) -> ResolvedName {
        match self.schema {
            Some(schema) => ResolvedName::new(RelationName::new(schema, self.name), false),
            None => ResolvedName::new(RelationName::new(default_schema, self.name), true),
        }
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.schema {
            Some(s) => write!(f, "{}.{}", s, self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: TypeName,
    pub nullable: bool, // true = nullable (default), false = NOT NULL
    pub default_expr: Option<DefaultExpr>,
    /// True if this column has an inline PRIMARY KEY constraint.
    pub is_inline_pk: bool,
    /// True if this column was declared as `serial`, `bigserial`, or `smallserial`.
    pub is_serial: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeName {
    /// The base type name, lowercased: "integer", "varchar", "numeric", etc.
    pub name: String,
    /// Type modifiers. For varchar(100): modifiers = [100].
    /// For numeric(10,2): modifiers = [10, 2].
    pub modifiers: Vec<i64>,
}

impl TypeName {
    pub fn simple(name: impl Into<String>) -> Self {
        Self {
            name: name.into().to_lowercase(),
            modifiers: vec![],
        }
    }

    pub fn with_modifiers(name: impl Into<String>, modifiers: Vec<i64>) -> Self {
        Self {
            name: name.into().to_lowercase(),
            modifiers,
        }
    }
}

impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if !self.modifiers.is_empty() {
            let mods: Vec<String> = self.modifiers.iter().map(|m| m.to_string()).collect();
            write!(f, "({})", mods.join(","))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefaultExpr {
    /// A constant literal: 0, 'active', TRUE, etc.
    Literal(String),
    /// A function call: now(), gen_random_uuid(), my_func(), etc.
    FunctionCall { name: String, args: Vec<String> },
    /// An expression we parsed but can't categorize. Treated as opaque.
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableConstraint<N = ResolvedName> {
    PrimaryKey {
        name: Option<String>,
        columns: Vec<String>,
        /// Non-key columns from `INCLUDE (...)`, in written order.
        include_columns: Vec<String>,
        /// Index name from `USING INDEX` clause, e.g. `ADD PRIMARY KEY USING INDEX idx`.
        /// When present, `columns` will be empty (PostgreSQL derives them from the index).
        using_index: Option<String>,
    },
    ForeignKey {
        name: Option<String>,
        columns: Vec<String>,
        ref_table: N,
        ref_columns: Vec<String>,
        not_valid: bool,
    },
    Unique {
        name: Option<String>,
        columns: Vec<String>,
        /// Non-key columns from `INCLUDE (...)`, in written order.
        include_columns: Vec<String>,
        /// Index name from `USING INDEX` clause, e.g. `ADD UNIQUE USING INDEX idx`.
        /// When present, `columns` will be empty (PostgreSQL derives them from the index).
        using_index: Option<String>,
    },
    Check {
        name: Option<String>,
        expression: String,
        /// Distinct columns the expression references, in order of first
        /// appearance. Postgres names an unnamed CHECK `{table}_{col}_check`
        /// when there is exactly one, and `{table}_check` otherwise.
        referenced_columns: Vec<String>,
        not_valid: bool,
    },
    /// EXCLUDE constraint. `elements` captures each `WITH`-clause entry's column
    /// or expression (e.g. `room, period` in `room WITH =, period WITH &&`);
    /// the operators themselves are not tracked.
    Exclude {
        name: Option<String>,
        elements: Vec<IndexColumn>,
        /// Non-key columns from `INCLUDE (...)`, in written order.
        include_columns: Vec<String>,
    },
}

/// An element in an index's column list.
///
/// Most indexes reference plain column names, but expression indexes
/// (e.g. `CREATE INDEX idx ON t (LOWER(email))`) store the deparsed SQL text.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexColumn {
    /// Simple column reference by name.
    Column(String),
    /// Expression index element, stored as deparsed SQL text with
    /// extracted column references for DROP/RENAME tracking.
    Expression {
        text: String,
        referenced_columns: Vec<String>,
        /// What this element contributes to a name Postgres generates for
        /// the index or EXCLUDE constraint, derived from the parse tree the
        /// way `FigureIndexColname` does: a function name, a column or field
        /// name, an internal type name, `case`, or
        /// [`IndexColumn::UNNAMED_EXPRESSION_NAME_PART`].
        generated_name_part: String,
    },
}

impl IndexColumn {
    /// The name part Postgres gives an expression element it has no better
    /// name for, such as an operator expression.
    pub const UNNAMED_EXPRESSION_NAME_PART: &'static str = "expr";

    /// What this element contributes to a name Postgres generates for the
    /// index or EXCLUDE constraint: the column name for a plain column, or the
    /// expression's derived name part.
    ///
    /// This is the name before de-duplication; Postgres appends a number to a
    /// part that repeats an earlier element's (`a, a` becomes `a_a1`).
    pub fn generated_name_part(&self) -> &str {
        match self {
            Self::Column(name) => name,
            Self::Expression {
                generated_name_part,
                ..
            } => generated_name_part,
        }
    }

    /// Returns the column name if this is a plain column, or `None` for expressions.
    pub fn column_name(&self) -> Option<&str> {
        match self {
            Self::Column(n) => Some(n),
            Self::Expression { .. } => None,
        }
    }

    /// Returns true if this entry references the given column — either as a
    /// plain column or inside an expression's `referenced_columns`.
    pub fn references_column(&self, col: &str) -> bool {
        match self {
            Self::Column(name) => name == col,
            Self::Expression {
                referenced_columns, ..
            } => referenced_columns.iter().any(|c| c == col),
        }
    }
}

/// A parsed statement with its source location.
#[derive(Debug, Clone)]
pub struct Located<T> {
    pub node: T,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpan {
    pub start_line: usize,   // 1-based
    pub end_line: usize,     // 1-based, inclusive
    pub start_offset: usize, // byte offset from start of file
    pub end_offset: usize,
}

impl SourceSpan {
    /// Create a span with only line numbers (offsets set to 0).
    ///
    /// Useful in tests where byte offsets are irrelevant.
    pub fn at(start_line: usize, end_line: usize) -> Self {
        Self {
            start_line,
            end_line,
            start_offset: 0,
            end_offset: 0,
        }
    }
}

#[cfg(test)]
impl ColumnDef {
    /// Minimal column: nullable, no default, not a PK, not serial.
    pub fn test(name: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_name: TypeName::simple(type_name),
            nullable: true,
            default_expr: None,
            is_inline_pk: false,
            is_serial: false,
        }
    }

    pub fn with_nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    pub fn with_default(mut self, default_expr: DefaultExpr) -> Self {
        self.default_expr = Some(default_expr);
        self
    }

    pub fn with_inline_pk(mut self) -> Self {
        self.is_inline_pk = true;
        self.nullable = false;
        self
    }

    pub fn with_serial(mut self) -> Self {
        self.is_serial = true;
        self.default_expr = Some(DefaultExpr::FunctionCall {
            name: "nextval".to_string(),
            args: vec![],
        });
        self
    }

    pub fn with_type(mut self, type_name: TypeName) -> Self {
        self.type_name = type_name;
        self
    }
}

#[cfg(test)]
impl CreateTable<QualifiedName> {
    /// Minimal CREATE TABLE: no columns, no constraints, permanent, no IF NOT EXISTS, no partitions.
    pub fn test(name: QualifiedName) -> Self {
        Self {
            name,
            columns: vec![],
            constraints: vec![],
            persistence: TablePersistence::Permanent,
            if_not_exists: false,
            partition_by: None,
            partition_of: None,
        }
    }

    pub fn with_columns(mut self, columns: Vec<ColumnDef>) -> Self {
        self.columns = columns;
        self
    }

    pub fn with_constraints(mut self, constraints: Vec<TableConstraint<QualifiedName>>) -> Self {
        self.constraints = constraints;
        self
    }

    pub fn with_persistence(mut self, persistence: TablePersistence) -> Self {
        self.persistence = persistence;
        self
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }

    pub fn with_partition_by(mut self, strategy: PartitionStrategy, columns: Vec<String>) -> Self {
        self.partition_by = Some(PartitionBy { strategy, columns });
        self
    }

    pub fn with_partition_of(mut self, parent: QualifiedName) -> Self {
        self.partition_of = Some(parent);
        self
    }
}

#[cfg(test)]
impl CreateIndex<QualifiedName> {
    /// Minimal CREATE INDEX: no columns, no INCLUDE, not unique, not concurrent, no IF NOT EXISTS, no WHERE, not ONLY, btree.
    pub fn test(index_name: impl Into<Option<String>>, table_name: QualifiedName) -> Self {
        Self {
            index_name: index_name.into(),
            table_name,
            columns: vec![],
            include_columns: vec![],
            unique: false,
            concurrent: false,
            if_not_exists: false,
            where_clause: None,
            only: false,
            access_method: crate::catalog::types::IndexState::DEFAULT_ACCESS_METHOD.to_string(),
        }
    }

    pub fn with_where_clause(mut self, clause: impl Into<String>) -> Self {
        self.where_clause = Some(clause.into());
        self
    }

    pub fn with_columns(mut self, columns: Vec<IndexColumn>) -> Self {
        self.columns = columns;
        self
    }

    pub fn with_unique(mut self, unique: bool) -> Self {
        self.unique = unique;
        self
    }

    pub fn with_concurrent(mut self, concurrent: bool) -> Self {
        self.concurrent = concurrent;
        self
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }

    pub fn with_only(mut self, only: bool) -> Self {
        self.only = only;
        self
    }

    pub fn with_access_method(mut self, method: impl Into<String>) -> Self {
        self.access_method = method.into();
        self
    }
}

#[cfg(test)]
impl DropTable<QualifiedName> {
    /// Minimal DROP TABLE: if_exists defaults to true (safe default), cascade false.
    pub fn test(name: QualifiedName) -> Self {
        Self {
            name,
            if_exists: true,
            cascade: false,
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn with_cascade(mut self, cascade: bool) -> Self {
        self.cascade = cascade;
        self
    }
}

#[cfg(test)]
impl DropIndex<QualifiedName> {
    /// Minimal DROP INDEX: not concurrent, if_exists defaults to true.
    pub fn test(index_name: QualifiedName) -> Self {
        Self {
            index_name,
            concurrent: false,
            if_exists: true,
        }
    }

    pub fn with_concurrent(mut self, concurrent: bool) -> Self {
        self.concurrent = concurrent;
        self
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }
}

#[cfg(test)]
impl DropSchema {
    /// Minimal DROP SCHEMA: if_exists defaults to false, cascade false.
    pub fn test(schema_name: impl Into<String>) -> Self {
        Self {
            schema_name: schema_name.into(),
            if_exists: false,
            cascade: false,
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn with_cascade(mut self, cascade: bool) -> Self {
        self.cascade = cascade;
        self
    }
}

#[cfg(test)]
impl TruncateTable<QualifiedName> {
    /// Minimal TRUNCATE TABLE: cascade false.
    pub fn test(name: QualifiedName) -> Self {
        Self {
            name,
            cascade: false,
        }
    }

    pub fn with_cascade(mut self, cascade: bool) -> Self {
        self.cascade = cascade;
        self
    }
}

#[cfg(test)]
impl InsertInto<QualifiedName> {
    /// Minimal INSERT INTO.
    pub fn test(table_name: QualifiedName) -> Self {
        Self { table_name }
    }
}

#[cfg(test)]
impl UpdateTable<QualifiedName> {
    /// Minimal UPDATE.
    pub fn test(table_name: QualifiedName) -> Self {
        Self { table_name }
    }
}

#[cfg(test)]
impl DeleteFrom<QualifiedName> {
    /// Minimal DELETE FROM.
    pub fn test(table_name: QualifiedName) -> Self {
        Self { table_name }
    }
}

#[cfg(test)]
impl Cluster<QualifiedName> {
    /// Minimal CLUSTER: no index.
    pub fn test(table: QualifiedName) -> Self {
        Self { table, index: None }
    }

    pub fn with_index(mut self, index: impl Into<String>) -> Self {
        self.index = Some(index.into());
        self
    }
}

#[cfg(test)]
impl VacuumFull<QualifiedName> {
    /// VACUUM FULL targeting a specific table.
    pub fn test(table: QualifiedName) -> Self {
        Self { table: Some(table) }
    }

    /// VACUUM FULL targeting all tables (no table specified).
    pub fn test_all() -> Self {
        Self { table: None }
    }
}

#[cfg(test)]
impl Reindex<QualifiedName> {
    /// Build a REINDEX TABLE (non-concurrent) for testing.
    pub fn test_table(table: QualifiedName) -> Self {
        Self {
            kind: ReindexObjectKind::Table,
            target: ReindexTarget::Relation(table),
            concurrent: false,
        }
    }

    /// Build a REINDEX INDEX (non-concurrent) for testing.
    pub fn test_index(index: QualifiedName) -> Self {
        Self {
            kind: ReindexObjectKind::Index,
            target: ReindexTarget::Relation(index),
            concurrent: false,
        }
    }

    /// Build a REINDEX SCHEMA (non-concurrent) for testing.
    pub fn test_schema(name: &str) -> Self {
        Self {
            kind: ReindexObjectKind::Schema,
            target: ReindexTarget::Named(name.to_string()),
            concurrent: false,
        }
    }

    /// Build a REINDEX DATABASE (non-concurrent) for testing.
    pub fn test_database(name: &str) -> Self {
        Self {
            kind: ReindexObjectKind::Database,
            target: ReindexTarget::Named(name.to_string()),
            concurrent: false,
        }
    }

    /// Set concurrent flag for testing.
    pub fn with_concurrent(mut self) -> Self {
        self.concurrent = true;
        self
    }
}

// Convenience conversions for test construction: builder.into() -> IrNode variant,
// in parser form or resolved form.
#[cfg(test)]
impl<N> From<CreateTable<N>> for IrNode<N> {
    fn from(value: CreateTable<N>) -> Self {
        IrNode::CreateTable(value)
    }
}

#[cfg(test)]
impl<N> From<AlterTable<N>> for IrNode<N> {
    fn from(value: AlterTable<N>) -> Self {
        IrNode::AlterTable(value)
    }
}

#[cfg(test)]
impl<N> From<CreateIndex<N>> for IrNode<N> {
    fn from(value: CreateIndex<N>) -> Self {
        IrNode::CreateIndex(value)
    }
}

#[cfg(test)]
impl<N> From<DropTable<N>> for IrNode<N> {
    fn from(value: DropTable<N>) -> Self {
        IrNode::DropTable(value)
    }
}

#[cfg(test)]
impl<N> From<DropIndex<N>> for IrNode<N> {
    fn from(value: DropIndex<N>) -> Self {
        IrNode::DropIndex(value)
    }
}

#[cfg(test)]
impl<N> From<DropSchema> for IrNode<N> {
    fn from(value: DropSchema) -> Self {
        IrNode::DropSchema(value)
    }
}

#[cfg(test)]
impl<N> From<TruncateTable<N>> for IrNode<N> {
    fn from(value: TruncateTable<N>) -> Self {
        IrNode::TruncateTable(value)
    }
}

#[cfg(test)]
impl<N> From<InsertInto<N>> for IrNode<N> {
    fn from(value: InsertInto<N>) -> Self {
        IrNode::InsertInto(value)
    }
}

#[cfg(test)]
impl<N> From<UpdateTable<N>> for IrNode<N> {
    fn from(value: UpdateTable<N>) -> Self {
        IrNode::UpdateTable(value)
    }
}

#[cfg(test)]
impl<N> From<DeleteFrom<N>> for IrNode<N> {
    fn from(value: DeleteFrom<N>) -> Self {
        IrNode::DeleteFrom(value)
    }
}

#[cfg(test)]
impl<N> From<Cluster<N>> for IrNode<N> {
    fn from(value: Cluster<N>) -> Self {
        IrNode::Cluster(value)
    }
}

#[cfg(test)]
impl<N> From<VacuumFull<N>> for IrNode<N> {
    fn from(value: VacuumFull<N>) -> Self {
        IrNode::VacuumFull(value)
    }
}

#[cfg(test)]
impl<N> From<Reindex<N>> for IrNode<N> {
    fn from(value: Reindex<N>) -> Self {
        IrNode::Reindex(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_in_different_schemas_resolves_to_distinct_relations() {
        let public_orders = QualifiedName::qualified("public", "orders");
        let audit_orders = QualifiedName::qualified("audit", "orders");
        assert_ne!(public_orders, audit_orders);
        assert_ne!(
            public_orders.resolve("public").relation(),
            audit_orders.resolve("public").relation()
        );
    }

    #[test]
    fn unqualified_and_default_qualified_names_resolve_to_equal_names() {
        use std::collections::HashSet;
        let unqualified = QualifiedName::unqualified("orders").resolve("public");
        let default_qualified = QualifiedName::qualified("public", "orders").resolve("public");
        assert_ne!(unqualified.display_name(), default_qualified.display_name());
        assert_eq!(unqualified, default_qualified);
        let set = HashSet::from([unqualified]);
        assert!(set.contains(&default_qualified));
    }

    #[test]
    fn test_display_unqualified() {
        let name = QualifiedName::unqualified("orders");
        assert_eq!(format!("{}", name), "orders");
    }

    #[test]
    fn test_display_qualified() {
        let name = QualifiedName::qualified("myschema", "orders");
        assert_eq!(format!("{}", name), "myschema.orders");
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let a = QualifiedName::qualified("public", "orders");
        let b = QualifiedName::qualified("public", "orders");
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }
}
