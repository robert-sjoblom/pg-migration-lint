//! Table catalog types
//!
//! The catalog represents the database schema state at a point in migration history.
//! It's built by replaying migrations in order.

use crate::names::RelationName;
use crate::parser::ir::{DefaultExpr, IndexColumn, PartitionStrategy, TypeName};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tables: HashMap<RelationName, TableState>,
    /// Reverse lookup: bare index name → owning table.
    index_to_table: HashMap<String, RelationName>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a table by its schema-qualified name.
    pub fn get_table(&self, name: &RelationName) -> Option<&TableState> {
        self.tables.get(name)
    }

    pub(crate) fn get_table_mut(&mut self, name: &RelationName) -> Option<&mut TableState> {
        self.tables.get_mut(name)
    }

    /// True if a table with this schema-qualified name exists.
    pub fn has_table(&self, name: &RelationName) -> bool {
        self.tables.contains_key(name)
    }

    pub(crate) fn insert_table(&mut self, table: TableState) {
        // Register all indexes in the reverse lookup.
        for idx in &table.indexes {
            self.index_to_table
                .insert(idx.name.clone(), table.name.clone());
        }
        self.tables.insert(table.name.clone(), table);
    }

    pub(crate) fn remove_table(&mut self, name: &RelationName) -> Option<TableState> {
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
    pub(crate) fn register_index(&mut self, index_name: &str, table: &RelationName) {
        self.index_to_table
            .insert(index_name.to_string(), table.clone());
    }

    /// Remove an index from the reverse lookup.
    pub(crate) fn unregister_index(&mut self, index_name: &str) {
        self.index_to_table.remove(index_name);
    }

    /// Look up which table owns a given index. O(1).
    pub(crate) fn table_for_index(&self, index_name: &str) -> Option<&RelationName> {
        self.index_to_table.get(index_name)
    }

    /// Look up an index by name across all tables. Returns the `IndexState` if found.
    pub(crate) fn get_index(&self, index_name: &str) -> Option<&IndexState> {
        let table = self.index_to_table.get(index_name)?;
        let table = self.tables.get(table)?;
        table.indexes.iter().find(|idx| idx.name == index_name)
    }

    pub fn tables(&self) -> impl Iterator<Item = &TableState> {
        self.tables.values()
    }

    /// Returns the names of all partition children of the given parent.
    ///
    /// Computed on demand by scanning tables with matching `parent_table`.
    /// Partition queries are rare (only a few rules check them), so the O(n)
    /// scan is negligible compared to maintaining a bidirectional map.
    pub(crate) fn get_partition_children(&self, parent: &RelationName) -> Vec<RelationName> {
        self.tables
            .values()
            .filter(|t| t.parent_table.as_ref() == Some(parent))
            .map(|t| t.name.clone())
            .collect()
    }

    /// Returns `true` if the given table is a partition child (has a `parent_table`).
    #[cfg(test)]
    pub(crate) fn is_partition_child(&self, table: &RelationName) -> bool {
        self.tables
            .get(table)
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
    pub name: RelationName,
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
    /// The parent table, if this table is a partition child.
    pub parent_table: Option<RelationName>,
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
            ConstraintState::Exclude { elements, .. } => {
                !elements.iter().any(|e| e.references_column(name))
            }
        });

        // Recalculate has_primary_key in case the PK was removed.
        self.has_primary_key = self
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::PrimaryKey { .. }));
    }

    /// Check if any usable index on this table indexes at least one of the
    /// given FK columns, in any position. An index covering only some of the
    /// FK columns still lets PostgreSQL avoid a genuine sequential scan
    /// (Index Scan with a Filter on the uncovered columns, or Bitmap Heap
    /// Scan with a Recheck), so it counts as coverage even though a fully
    /// covering index performs better. Column order and position do not
    /// matter here -- contrast with PGM508's `is_prefix`, which does care
    /// about order.
    ///
    /// Skipped indexes:
    /// - Partial indexes — they only index a subset of rows.
    /// - ON ONLY indexes — they don't cover partition children.
    /// - GIN, GiST, and BRIN indexes — they don't provide the direct equality
    ///   point lookups FK enforcement needs (BRIN is a lossy range summary;
    ///   GIN/GiST serve other operator classes). B-tree and hash indexes both
    ///   support equality lookups and so both count as coverage; PostgreSQL
    ///   hash indexes are inherently single-column, which lines up with the
    ///   single-FK-column-subset case this function already treats as coverage.
    ///
    /// Expression entries never match, since an expression cannot equal a
    /// plain FK column name.
    pub fn has_indexed_fk_column(&self, fk_columns: &[String]) -> bool {
        self.indexes.iter().any(|idx| {
            if idx.is_partial() || idx.only || !idx.supports_equality_lookup() {
                return false;
            }
            idx.column_names()
                .any(|name| fk_columns.iter().any(|fc| fc == name))
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
    pub fn is_btree(&self) -> bool {
        self.access_method == Self::DEFAULT_ACCESS_METHOD
    }

    /// True if this index's access method supports direct equality point
    /// lookups (B-tree or hash) rather than only specialized operator classes
    /// (GIN, GiST) or lossy range summaries (BRIN).
    pub fn supports_equality_lookup(&self) -> bool {
        self.is_btree() || self.access_method == "hash"
    }
}

/// A constraint on a table. Every constraint has a name, as in `pg_constraint`:
/// the one written in the DDL, or the one Postgres generated for it (see
/// `catalog::generated_names`). PRIMARY KEY and UNIQUE constraints share
/// their name with their backing index.
#[derive(Debug, Clone)]
pub enum ConstraintState {
    PrimaryKey {
        name: String,
        columns: Vec<String>,
    },
    ForeignKey {
        name: String,
        columns: Vec<String>,
        ref_table: RelationName,
        /// User-facing referenced table name (omits synthetic schema prefix).
        ref_table_display: String,
        ref_columns: Vec<String>,
        not_valid: bool,
    },
    Unique {
        name: String,
        columns: Vec<String>,
    },
    Check {
        name: String,
        expression: String,
        not_valid: bool,
    },
    Exclude {
        name: String,
        elements: Vec<IndexColumn>,
    },
}

impl ConstraintState {
    /// The constraint's name, given or generated.
    pub fn name(&self) -> &str {
        match self {
            ConstraintState::PrimaryKey { name, .. }
            | ConstraintState::ForeignKey { name, .. }
            | ConstraintState::Unique { name, .. }
            | ConstraintState::Check { name, .. }
            | ConstraintState::Exclude { name, .. } => name,
        }
    }

    /// True for PRIMARY KEY, UNIQUE and EXCLUDE: Postgres enforces them with
    /// an index of the same name, which dropping the constraint drops too.
    pub fn is_index_backed(&self) -> bool {
        matches!(
            self,
            ConstraintState::PrimaryKey { .. }
                | ConstraintState::Unique { .. }
                | ConstraintState::Exclude { .. }
        )
    }

    /// Returns true if this constraint involves the given column name.
    pub fn involves_column(&self, col: &str) -> bool {
        match self {
            ConstraintState::PrimaryKey { columns, .. }
            | ConstraintState::ForeignKey { columns, .. }
            | ConstraintState::Unique { columns, .. } => columns.iter().any(|c| c == col),
            ConstraintState::Check { expression, .. } => {
                expression_mentions_column(expression, col)
            }
            ConstraintState::Exclude { elements, .. } => {
                elements.iter().any(|e| e.references_column(col))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::builder::{CatalogBuilder, rel};
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

        assert!(catalog.is_partition_child(&rel("child")));
        assert!(!catalog.is_partition_child(&rel("parent")));
        assert!(!catalog.is_partition_child(&rel("standalone")));
        assert!(!catalog.is_partition_child(&rel("nonexistent")));
    }

    #[test]
    fn test_remove_table_returns_none_for_nonexistent() {
        let mut catalog = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false);
            })
            .build();

        assert!(catalog.remove_table(&rel("nonexistent")).is_none());
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

        let children = catalog.get_partition_children(&rel("parent"));
        assert_eq!(children, vec![rel("child")]);
        assert!(
            catalog
                .get_partition_children(&rel("nonexistent"))
                .is_empty()
        );
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
            name: "chk_positive".to_string(),
            expression: "(amount > 0)".to_string(),
            not_valid: false,
        };
        assert_eq!(
            constraint.involves_column(col),
            expected,
            "CHECK constraint with expression '(amount > 0)' involves_column(\"{col}\") should be {expected}"
        );
    }

    #[rstest]
    #[case::matches("room", true)]
    #[case::no_match("id", false)]
    fn test_involves_column_exclude_constraint(#[case] col: &str, #[case] expected: bool) {
        let constraint = ConstraintState::Exclude {
            name: "excl_rooms".to_string(),
            elements: vec![
                IndexColumn::Column("room".to_string()),
                IndexColumn::Column("period".to_string()),
            ],
        };
        assert_eq!(
            constraint.involves_column(col),
            expected,
            "EXCLUDE constraint over (room, period) involves_column(\"{col}\") should be {expected}"
        );
    }

    #[test]
    fn test_remove_column_drops_exclude_referencing_column() {
        let catalog = CatalogBuilder::new()
            .table("rooms", |t| {
                t.column("id", "integer", false)
                    .column("room", "int4range", false)
                    .column("period", "tsrange", false)
                    .exclude_constraint(Some("excl_rooms"), &["room", "period"]);
            })
            .build();

        let mut table = catalog.get_table(&rel("rooms")).unwrap().clone();
        assert_eq!(
            table.constraints.len(),
            1,
            "should start with one EXCLUDE constraint"
        );

        table.remove_column("room");

        assert!(
            table.constraints.is_empty(),
            "EXCLUDE constraint referencing 'room' should be removed after dropping 'room'"
        );
    }

    #[test]
    fn test_remove_column_keeps_unrelated_exclude() {
        let catalog = CatalogBuilder::new()
            .table("rooms", |t| {
                t.column("id", "integer", false)
                    .column("room", "int4range", false)
                    .column("period", "tsrange", false)
                    .column("extra", "text", true)
                    .exclude_constraint(Some("excl_rooms"), &["room", "period"]);
            })
            .build();

        let mut table = catalog.get_table(&rel("rooms")).unwrap().clone();

        table.remove_column("extra");

        assert_eq!(
            table.constraints.len(),
            1,
            "EXCLUDE constraint should be preserved after dropping unrelated 'extra'"
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

        let mut table = catalog.get_table(&rel("orders")).unwrap().clone();
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

        let mut table = catalog.get_table(&rel("orders")).unwrap().clone();
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
                |c| matches!(c, ConstraintState::Check { name, .. } if name == "chk_positive")
            ),
            "chk_positive should still exist"
        );
    }
}
