//! Table catalog types
//!
//! The catalog represents the database schema state at a point in migration history.
//! It's built by replaying migrations in order.

use crate::parser::ir::{DefaultExpr, TypeName};
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
        // Also remove indexes that reference this column
        self.indexes
            .retain(|idx| !idx.columns.iter().any(|c| c == name));

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
    pub fn has_covering_index(&self, fk_columns: &[String]) -> bool {
        self.indexes.iter().any(|idx| {
            idx.columns.len() >= fk_columns.len()
                && idx.columns.iter().zip(fk_columns).all(|(ic, fc)| ic == fc)
        })
    }

    /// Check if the given columns are already covered by a unique index or
    /// UNIQUE constraint. Used by PGM012 to determine whether `ADD PRIMARY KEY`
    /// can rely on pre-existing uniqueness enforcement.
    ///
    /// For indexes: requires exact column match (same columns, same order).
    /// For constraints: requires exact column set match (order-independent).
    pub fn has_unique_covering(&self, columns: &[String]) -> bool {
        // Check unique indexes: exact column match (same order, same length)
        let index_match = self.indexes.iter().any(|idx| {
            idx.unique
                && idx.columns.len() == columns.len()
                && idx.columns.iter().zip(columns).all(|(ic, c)| ic == c)
        });
        if index_match {
            return true;
        }

        // Check UNIQUE constraints: exact column set match (order-independent)
        self.constraints.iter().any(|c| {
            if let ConstraintState::Unique {
                columns: constraint_cols,
                ..
            } = c
            {
                if constraint_cols.len() != columns.len() {
                    return false;
                }
                let mut sorted_constraint: Vec<&String> = constraint_cols.iter().collect();
                let mut sorted_target: Vec<&String> = columns.iter().collect();
                sorted_constraint.sort();
                sorted_target.sort();
                sorted_constraint == sorted_target
            } else {
                false
            }
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

    /// Returns all constraints that involve the given column.
    pub fn constraints_involving_column(&self, col: &str) -> Vec<&ConstraintState> {
        self.constraints
            .iter()
            .filter(|c| c.involves_column(col))
            .collect()
    }

    /// Returns all indexes whose column list includes the given column.
    pub fn indexes_involving_column(&self, col: &str) -> Vec<&IndexState> {
        self.indexes
            .iter()
            .filter(|idx| idx.columns.iter().any(|c| c == col))
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
