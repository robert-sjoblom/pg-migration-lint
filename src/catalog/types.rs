//! Table catalog types
//!
//! The catalog represents the database schema state at a point in migration history.
//! It's built by replaying migrations in order.

use crate::parser::ir::{DefaultExpr, IndexColumn, TypeName};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tables: HashMap<String, TableState>,
    /// Reverse lookup: index name → owning table key.
    index_to_table: HashMap<String, String>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_table(&self, name: &str) -> Option<&TableState> {
        self.tables.get(name)
    }

    pub fn get_table_mut(&mut self, name: &str) -> Option<&mut TableState> {
        self.tables.get_mut(name)
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }

    pub fn insert_table(&mut self, table: TableState) {
        // Register all indexes in the reverse lookup.
        for idx in &table.indexes {
            if !idx.name.is_empty() {
                self.index_to_table
                    .insert(idx.name.clone(), table.name.clone());
            }
        }
        self.tables.insert(table.name.clone(), table);
    }

    pub fn remove_table(&mut self, name: &str) -> Option<TableState> {
        if let Some(table) = self.tables.remove(name) {
            for idx in &table.indexes {
                self.index_to_table.remove(&idx.name);
            }
            Some(table)
        } else {
            None
        }
    }

    /// Register an index in the reverse lookup.
    pub fn register_index(&mut self, index_name: &str, table_key: &str) {
        if !index_name.is_empty() {
            self.index_to_table
                .insert(index_name.to_string(), table_key.to_string());
        }
    }

    /// Remove an index from the reverse lookup.
    pub fn unregister_index(&mut self, index_name: &str) {
        self.index_to_table.remove(index_name);
    }

    /// Look up which table owns a given index. O(1).
    pub fn table_for_index(&self, index_name: &str) -> Option<&str> {
        self.index_to_table.get(index_name).map(|s| s.as_str())
    }

    /// Look up an index by name across all tables. Returns the `IndexState` if found.
    pub fn get_index(&self, index_name: &str) -> Option<&IndexState> {
        let table_key = self.index_to_table.get(index_name)?;
        let table = self.tables.get(table_key)?;
        table.indexes.iter().find(|idx| idx.name == index_name)
    }

    pub fn tables(&self) -> impl Iterator<Item = &TableState> {
        self.tables.values()
    }
}

#[derive(Debug, Clone)]
pub struct TableState {
    pub name: String,
    /// User-facing name (omits synthetic schema prefix).
    pub display_name: String,
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
        // Remove indexes that reference this column — either as a plain column entry
        // or inside an expression (e.g. `lower(email)` references `email`).
        self.indexes.retain(|idx| !idx.references_column(name));

        // Remove constraints referencing the dropped column.
        // PostgreSQL drops the entire constraint, not just the column from it.
        self.constraints.retain(|c| match c {
            ConstraintState::PrimaryKey { columns }
            | ConstraintState::ForeignKey { columns, .. }
            | ConstraintState::Unique { columns, .. } => !columns.iter().any(|c| c == name),
            ConstraintState::Check { .. } => true,
        });

        // Recalculate has_primary_key in case the PK was removed.
        self.has_primary_key = self
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::PrimaryKey { .. }));
    }

    /// Check if any index on this table covers the given columns as a prefix.
    /// Column order matters: [a, b] is covered by [a, b, c] but not [b, a].
    ///
    /// Partial indexes are skipped entirely — they cannot satisfy FK coverage
    /// because they only index a subset of rows. An expression entry at
    /// position N stops prefix matching, since expressions cannot match an
    /// FK column name (e.g. FK `(a, b)` is NOT covered by index `(a, lower(b))`).
    pub fn has_covering_index(&self, fk_columns: &[String]) -> bool {
        self.indexes.iter().any(|idx| {
            if idx.is_partial() {
                return false;
            }
            idx.entries.len() >= fk_columns.len()
                && idx
                    .entries
                    .iter()
                    .zip(fk_columns)
                    .all(|(entry, fc)| matches!(entry, IndexEntry::Column(name) if name == fc))
        })
    }

    /// Check if this table has a UNIQUE constraint or unique index where all
    /// columns are NOT NULL. Used for PGM503 (UNIQUE NOT NULL substitute for PK).
    ///
    /// Partial indexes and expression indexes are excluded — they cannot serve
    /// as a PK substitute.
    pub fn has_unique_not_null(&self) -> bool {
        let constraint_match = self.constraints.iter().any(|c| {
            if let ConstraintState::Unique { columns, .. } = c {
                columns.iter().all(|col_name| {
                    self.get_column(col_name)
                        .map(|col| !col.nullable)
                        .unwrap_or(false)
                })
            } else {
                false
            }
        });
        if constraint_match {
            return true;
        }
        self.indexes.iter().any(|idx| {
            idx.unique
                && !idx.is_partial()
                && !idx.has_expressions()
                && idx.column_names().all(|col_name| {
                    self.get_column(col_name)
                        .map(|col| !col.nullable)
                        .unwrap_or(false)
                })
        })
    }

    /// Returns all constraints that involve the given column.
    pub fn constraints_involving_column(&self, col: &str) -> Vec<&ConstraintState> {
        self.constraints
            .iter()
            .filter(|c| c.involves_column(col))
            .collect()
    }

    /// Returns all indexes that reference the given column — either as a plain
    /// column entry or inside an expression (e.g. `lower(col)` references `col`).
    pub fn indexes_involving_column(&self, col: &str) -> Vec<&IndexState> {
        self.indexes
            .iter()
            .filter(|idx| idx.references_column(col))
            .collect()
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

/// An element in an index, mirroring [`crate::parser::ir::IndexColumn`]
/// at the catalog level.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexEntry {
    /// Plain column reference.
    Column(String),
    /// Expression index element (deparsed SQL text) with extracted column references.
    Expression {
        text: String,
        referenced_columns: Vec<String>,
    },
}

impl IndexEntry {
    /// Returns the column name if this is a plain column, or `None` for expressions.
    pub fn column_name(&self) -> Option<&str> {
        match self {
            Self::Column(n) => Some(n),
            Self::Expression { .. } => None,
        }
    }
}

impl From<&IndexColumn> for IndexEntry {
    fn from(ic: &IndexColumn) -> Self {
        match ic {
            IndexColumn::Column(name) => Self::Column(name.clone()),
            IndexColumn::Expression {
                text,
                referenced_columns,
            } => Self::Expression {
                text: text.clone(),
                referenced_columns: referenced_columns.clone(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexState {
    pub name: String,
    /// Index entries in definition order. Order matters for prefix matching.
    pub entries: Vec<IndexEntry>,
    pub unique: bool,
    /// Deparsed WHERE clause for partial indexes (e.g. `"active = true"`).
    pub where_clause: Option<String>,
}

impl IndexState {
    /// Iterator over plain column names, skipping expression entries.
    pub fn column_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|e| e.column_name())
    }

    /// True if any entry is an expression (not a plain column).
    pub fn has_expressions(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, IndexEntry::Expression { .. }))
    }

    /// True if any entry (plain column or expression) references the given column.
    pub fn references_column(&self, col: &str) -> bool {
        self.entries.iter().any(|e| match e {
            IndexEntry::Column(name) => name == col,
            IndexEntry::Expression {
                referenced_columns, ..
            } => referenced_columns.iter().any(|c| c == col),
        })
    }

    /// True if this is a partial index (has a WHERE clause).
    pub fn is_partial(&self) -> bool {
        self.where_clause.is_some()
    }
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
        /// User-facing referenced table name (omits synthetic schema prefix).
        ref_table_display: String,
        ref_columns: Vec<String>,
        not_valid: bool,
    },
    Unique {
        name: Option<String>,
        columns: Vec<String>,
    },
    Check {
        name: Option<String>,
        not_valid: bool,
    },
}

impl ConstraintState {
    /// Returns true if this constraint involves the given column name.
    pub fn involves_column(&self, col: &str) -> bool {
        match self {
            ConstraintState::PrimaryKey { columns }
            | ConstraintState::ForeignKey { columns, .. }
            | ConstraintState::Unique { columns, .. } => columns.iter().any(|c| c == col),
            ConstraintState::Check { .. } => false,
        }
    }
}
