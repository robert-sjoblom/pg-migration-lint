//! Intermediate Representation (IR) for SQL statements
//!
//! The IR layer decouples the parser from the rule engine. It represents
//! only the information needed for linting, not the full PostgreSQL AST.

use std::fmt;
use std::hash::{Hash, Hasher};

/// A parsed SQL statement mapped to a high-level operation.
/// Each variant carries only the fields rules need — not the full AST.
#[derive(Debug, Clone, PartialEq)]
pub enum IrNode {
    CreateTable(CreateTable),
    AlterTable(AlterTable),
    CreateIndex(CreateIndex),
    DropIndex(DropIndex),
    DropTable(DropTable),
    /// Rename an existing table. pg_query emits `RenameStmt`, not `AlterTableStmt`.
    RenameTable {
        name: QualifiedName,
        new_name: String,
    },
    /// Rename a column on an existing table. pg_query emits `RenameStmt`.
    RenameColumn {
        table: QualifiedName,
        old_name: String,
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
    /// SET NOT NULL on an existing column (requires ACCESS EXCLUSIVE lock).
    SetNotNull {
        column_name: String,
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
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropTable {
    pub name: QualifiedName,
    pub if_exists: bool,
}

// --- Supporting types ---

/// Schema-qualified name. `schema` is None for unqualified references.
///
/// `PartialEq`, `Eq`, and `Hash` are implemented manually on `schema` + `name`
/// only, excluding the pre-computed `catalog_key` cache and `schema_is_default`.
#[derive(Debug, Clone)]
pub struct QualifiedName {
    pub schema: Option<String>,
    pub name: String,
    /// Pre-computed lookup key: `"schema.name"` when qualified, `"name"` when not.
    /// Updated by constructors and `set_default_schema()`.
    catalog_key: String,
    /// True when the schema was assigned by normalization, not by the user.
    /// Used to suppress the schema prefix in user-facing messages.
    schema_is_default: bool,
}

impl PartialEq for QualifiedName {
    fn eq(&self, other: &Self) -> bool {
        self.schema == other.schema && self.name == other.name
    }
}

impl Eq for QualifiedName {}

impl Hash for QualifiedName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.schema.hash(state);
        self.name.hash(state);
    }
}

impl QualifiedName {
    pub fn unqualified(name: impl Into<String>) -> Self {
        let name = name.into();
        let catalog_key = name.clone();
        Self {
            schema: None,
            name,
            catalog_key,
            schema_is_default: false,
        }
    }

    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        let schema = schema.into();
        let name = name.into();
        let catalog_key = format!("{}.{}", schema, name);
        Self {
            schema: Some(schema),
            name,
            catalog_key,
            schema_is_default: false,
        }
    }

    /// Returns the pre-computed key used for catalog lookup.
    ///
    /// Before normalization this returns just the table name for unqualified
    /// references. After `set_default_schema()` has been called, all names
    /// have an explicit schema and this returns `"schema.name"`.
    pub fn catalog_key(&self) -> &str {
        &self.catalog_key
    }

    /// Assign a default schema to an unqualified name and recompute the catalog key.
    ///
    /// If the name is already schema-qualified this is a no-op.
    pub fn set_default_schema(&mut self, default: &str) {
        if self.schema.is_none() {
            self.schema = Some(default.to_string());
            self.catalog_key = format!("{}.{}", default, self.name);
            self.schema_is_default = true;
        }
    }

    /// Returns the user-facing name: just `name` if the schema was synthesized
    /// by normalization, or `schema.name` if the user wrote it explicitly.
    pub fn display_name(&self) -> String {
        if self.schema_is_default {
            self.name.clone()
        } else {
            match &self.schema {
                Some(s) => format!("{}.{}", s, self.name),
                None => self.name.clone(),
            }
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
pub enum TableConstraint {
    PrimaryKey {
        columns: Vec<String>,
        /// Index name from `USING INDEX` clause, e.g. `ADD PRIMARY KEY USING INDEX idx`.
        /// When present, `columns` will be empty (PostgreSQL derives them from the index).
        using_index: Option<String>,
    },
    ForeignKey {
        name: Option<String>,
        columns: Vec<String>,
        ref_table: QualifiedName,
        ref_columns: Vec<String>,
        not_valid: bool,
    },
    Unique {
        name: Option<String>,
        columns: Vec<String>,
        /// Index name from `USING INDEX` clause, e.g. `ADD UNIQUE USING INDEX idx`.
        /// When present, `columns` will be empty (PostgreSQL derives them from the index).
        using_index: Option<String>,
    },
    Check {
        name: Option<String>,
        expression: String,
        not_valid: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unqualified_catalog_key() {
        let name = QualifiedName::unqualified("orders");
        assert_eq!(name.catalog_key(), "orders");
    }

    #[test]
    fn test_qualified_catalog_key() {
        let name = QualifiedName::qualified("myschema", "orders");
        assert_eq!(name.catalog_key(), "myschema.orders");
    }

    #[test]
    fn test_set_default_schema_on_unqualified() {
        let mut name = QualifiedName::unqualified("orders");
        name.set_default_schema("public");
        assert_eq!(name.schema, Some("public".to_string()));
        assert_eq!(name.catalog_key(), "public.orders");
    }

    #[test]
    fn test_set_default_schema_noop_on_qualified() {
        let mut name = QualifiedName::qualified("myschema", "orders");
        name.set_default_schema("public");
        // Should not change — already qualified
        assert_eq!(name.schema, Some("myschema".to_string()));
        assert_eq!(name.catalog_key(), "myschema.orders");
    }

    #[test]
    fn test_different_schemas_are_distinct() {
        let a = QualifiedName::qualified("public", "orders");
        let b = QualifiedName::qualified("audit", "orders");
        assert_ne!(a, b);
        assert_ne!(a.catalog_key(), b.catalog_key());
    }

    #[test]
    fn test_equality_ignores_catalog_key_cache() {
        // Two names with same schema + name are equal even though
        // catalog_key is a derived field.
        let a = QualifiedName::qualified("public", "orders");
        let b = QualifiedName::qualified("public", "orders");
        assert_eq!(a, b);
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
    fn test_display_after_set_default_schema() {
        let mut name = QualifiedName::unqualified("orders");
        name.set_default_schema("public");
        // Display should now show the schema since it was set
        assert_eq!(format!("{}", name), "public.orders");
    }

    #[test]
    fn test_display_name_unqualified() {
        let name = QualifiedName::unqualified("orders");
        assert_eq!(name.display_name(), "orders");
    }

    #[test]
    fn test_display_name_qualified() {
        let name = QualifiedName::qualified("myschema", "orders");
        assert_eq!(name.display_name(), "myschema.orders");
    }

    #[test]
    fn test_display_name_after_set_default_schema() {
        let mut name = QualifiedName::unqualified("orders");
        name.set_default_schema("public");
        // display_name omits the synthetic schema
        assert_eq!(name.display_name(), "orders");
        // Display still shows the full form
        assert_eq!(format!("{}", name), "public.orders");
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
