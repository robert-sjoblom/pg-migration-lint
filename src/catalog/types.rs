//! Table catalog types
//!
//! The catalog represents the database schema state at a point in migration history.
//! It's built by replaying migrations in order.

use crate::parser::ir::{DefaultExpr, IndexColumn, PartitionStrategy, TypeName};
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

    pub(crate) fn get_table_mut(&mut self, name: &str) -> Option<&mut TableState> {
        self.tables.get_mut(name)
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }

    pub(crate) fn insert_table(&mut self, table: TableState) {
        // Register all indexes in the reverse lookup.
        for idx in &table.indexes {
            if !idx.name.is_empty() {
                self.index_to_table
                    .insert(idx.name.clone(), table.name.clone());
            }
        }
        self.tables.insert(table.name.clone(), table);
    }

    pub(crate) fn remove_table(&mut self, name: &str) -> Option<TableState> {
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
    pub(crate) fn register_index(&mut self, index_name: &str, table_key: &str) {
        if !index_name.is_empty() {
            self.index_to_table
                .insert(index_name.to_string(), table_key.to_string());
        }
    }

    /// Remove an index from the reverse lookup.
    pub(crate) fn unregister_index(&mut self, index_name: &str) {
        self.index_to_table.remove(index_name);
    }

    /// Look up which table owns a given index. O(1).
    pub(crate) fn table_for_index(&self, index_name: &str) -> Option<&str> {
        self.index_to_table.get(index_name).map(|s| s.as_str())
    }

    /// Look up an index by name across all tables. Returns the `IndexState` if found.
    pub(crate) fn get_index(&self, index_name: &str) -> Option<&IndexState> {
        let table_key = self.index_to_table.get(index_name)?;
        let table = self.tables.get(table_key)?;
        table.indexes.iter().find(|idx| idx.name == index_name)
    }

    pub fn tables(&self) -> impl Iterator<Item = &TableState> {
        self.tables.values()
    }

    /// Returns the catalog keys of all partition children of the given parent.
    ///
    /// Computed on demand by scanning tables with matching `parent_table`.
    /// Partition queries are rare (only a few rules check them), so the O(n)
    /// scan is negligible compared to maintaining a bidirectional map.
    pub(crate) fn get_partition_children(&self, key: &str) -> Vec<String> {
        self.tables
            .values()
            .filter(|t| t.parent_table.as_deref() == Some(key))
            .map(|t| t.name.clone())
            .collect()
    }

    /// Returns `true` if the given table is a partition child (has a `parent_table`).
    #[cfg(test)]
    pub(crate) fn is_partition_child(&self, key: &str) -> bool {
        self.tables
            .get(key)
            .and_then(|t| t.parent_table.as_ref())
            .is_some()
    }
}

/// Partition key specification stored in the catalog.
#[derive(Debug, Clone)]
pub struct PartitionByInfo {
    pub strategy: PartitionStrategy,
    pub columns: Vec<String>,
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
    /// True if this table uses `PARTITION BY` (is a partitioned parent table).
    pub is_partitioned: bool,
    /// Partition strategy and columns, if this table is partitioned.
    pub partition_by: Option<PartitionByInfo>,
    /// Catalog key of the parent table, if this table is a partition child.
    pub parent_table: Option<String>,
}

impl TableState {
    pub fn get_column(&self, name: &str) -> Option<&ColumnState> {
        self.columns.iter().find(|c| c.name == name)
    }

    pub(crate) fn get_column_mut(&mut self, name: &str) -> Option<&mut ColumnState> {
        self.columns.iter_mut().find(|c| c.name == name)
    }

    pub(crate) fn remove_column(&mut self, name: &str) {
        self.columns.retain(|c| c.name != name);
        // Remove indexes that reference this column — either as a plain column entry
        // or inside an expression (e.g. `lower(email)` references `email`).
        self.indexes.retain(|idx| !idx.references_column(name));

        // Remove constraints referencing the dropped column.
        // PostgreSQL drops the entire constraint, not just the column from it.
        self.constraints.retain(|c| match c {
            ConstraintState::PrimaryKey { columns, .. }
            | ConstraintState::ForeignKey { columns, .. }
            | ConstraintState::Unique { columns, .. } => !columns.iter().any(|c| c == name),
            ConstraintState::Check { expression, .. } => {
                !expression_mentions_column(expression, name)
            }
            // TODO: EXCLUDE constraints do reference columns (e.g. `room WITH =`),
            // but the IR does not capture them. PostgreSQL drops EXCLUDE constraints
            // on column drop, so this `true` makes the catalog diverge. Fix when
            // the IR is expanded to track EXCLUDE element columns.
            ConstraintState::Exclude { .. } => true,
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
    /// Skipped indexes:
    /// - Partial indexes — they only index a subset of rows.
    /// - ON ONLY indexes — they don't cover partition children.
    /// - Non-B-tree indexes (GIN, GiST, BRIN, hash) — only B-tree indexes
    ///   support the ordered lookups PostgreSQL uses for FK enforcement.
    ///
    /// An expression entry at position N stops prefix matching, since
    /// expressions cannot match an FK column name (e.g. FK `(a, b)` is NOT
    /// covered by index `(a, lower(b))`).
    pub fn has_covering_index(&self, fk_columns: &[String]) -> bool {
        self.indexes.iter().any(|idx| {
            if idx.is_partial() || idx.only || !idx.is_btree() {
                return false;
            }
            idx.entries.len() >= fk_columns.len()
                && idx
                    .entries
                    .iter()
                    .zip(fk_columns)
                    .all(|(entry, fc)| matches!(entry, IndexColumn::Column(name) if name == fc))
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
                && idx.is_btree()
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

    /// Returns true if any CHECK constraint on this table references all of
    /// the given column names. Used by PGM005 to verify that a CHECK is
    /// relevant to the partition bound rather than an unrelated constraint.
    pub fn has_check_referencing_columns(&self, columns: &[String]) -> bool {
        self.constraints.iter().any(|c| {
            if let ConstraintState::Check {
                expression,
                not_valid,
                ..
            } = c
            {
                // NOT VALID CHECKs don't skip the partition scan — PostgreSQL
                // requires the constraint to be validated before ATTACH PARTITION
                // can skip the full table verification.
                if *not_valid {
                    return false;
                }
                columns
                    .iter()
                    .all(|col| expression_mentions_column(expression, col))
            } else {
                false
            }
        })
    }
}

/// Check if an expression text contains a column name as an identifier.
///
/// Splits on non-identifier characters and checks for an exact token match.
/// This avoids false positives like `ts` matching `timestamp`.
fn expression_mentions_column(expression: &str, column: &str) -> bool {
    expression
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|token| token == column)
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
    /// Index entries in definition order. Order matters for prefix matching.
    pub entries: Vec<IndexColumn>,
    pub unique: bool,
    /// Deparsed WHERE clause for partial indexes (e.g. `"active = true"`).
    pub where_clause: Option<String>,
    /// `CREATE INDEX ON ONLY parent_table` — index exists only on the parent,
    /// not propagated to partitions. Flipped to `false` by `ALTER INDEX ATTACH PARTITION`.
    pub only: bool,
    /// Index access method: `"btree"` (default), `"gin"`, `"gist"`, `"hash"`, `"brin"`.
    pub access_method: String,
}

impl IndexState {
    /// Default index access method in PostgreSQL.
    pub const DEFAULT_ACCESS_METHOD: &str = "btree";

    /// Iterator over plain column names, skipping expression entries.
    pub fn column_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|e| e.column_name())
    }

    /// True if any entry is an expression (not a plain column).
    pub fn has_expressions(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, IndexColumn::Expression { .. }))
    }

    /// True if any entry (plain column or expression) references the given column.
    pub fn references_column(&self, col: &str) -> bool {
        self.entries.iter().any(|e| e.references_column(col))
    }

    /// True if this is a partial index (has a WHERE clause).
    pub fn is_partial(&self) -> bool {
        self.where_clause.is_some()
    }

    /// True if this index uses the B-tree access method.
    /// Only B-tree indexes can serve FK lookups in PostgreSQL.
    pub fn is_btree(&self) -> bool {
        self.access_method == Self::DEFAULT_ACCESS_METHOD
    }
}

#[derive(Debug, Clone)]
pub enum ConstraintState {
    PrimaryKey {
        name: Option<String>,
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
        /// Index name from `USING INDEX` clause. When the constraint is dropped,
        /// this index must also be removed (its name may differ from the constraint name).
        using_index: Option<String>,
    },
    Check {
        name: Option<String>,
        expression: String,
        not_valid: bool,
    },
    Exclude {
        name: Option<String>,
    },
}

impl ConstraintState {
    /// Returns true if this constraint involves the given column name.
    pub fn involves_column(&self, col: &str) -> bool {
        match self {
            ConstraintState::PrimaryKey { columns, .. }
            | ConstraintState::ForeignKey { columns, .. }
            | ConstraintState::Unique { columns, .. } => columns.iter().any(|c| c == col),
            ConstraintState::Check { expression, .. } => {
                expression_mentions_column(expression, col)
            }
            ConstraintState::Exclude { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::PartitionStrategy;
    use rstest::rstest;

    #[test]
    fn test_is_partition_child() {
        let catalog = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .table("standalone", |t| {
                t.column("id", "integer", false);
            })
            .build();

        assert!(catalog.is_partition_child("child"));
        assert!(!catalog.is_partition_child("parent"));
        assert!(!catalog.is_partition_child("standalone"));
        assert!(!catalog.is_partition_child("nonexistent"));
    }

    #[test]
    fn test_remove_table_returns_none_for_nonexistent() {
        let mut catalog = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false);
            })
            .build();

        assert!(catalog.remove_table("nonexistent").is_none());
    }

    #[test]
    fn test_get_partition_children_computed_on_demand() {
        let catalog = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .build();

        let children = catalog.get_partition_children("parent");
        assert_eq!(children, vec!["child"]);
        assert!(catalog.get_partition_children("nonexistent").is_empty());
    }

    #[rstest]
    #[case::simple("(ts >= '2024-01-01')", "ts", true)]
    #[case::cast("ts::date >= '2024-01-01'", "ts", true)]
    #[case::no_match("(amount > 0)", "ts", false)]
    #[case::substring_no_false_positive("(id_type = 'foo')", "id", false)]
    #[case::empty_expression("", "ts", false)]
    #[case::function_call("date_trunc('month', ts)", "ts", true)]
    fn test_expression_mentions_column(
        #[case] expr: &str,
        #[case] col: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(expression_mentions_column(expr, col), expected);
    }

    #[rstest]
    #[case::matches("amount", true)]
    #[case::no_match("id", false)]
    fn test_involves_column_check_constraint(#[case] col: &str, #[case] expected: bool) {
        let constraint = ConstraintState::Check {
            name: Some("chk_positive".to_string()),
            expression: "(amount > 0)".to_string(),
            not_valid: false,
        };
        assert_eq!(
            constraint.involves_column(col),
            expected,
            "CHECK constraint with expression '(amount > 0)' involves_column(\"{col}\") should be {expected}"
        );
    }

    #[test]
    fn test_remove_column_drops_check_referencing_column() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("amount", "integer", false)
                    .check_constraint(Some("chk_positive"), "(amount > 0)", false);
            })
            .build();

        let mut table = catalog.get_table("orders").unwrap().clone();
        assert_eq!(
            table.constraints.len(),
            1,
            "should start with one CHECK constraint"
        );

        table.remove_column("amount");

        assert!(
            table.constraints.is_empty(),
            "CHECK constraint referencing 'amount' should be removed after dropping 'amount'"
        );
    }

    #[test]
    fn test_remove_column_keeps_unrelated_check() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("amount", "integer", false)
                    .column("extra", "text", true)
                    .check_constraint(Some("chk_positive"), "(amount > 0)", false);
            })
            .build();

        let mut table = catalog.get_table("orders").unwrap().clone();
        assert_eq!(
            table.constraints.len(),
            1,
            "should start with one CHECK constraint"
        );

        table.remove_column("extra");

        assert_eq!(
            table.constraints.len(),
            1,
            "CHECK constraint referencing 'amount' should be preserved after dropping unrelated 'extra'"
        );
        assert!(
            table.constraints.iter().any(
                |c| matches!(c, ConstraintState::Check { name: Some(n), .. } if n == "chk_positive")
            ),
            "chk_positive should still exist"
        );
    }
}
