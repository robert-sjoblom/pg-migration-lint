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
//!          .fk("fk_customer", &["customer_id"], "customers", &["id"])
//!     })
//!     .build();
//! ```

use crate::catalog::types::{Catalog, ColumnState, ConstraintState, IndexState, TableState};
use crate::parser::ir::{DefaultExpr, TypeName};

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

    pub fn build(self) -> Catalog {
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
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique,
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

    /// Mark this table as incomplete (affected by unparseable SQL)
    pub fn incomplete(&mut self) -> &mut Self {
        self.state.incomplete = true;
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
}
