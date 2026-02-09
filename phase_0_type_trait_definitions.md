// ============================================================
// src/parser/ir.rs — Intermediate Representation
// ============================================================

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
    Ignored { raw_sql: String },
    /// SQL that failed to parse or is inherently opaque (DO $$ blocks, dynamic SQL).
    /// The replay engine uses `table_hint` to mark affected tables as incomplete.
    Unparseable { raw_sql: String, table_hint: Option<String> },
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
        Self { schema: None, name: name.into() }
    }

    pub fn qualified(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self { schema: Some(schema.into()), name: name.into() }
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
        Self { name: name.into().to_lowercase(), modifiers: vec![] }
    }

    pub fn with_modifiers(name: impl Into<String>, modifiers: Vec<i64>) -> Self {
        Self { name: name.into().to_lowercase(), modifiers }
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
    PrimaryKey { columns: Vec<String> },
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


// ============================================================
// src/input/mod.rs — Migration loading
// ============================================================

use std::path::{Path, PathBuf};

/// A single migration unit: one changeset (Liquibase) or one file (go-migrate).
#[derive(Debug, Clone)]
pub struct MigrationUnit {
    /// Identifier for ordering/logging. Changeset ID or filename.
    pub id: String,

    /// The SQL statements as IR nodes with source locations.
    pub statements: Vec<Located<IrNode>>,

    /// The source file to report findings against.
    pub source_file: PathBuf,

    /// Line offset in the source file where this unit starts.
    /// For raw SQL files this is 1. For Liquibase XML, it's the <changeSet> line.
    pub source_line_offset: usize,

    /// Whether this unit executes inside a transaction.
    /// Liquibase: derived from runInTransaction attribute.
    /// go-migrate: true by default unless explicitly disabled.
    pub run_in_transaction: bool,

    /// Is this a down/rollback migration?
    pub is_down: bool,
}

/// An ordered sequence of migration units representing the full history.
#[derive(Debug)]
pub struct MigrationHistory {
    pub units: Vec<MigrationUnit>,
}

/// Trait for migration loaders. Each input format implements this.
pub trait MigrationLoader {
    /// Load migrations from the given paths, in the configured order.
    fn load(&self, paths: &[PathBuf]) -> Result<MigrationHistory, LoadError>;
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("IO error reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },

    #[error("Parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("Liquibase bridge failed: {message}")]
    BridgeError { message: String },

    #[error("Configuration error: {message}")]
    Config { message: String },
}


// ============================================================
// src/catalog/types.rs — Table catalog
// ============================================================

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tables: HashMap<String, TableState>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_table(&self, name: &str) -> Option<&TableState> {
        self.tables.get(name)
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }

    pub fn insert_table(&mut self, table: TableState) {
        self.tables.insert(table.name.clone(), table);
    }

    pub fn remove_table(&mut self, name: &str) -> Option<TableState> {
        self.tables.remove(name)
    }

    pub fn tables(&self) -> impl Iterator<Item = &TableState> {
        self.tables.values()
    }
}

#[derive(Debug, Clone)]
pub struct TableState {
    pub name: String,
    pub columns: Vec<ColumnState>,
    pub indexes: Vec<IndexState>,
    pub constraints: Vec<ConstraintState>,
    pub has_primary_key: bool,
    /// True if an unparseable statement referenced this table.
    /// Rules should consider lowering confidence on findings for incomplete tables.
    pub incomplete: bool,
}

impl TableState {
    pub fn get_column(&self, name: &str) -> Option<&ColumnState> {
        self.columns.iter().find(|c| c.name == name)
    }

    pub fn get_column_mut(&mut self, name: &str) -> Option<&mut ColumnState> {
        self.columns.iter_mut().find(|c| c.name == name)
    }

    pub fn remove_column(&mut self, name: &str) {
        self.columns.retain(|c| c.name != name);
        // Also remove indexes that reference this column
        self.indexes.retain(|idx| !idx.columns.iter().any(|c| c == name));
    }

    /// Check if any index on this table covers the given columns as a prefix.
    /// Column order matters: [a, b] is covered by [a, b, c] but not [b, a].
    pub fn has_covering_index(&self, fk_columns: &[String]) -> bool {
        self.indexes.iter().any(|idx| {
            idx.columns.len() >= fk_columns.len()
                && idx.columns.iter().zip(fk_columns).all(|(ic, fc)| ic == fc)
        })
    }

    /// Check if this table has a UNIQUE constraint where all columns are NOT NULL.
    /// Used for PGM005 (UNIQUE NOT NULL substitute for PK).
    pub fn has_unique_not_null(&self) -> bool {
        self.constraints.iter().any(|c| {
            if let ConstraintState::Unique { columns, .. } = c {
                columns.iter().all(|col_name| {
                    self.get_column(col_name)
                        .map(|col| !col.nullable)
                        .unwrap_or(false)
                })
            } else {
                false
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ColumnState {
    pub name: String,
    pub type_name: TypeName, // Reuses the IR type
    pub nullable: bool,
    pub has_default: bool,
    pub default_expr: Option<DefaultExpr>, // Reuses the IR type
}

#[derive(Debug, Clone)]
pub struct IndexState {
    pub name: String,
    /// Column names in index order. Order matters for prefix matching.
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone)]
pub enum ConstraintState {
    PrimaryKey {
        columns: Vec<String>,
    },
    ForeignKey {
        name: Option<String>,
        columns: Vec<String>,
        ref_table: String,
        ref_columns: Vec<String>,
    },
    Unique {
        name: Option<String>,
        columns: Vec<String>,
    },
    Check {
        name: Option<String>,
    },
}


// ============================================================
// src/catalog/replay.rs — Replay engine
// ============================================================

/// Apply a single migration unit's IR nodes to mutate the catalog.
/// Called by the pipeline for each unit in order.
pub fn apply(catalog: &mut Catalog, unit: &MigrationUnit);

// The pipeline (main.rs) drives the single-pass replay:
//
// for unit in history.units:
//     if unit is in changed_files:
//         let catalog_before = catalog.clone();
//         apply(&mut catalog, &unit);
//         lint(&unit, &catalog_before, &catalog);
//     else:
//         apply(&mut catalog, &unit);
//
// No dual-replay. No replay_until. One clone per linted unit.


// ============================================================
// src/rules/mod.rs — Rule engine
// ============================================================

use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Minor,
    Major,
    Critical,
    Blocker,
}

impl Severity {
    /// Parse from config string. Case-insensitive.
    pub fn from_str(s: &str) -> Option<Self>;

    /// SonarQube severity string.
    pub fn sonarqube_str(&self) -> &'static str;
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
}

/// Context available to rules during linting.
pub struct LintContext<'a> {
    /// The catalog state BEFORE the current unit was applied.
    /// Clone taken just before apply(). Used by PGM001/002 to check
    /// if a table is pre-existing.
    pub catalog_before: &'a Catalog,

    /// The catalog state AFTER the current unit was applied.
    /// Used for post-file checks (PGM003, PGM004, PGM005).
    pub catalog_after: &'a Catalog,

    /// Set of table names created in the current set of changed files.
    /// Built incrementally during the single-pass replay: when a changed
    /// file contains a CreateTable, add it to this set before linting
    /// subsequent changed files.
    pub tables_created_in_change: &'a HashSet<String>,

    /// Whether this migration unit runs in a transaction.
    pub run_in_transaction: bool,

    /// Whether this is a down/rollback migration.
    pub is_down: bool,

    /// The source file being linted.
    pub file: &'a PathBuf,
}

/// Trait that every rule implements.
pub trait Rule: Send + Sync {
    /// Stable rule identifier: "PGM001", "PGM002", etc.
    fn id(&self) -> &'static str;

    /// Default severity for this rule.
    fn default_severity(&self) -> Severity;

    /// Human-readable short description.
    fn description(&self) -> &'static str;

    /// Detailed explanation for --explain. Includes failure mode, example, fix.
    fn explain(&self) -> &'static str;

    /// Run the rule against a single migration unit.
    ///
    /// `statements` are the IR nodes for the unit being linted.
    /// `ctx` provides catalog state and changed-file context.
    ///
    /// Returns findings with severity set to `default_severity()`.
    /// The caller handles down-migration severity capping and suppression filtering.
    fn check(
        &self,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding>;
}

/// Registry of all rules.
pub struct RuleRegistry {
    rules: Vec<Box<dyn Rule>>,
}

impl RuleRegistry {
    pub fn new() -> Self;

    /// Register all built-in rules.
    pub fn register_defaults(&mut self);

    /// Get a rule by ID (for --explain).
    pub fn get(&self, id: &str) -> Option<&dyn Rule>;

    /// Iterate all rules.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Rule>;
}


// ============================================================
// src/suppress.rs — Suppression handling
// ============================================================

use std::collections::HashSet;

/// Parsed suppression directives from a single file.
#[derive(Debug, Default)]
pub struct Suppressions {
    /// Rules suppressed for the entire file.
    file_level: HashSet<String>,

    /// Rules suppressed for a specific line (the statement after the comment).
    /// Key: line number of the statement (not the comment).
    line_level: HashMap<usize, HashSet<String>>,
}

impl Suppressions {
    /// Check if a rule is suppressed at a given line.
    pub fn is_suppressed(&self, rule_id: &str, statement_line: usize) -> bool;
}

/// Parse suppression comments from SQL source text.
/// Must be called before IR parsing (operates on raw text).
pub fn parse_suppressions(source: &str) -> Suppressions;


// ============================================================
// src/output/mod.rs — Output reporters
// ============================================================

use std::path::Path;

/// Trait for output format reporters.
pub trait Reporter {
    /// Write findings to the given output directory.
    /// The filename is determined by the reporter (e.g., "findings.sarif").
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("IO error writing report: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Text reporter also supports writing to stdout (for --format text).
pub struct TextReporter {
    pub use_stdout: bool,
}

pub struct SarifReporter;
pub struct SonarQubeReporter;


// ============================================================
// src/catalog/builder.rs — Test harness (Phase 0 priority)
// ============================================================

#[cfg(test)]
pub struct CatalogBuilder {
    catalog: Catalog,
}

#[cfg(test)]
impl CatalogBuilder {
    pub fn new() -> Self {
        Self { catalog: Catalog::new() }
    }

    pub fn table(mut self, name: &str, f: impl FnOnce(&mut TableBuilder)) -> Self {
        let mut builder = TableBuilder::new(name);
        f(&mut builder);
        self.catalog.insert_table(builder.build());
        self
    }

    pub fn build(self) -> Catalog {
        self.catalog
    }
}

#[cfg(test)]
pub struct TableBuilder {
    state: TableState,
}

#[cfg(test)]
impl TableBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            state: TableState {
                name: name.to_string(),
                columns: vec![],
                indexes: vec![],
                constraints: vec![],
                has_primary_key: false,
                incomplete: false,
            },
        }
    }

    pub fn column(&mut self, name: &str, type_name: &str, nullable: bool) -> &mut Self {
        self.state.columns.push(ColumnState {
            name: name.to_string(),
            type_name: TypeName::simple(type_name),
            nullable,
            has_default: false,
            default_expr: None,
        });
        self
    }

    pub fn column_with_default(
        &mut self,
        name: &str,
        type_name: &str,
        nullable: bool,
        default: DefaultExpr,
    ) -> &mut Self {
        self.state.columns.push(ColumnState {
            name: name.to_string(),
            type_name: TypeName::simple(type_name),
            nullable,
            has_default: true,
            default_expr: Some(default),
        });
        self
    }

    pub fn index(&mut self, name: &str, columns: &[&str], unique: bool) -> &mut Self {
        self.state.indexes.push(IndexState {
            name: name.to_string(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique,
        });
        self
    }

    pub fn pk(&mut self, columns: &[&str]) -> &mut Self {
        self.state.has_primary_key = true;
        self.state.constraints.push(ConstraintState::PrimaryKey {
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn fk(
        &mut self,
        name: &str,
        columns: &[&str],
        ref_table: &str,
        ref_columns: &[&str],
    ) -> &mut Self {
        self.state.constraints.push(ConstraintState::ForeignKey {
            name: Some(name.to_string()),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            ref_table: ref_table.to_string(),
            ref_columns: ref_columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn unique(&mut self, name: &str, columns: &[&str]) -> &mut Self {
        self.state.constraints.push(ConstraintState::Unique {
            name: Some(name.to_string()),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn incomplete(&mut self) -> &mut Self {
        self.state.incomplete = true;
        self
    }

    pub fn build(self) -> TableState {
        self.state
    }
}