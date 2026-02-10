//! Intermediate Representation (IR) for SQL statements
//!
//! The IR layer decouples the parser from the rule engine. It represents
//! only the information needed for linting, not the full PostgreSQL AST.

use std::fmt;

/// A parsed SQL statement mapped to a high-level operation.
/// Each variant carries only the fields rules need — not the full AST.
#[derive(Debug, Clone, PartialEq)]
pub enum IrNode {
    CreateTable(CreateTable),
    AlterTable(AlterTable),
    CreateIndex(CreateIndex),
    DropIndex(DropIndex),
    DropTable(DropTable),
    /// SQL that parsed successfully but has no IR mapping (e.g., GRANT, COMMENT ON).
    /// Not an error — just not relevant to linting.
    Ignored {
        raw_sql: String,
    },
    /// SQL that failed to parse or is inherently opaque (DO $$ blocks, dynamic SQL).
    /// The replay engine uses `table_hint` to mark affected tables as incomplete.
    Unparseable {
        raw_sql: String,
        table_hint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub name: QualifiedName,
    pub columns: Vec<ColumnDef>,
    pub constraints: Vec<TableConstraint>,
    pub temporary: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterTable {
    pub name: QualifiedName,
    pub actions: Vec<AlterTableAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlterTableAction {
    AddColumn(ColumnDef),
    DropColumn {
        name: String,
    },
    AddConstraint(TableConstraint),
    AlterColumnType {
        column_name: String,
        new_type: TypeName,
        /// Only available if catalog provides it — not from the SQL itself.
        /// Rules that need old_type must look it up in the catalog.
        old_type: Option<TypeName>,
    },
    /// Catch-all for ALTER TABLE actions we parse but don't model.
    Other {
        description: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndex {
    pub index_name: Option<String>,
    pub table_name: QualifiedName,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
    pub concurrent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropIndex {
    pub index_name: String,
    pub concurrent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropTable {
    pub name: QualifiedName,
}

// --- Supporting types ---

/// Schema-qualified name. `schema` is None for unqualified references.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedName {
    pub schema: Option<String>,
    pub name: String,
}

impl QualifiedName {
    pub fn unqualified(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// Returns the name used for catalog lookup. Ignores schema for now
    /// (flat catalog). Future: schema-aware lookup.
    pub fn catalog_key(&self) -> &str {
        &self.name
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
pub enum TableConstraint {
    PrimaryKey {
        columns: Vec<String>,
    },
    ForeignKey {
        name: Option<String>,
        columns: Vec<String>,
        ref_table: QualifiedName,
        ref_columns: Vec<String>,
    },
    Unique {
        name: Option<String>,
        columns: Vec<String>,
    },
    Check {
        name: Option<String>,
        expression: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexColumn {
    pub name: String,
    // Future: ASC/DESC, NULLS FIRST/LAST, opclass. Not needed for v1.
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
