//! Test harness for building catalog state
//!
//! This builder provides a fluent API for constructing catalog state in tests.
//! It's a Phase 0 priority deliverable - both Catalog Agent and Rules Agent
//! depend on this for component tests.
//!
//! # Example
//!
//! ```rust
//! use pg_migration_lint::catalog::builder::CatalogBuilder;
//! use pg_migration_lint::parser::ir::DefaultExpr;
//!
//! let catalog = CatalogBuilder::new()
//!     .table("orders", |t| {
//!         t.column("id", "int", false)
//!          .column("status", "text", true)
//!          .index("idx_status", &["status"], false)
//!          .pk(&["id"])
//!          .fk("fk_customer", &["customer_id"], "customers", &["id"]);
//!     })
//!     .build();
//! ```

use crate::catalog::types::{
    Catalog, ColumnState, ConstraintState, IndexEntry, IndexState, PartitionByInfo, TableState,
};
use crate::parser::ir::{DefaultExpr, PartitionStrategy, TypeName};

/// Heuristic: extract bare identifiers from expression text as column references.
///
/// Splits on non-alphanumeric/underscore characters, filters out SQL keywords
/// and purely numeric tokens. Good enough for test builder usage.
fn extract_column_refs_from_expr_text(expr: &str) -> Vec<String> {
    let keywords = [
        "lower",
        "upper",
        "coalesce",
        "cast",
        "concat",
        "replace",
        "substring",
        "trim",
        "btrim",
        "ltrim",
        "rtrim",
        "length",
        "left",
        "right",
        "md5",
        "text",
        "integer",
        "boolean",
        "varchar",
        "numeric",
        "date",
        "time",
        "interval",
        "json",
        "jsonb",
        "true",
        "false",
        "null",
        "is",
        "not",
        "and",
        "or",
        "in",
    ];
    let mut refs = Vec::new();
    for token in expr.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let t = token.trim();
        if !t.is_empty()
            && !t.chars().all(|c| c.is_ascii_digit())
            && !keywords.contains(&t.to_lowercase().as_str())
            && !refs.contains(&t.to_string())
        {
            refs.push(t.to_string());
        }
    }
    refs.sort();
    refs
}

/// Builder for constructing a Catalog in tests
pub struct CatalogBuilder {
    catalog: Catalog,
}

impl CatalogBuilder {
    pub fn new() -> Self {
        Self {
            catalog: Catalog::new(),
        }
    }

    /// Add a table to the catalog. The closure receives a TableBuilder
    /// to configure columns, indexes, and constraints.
    pub fn table(mut self, name: &str, f: impl FnOnce(&mut TableBuilder)) -> Self {
        let mut builder = TableBuilder::new(name);
        f(&mut builder);
        self.catalog.insert_table(builder.build());
        self
    }

    pub fn build(mut self) -> Catalog {
        // Build partition_children map from tables with parent_table set.
        let pairs: Vec<(String, String)> = self
            .catalog
            .tables()
            .filter_map(|table| {
                table
                    .parent_table
                    .as_ref()
                    .map(|pk| (pk.clone(), table.name.clone()))
            })
            .collect();
        for (parent_key, child_key) in pairs {
            self.catalog.attach_partition(&parent_key, &child_key);
        }
        self.catalog
    }
}

impl Default for CatalogBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for constructing a TableState in tests
pub struct TableBuilder {
    state: TableState,
}

impl TableBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            state: TableState {
                name: name.to_string(),
                display_name: name.to_string(),
                columns: vec![],
                indexes: vec![],
                constraints: vec![],
                has_primary_key: false,
                incomplete: false,
                is_partitioned: false,
                partition_by: None,
                parent_table: None,
            },
        }
    }

    /// Add a column without a default value
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

    /// Add a column with a default value
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

    /// Add an index
    pub fn index(&mut self, name: &str, columns: &[&str], unique: bool) -> &mut Self {
        self.state.indexes.push(IndexState {
            name: name.to_string(),
            entries: columns
                .iter()
                .map(|s| IndexEntry::Column(s.to_string()))
                .collect(),
            unique,
            where_clause: None,
            only: false,
        });
        self
    }

    /// Add an ON ONLY index (not propagated to partitions).
    pub fn only_index(&mut self, name: &str, columns: &[&str], unique: bool) -> &mut Self {
        self.state.indexes.push(IndexState {
            name: name.to_string(),
            entries: columns
                .iter()
                .map(|s| IndexEntry::Column(s.to_string()))
                .collect(),
            unique,
            where_clause: None,
            only: true,
        });
        self
    }

    /// Add a partial index (with a WHERE clause).
    pub fn partial_index(
        &mut self,
        name: &str,
        columns: &[&str],
        unique: bool,
        where_clause: &str,
    ) -> &mut Self {
        self.state.indexes.push(IndexState {
            name: name.to_string(),
            entries: columns
                .iter()
                .map(|s| IndexEntry::Column(s.to_string()))
                .collect(),
            unique,
            where_clause: Some(where_clause.to_string()),
            only: false,
        });
        self
    }

    /// Add an expression index.
    ///
    /// Entries prefixed with `"expr:"` are treated as expressions; all others
    /// are plain column names. Example: `&["tenant_id", "expr:lower(email)"]`.
    ///
    /// Referenced columns are extracted heuristically from expression text:
    /// bare identifiers that aren't SQL keywords are treated as column refs.
    /// The result is sorted for deterministic ordering.
    pub fn expression_index(
        &mut self,
        name: &str,
        entries_spec: &[&str],
        unique: bool,
    ) -> &mut Self {
        self.state.indexes.push(IndexState {
            name: name.to_string(),
            entries: entries_spec
                .iter()
                .map(|s| {
                    if let Some(expr) = s.strip_prefix("expr:") {
                        IndexEntry::Expression {
                            text: expr.to_string(),
                            referenced_columns: extract_column_refs_from_expr_text(expr),
                        }
                    } else {
                        IndexEntry::Column(s.to_string())
                    }
                })
                .collect(),
            unique,
            where_clause: None,
            only: false,
        });
        self
    }

    /// Add a primary key constraint
    pub fn pk(&mut self, columns: &[&str]) -> &mut Self {
        self.state.has_primary_key = true;
        self.state.constraints.push(ConstraintState::PrimaryKey {
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Add a foreign key constraint
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
            ref_table_display: ref_table.to_string(),
            ref_columns: ref_columns.iter().map(|s| s.to_string()).collect(),
            not_valid: false,
        });
        self
    }

    /// Add a unique constraint
    pub fn unique(&mut self, name: &str, columns: &[&str]) -> &mut Self {
        self.state.constraints.push(ConstraintState::Unique {
            name: Some(name.to_string()),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Add a CHECK constraint with expression text
    pub fn check_constraint(
        &mut self,
        name: Option<&str>,
        expression: &str,
        not_valid: bool,
    ) -> &mut Self {
        self.state.constraints.push(ConstraintState::Check {
            name: name.map(|s| s.to_string()),
            expression: expression.to_string(),
            not_valid,
        });
        self
    }

    /// Mark this table as incomplete (affected by unparseable SQL)
    pub fn incomplete(&mut self) -> &mut Self {
        self.state.incomplete = true;
        self
    }

    /// Mark this table as partitioned with the given strategy and columns.
    pub fn partitioned_by(&mut self, strategy: PartitionStrategy, columns: &[&str]) -> &mut Self {
        self.state.is_partitioned = true;
        self.state.partition_by = Some(PartitionByInfo {
            strategy,
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Mark this table as a partition child of the given parent.
    pub fn partition_of(&mut self, parent_key: &str) -> &mut Self {
        self.state.parent_table = Some(parent_key.to_string());
        self
    }

    pub fn build(self) -> TableState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_builder_basic() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();

        assert!(catalog.has_table("users"));
        let table = catalog.get_table("users").unwrap();
        assert_eq!(table.columns.len(), 1);
        assert_eq!(table.columns[0].name, "id");
        assert!(table.has_primary_key);
    }

    #[test]
    fn test_catalog_builder_complex() {
        let catalog = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"])
                    .fk("fk_customer", &["customer_id"], "customers", &["id"])
                    .index("idx_status", &["status"], false);
            })
            .build();

        let orders = catalog.get_table("orders").unwrap();
        assert_eq!(orders.columns.len(), 3);
        assert_eq!(orders.indexes.len(), 1);
        assert_eq!(orders.constraints.len(), 2); // PK + FK
        assert!(orders.has_primary_key);
    }

    #[test]
    fn test_has_covering_index() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("customer_id", "integer", false)
                    .column("product_id", "integer", false)
                    .index(
                        "idx_customer_product",
                        &["customer_id", "product_id"],
                        false,
                    );
            })
            .build();

        let orders = catalog.get_table("orders").unwrap();

        // Exact match
        assert!(orders.has_covering_index(&["customer_id".to_string(), "product_id".to_string()]));

        // Prefix match
        assert!(orders.has_covering_index(&["customer_id".to_string()]));

        // Wrong order - should not match
        assert!(!orders.has_covering_index(&["product_id".to_string(), "customer_id".to_string()]));
    }

    #[test]
    fn test_has_unique_not_null() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"]);
            })
            .build();

        let users = catalog.get_table("users").unwrap();
        assert!(users.has_unique_not_null());
    }

    #[test]
    fn test_has_unique_not_null_via_index() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .index("idx_email_unique", &["email"], true);
            })
            .build();

        let users = catalog.get_table("users").unwrap();
        assert!(users.has_unique_not_null());
    }

    #[test]
    fn test_has_covering_index_skips_partial_index() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("status", "text", false).partial_index(
                    "idx_status_active",
                    &["status"],
                    false,
                    "active = true",
                );
            })
            .build();
        let orders = catalog.get_table("orders").unwrap();
        assert!(
            !orders.has_covering_index(&["status".to_string()]),
            "Partial index should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_has_covering_index_skips_only_index() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("ref_id", "integer", false)
                    .only_index("idx_ref", &["ref_id"], false);
            })
            .build();
        let orders = catalog.get_table("orders").unwrap();
        assert!(
            !orders.has_covering_index(&["ref_id".to_string()]),
            "ON ONLY index should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_has_covering_index_skips_expression_prefix() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false).expression_index(
                    "idx_email_lower",
                    &["expr:lower(email)"],
                    false,
                );
            })
            .build();
        let users = catalog.get_table("users").unwrap();
        assert!(
            !users.has_covering_index(&["email".to_string()]),
            "Expression index should NOT satisfy FK coverage for column 'email'"
        );
    }

    #[test]
    fn test_has_covering_index_column_before_expression_prefix_matches() {
        let catalog = CatalogBuilder::new()
            .table("items", |t| {
                t.column("tenant_id", "integer", false)
                    .column("email", "text", false)
                    .expression_index(
                        "idx_tenant_email",
                        &["tenant_id", "expr:lower(email)"],
                        false,
                    );
            })
            .build();
        let items = catalog.get_table("items").unwrap();
        // FK (tenant_id) is covered by index (tenant_id, lower(email)) — first entry is a plain column match.
        assert!(
            items.has_covering_index(&["tenant_id".to_string()]),
            "FK (tenant_id) should be covered by index (tenant_id, lower(email))"
        );
        // FK (tenant_id, email) is NOT covered — second entry is an expression, not a column.
        assert!(
            !items.has_covering_index(&["tenant_id".to_string(), "email".to_string()]),
            "FK (tenant_id, email) should NOT be covered by index (tenant_id, lower(email))"
        );
    }

    #[test]
    fn test_has_unique_not_null_skips_partial() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false).partial_index(
                    "idx_email_active",
                    &["email"],
                    true,
                    "deleted_at IS NULL",
                );
            })
            .build();
        let users = catalog.get_table("users").unwrap();
        assert!(
            !users.has_unique_not_null(),
            "Partial unique index should NOT count as PK substitute"
        );
    }

    #[test]
    fn test_has_unique_not_null_skips_expression_index() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false).expression_index(
                    "idx_email_lower",
                    &["expr:lower(email)"],
                    true,
                );
            })
            .build();
        let users = catalog.get_table("users").unwrap();
        assert!(
            !users.has_unique_not_null(),
            "Expression unique index should NOT count as PK substitute"
        );
    }

    // -----------------------------------------------------------------------
    // Partition support tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_partitioned_by_builder() {
        use crate::parser::ir::PartitionStrategy;

        let catalog = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "integer", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .build();

        let table = catalog.get_table("measurements").unwrap();
        assert!(table.is_partitioned);
        let pb = table.partition_by.as_ref().unwrap();
        assert!(matches!(pb.strategy, PartitionStrategy::Range));
        assert_eq!(pb.columns, vec!["ts".to_string()]);
    }

    #[test]
    fn test_partition_of_builder() {
        let catalog = CatalogBuilder::new()
            .table("child", |t| {
                t.column("id", "integer", false)
                    .partition_of("public.parent");
            })
            .build();

        let table = catalog.get_table("child").unwrap();
        assert_eq!(table.parent_table.as_deref(), Some("public.parent"));
    }

    #[test]
    fn test_partition_children_built_from_tables() {
        use crate::parser::ir::PartitionStrategy;

        let catalog = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("child_a", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .table("child_b", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .build();

        let children = catalog.get_partition_children("parent");
        assert_eq!(children.len(), 2);
        assert!(children.contains(&"child_a".to_string()));
        assert!(children.contains(&"child_b".to_string()));
    }
}
