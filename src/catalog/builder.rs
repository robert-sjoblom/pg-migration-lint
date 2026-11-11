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

use crate::catalog::generated_names::GeneratedName;
use crate::catalog::types::{
    Catalog, ColumnState, ConstraintState, IndexState, PartitionByInfo, TableState,
};
use crate::names::RelationName;
use crate::parser::ir::IndexColumn;
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

/// The schema the builder puts a bare test table name in, matching the
/// default `default_schema` the pipeline normalizes unqualified names to.
pub const TEST_DEFAULT_SCHEMA: &str = "public";

/// The relation a test table name refers to: `"orders"` is
/// `public.orders`, and `"myschema.orders"` splits at its first `.`. Only
/// test strings are split this way; nothing in the pipeline splits names.
pub fn test_relation(name: &str) -> RelationName {
    match name.split_once('.') {
        Some((schema, bare)) => RelationName::new(schema, bare),
        None => rel(name),
    }
}

/// The relation `public.{name}`, the key a bare name resolves to in tests.
pub fn rel(name: &str) -> RelationName {
    RelationName::new(TEST_DEFAULT_SCHEMA, name)
}

/// The relation `{schema}.{name}`.
pub fn rel_in(schema: &str, name: &str) -> RelationName {
    RelationName::new(schema, name)
}

/// Catalog lookups and assertions for tests that panic with what the catalog
/// actually holds. Table names are written as in [`CatalogBuilder`]: `"t"` is
/// `public.t` and `"audit.t"` is `audit.t` (see [`test_relation`]). Index
/// names are bare, as [`Catalog::table_for_index`] takes them.
#[cfg(test)]
pub(crate) trait CatalogAssertions {
    /// The table `name`; panics listing the catalog's tables if it is absent.
    fn table(&self, name: &str) -> &TableState;
    /// Panics listing the catalog's tables unless the table `name` exists.
    fn assert_table(&self, name: &str);
    /// Panics listing the catalog's tables if the table `name` exists.
    fn assert_no_table(&self, name: &str);
    /// Panics unless the partition children of `parent` are exactly
    /// `expected`, compared as sets.
    fn assert_partition_children(&self, parent: &str, expected: &[&str]);
    /// Panics unless the reverse lookup maps `index` to the table `table`.
    fn assert_index_owner(&self, index: &str, table: &str);
    /// Panics if the reverse lookup maps `index` to any table.
    fn assert_index_unregistered(&self, index: &str);
}

#[cfg(test)]
fn schema_dot_name(relation: &RelationName) -> String {
    format!("{}.{}", relation.schema, relation.name)
}

#[cfg(test)]
fn sorted_schema_dot_names<'a>(relations: impl IntoIterator<Item = &'a RelationName>) -> String {
    let mut names: Vec<String> = relations.into_iter().map(schema_dot_name).collect();
    names.sort();
    format!("[{}]", names.join(", "))
}

#[cfg(test)]
fn catalog_table_names(catalog: &Catalog) -> String {
    sorted_schema_dot_names(catalog.tables().map(|table| &table.name))
}

#[cfg(test)]
impl CatalogAssertions for Catalog {
    #[track_caller]
    fn table(&self, name: &str) -> &TableState {
        let relation = test_relation(name);
        match self.get_table(&relation) {
            Some(table) => table,
            None => panic!(
                "no table {}; catalog has {}",
                schema_dot_name(&relation),
                catalog_table_names(self)
            ),
        }
    }

    #[track_caller]
    fn assert_table(&self, name: &str) {
        self.table(name);
    }

    #[track_caller]
    fn assert_no_table(&self, name: &str) {
        let relation = test_relation(name);
        assert!(
            !self.has_table(&relation),
            "unexpected table {}; catalog has {}",
            schema_dot_name(&relation),
            catalog_table_names(self)
        );
    }

    #[track_caller]
    fn assert_partition_children(&self, parent: &str, expected: &[&str]) {
        let parent = test_relation(parent);
        let mut actual = self.get_partition_children(&parent);
        actual.sort();
        let mut expected: Vec<RelationName> =
            expected.iter().map(|name| test_relation(name)).collect();
        expected.sort();
        assert!(
            actual == expected,
            "partition children of {}: expected {}, actual {}",
            schema_dot_name(&parent),
            sorted_schema_dot_names(&expected),
            sorted_schema_dot_names(&actual)
        );
    }

    #[track_caller]
    fn assert_index_owner(&self, index: &str, table: &str) {
        let expected = test_relation(table);
        match self.table_for_index(index) {
            Some(owner) if *owner == expected => {}
            Some(owner) => panic!(
                "index {index} belongs to {}, expected {}",
                schema_dot_name(owner),
                schema_dot_name(&expected)
            ),
            None => panic!(
                "index {index} is unregistered, expected it to belong to {}",
                schema_dot_name(&expected)
            ),
        }
    }

    #[track_caller]
    fn assert_index_unregistered(&self, index: &str) {
        if let Some(owner) = self.table_for_index(index) {
            panic!(
                "index {index} should be unregistered but belongs to {}",
                schema_dot_name(owner)
            );
        }
    }
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
    /// to configure columns, indexes, and constraints. A bare `name` is put
    /// in `public`; see [`test_relation`].
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
    /// Start a table named `name`, which [`test_relation`] resolves. The
    /// display name is `name` as written.
    pub fn new(name: &str) -> Self {
        Self {
            state: TableState {
                name: test_relation(name),
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

    /// The table's bare name, which unnamed constraints are named after.
    fn bare_table_name(&self) -> &str {
        &self.state.name.name
    }

    /// Push an `IndexState` with sensible defaults; returns a mutable reference
    /// so callers can override `only`, `where_clause`, or `access_method`.
    fn push_index(
        &mut self,
        name: &str,
        entries: Vec<IndexColumn>,
        unique: bool,
    ) -> &mut IndexState {
        self.state.indexes.push(IndexState {
            name: name.to_string(),
            entries,
            unique,
            where_clause: None,
            only: false,
            access_method: IndexState::DEFAULT_ACCESS_METHOD.to_string(),
        });
        // SAFETY: we just pushed, so last_mut always returns Some
        let len = self.state.indexes.len();
        &mut self.state.indexes[len - 1]
    }

    /// Convert `&[&str]` column names to `Vec<IndexColumn>`.
    fn column_entries(columns: &[&str]) -> Vec<IndexColumn> {
        columns
            .iter()
            .map(|s| IndexColumn::Column(s.to_string()))
            .collect()
    }

    /// Add an index
    pub fn index(&mut self, name: &str, columns: &[&str], unique: bool) -> &mut Self {
        self.push_index(name, Self::column_entries(columns), unique);
        self
    }

    /// Add an index with a specific access method (e.g. "gin", "gist", "brin").
    pub fn index_with_method(
        &mut self,
        name: &str,
        columns: &[&str],
        unique: bool,
        method: &str,
    ) -> &mut Self {
        let idx = self.push_index(name, Self::column_entries(columns), unique);
        idx.access_method = method.to_string();
        self
    }

    /// Add an ON ONLY index (not propagated to partitions).
    pub fn only_index(&mut self, name: &str, columns: &[&str], unique: bool) -> &mut Self {
        let idx = self.push_index(name, Self::column_entries(columns), unique);
        idx.only = true;
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
        let idx = self.push_index(name, Self::column_entries(columns), unique);
        idx.where_clause = Some(where_clause.to_string());
        self
    }

    /// Add an expression index.
    ///
    /// Entries prefixed with `"expr:"` are treated as expressions; all others
    /// are plain column names. Example: `&["tenant_id", "expr:lower(email)"]`.
    ///
    /// Referenced columns are extracted heuristically from expression text:
    /// bare identifiers that aren't SQL keywords are treated as column refs.
    /// The result is sorted for deterministic ordering. Every expression gets
    /// the generic `expr` name part; parse SQL instead when a test depends on
    /// the name part Postgres would derive.
    pub fn expression_index(
        &mut self,
        name: &str,
        entries_spec: &[&str],
        unique: bool,
    ) -> &mut Self {
        let entries: Vec<IndexColumn> = entries_spec
            .iter()
            .map(|s| {
                if let Some(expr) = s.strip_prefix("expr:") {
                    IndexColumn::Expression {
                        text: expr.to_string(),
                        referenced_columns: extract_column_refs_from_expr_text(expr),
                        generated_name_part: IndexColumn::UNNAMED_EXPRESSION_NAME_PART.to_string(),
                    }
                } else {
                    IndexColumn::Column(s.to_string())
                }
            })
            .collect();
        self.push_index(name, entries, unique);
        self
    }

    /// Add a primary key constraint named `{table}_pkey`, as Postgres names
    /// an unnamed one. Like the other constraint methods, it adds no backing
    /// index.
    pub fn pk(&mut self, columns: &[&str]) -> &mut Self {
        let name = GeneratedName::primary_key(self.bare_table_name()).unsuffixed();
        self.state.has_primary_key = true;
        self.state.constraints.push(ConstraintState::PrimaryKey {
            name,
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Add a primary key constraint with a custom name
    pub fn named_pk(&mut self, pk_name: &str, columns: &[&str]) -> &mut Self {
        self.state.has_primary_key = true;
        self.state.constraints.push(ConstraintState::PrimaryKey {
            name: pk_name.to_string(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Add a foreign key constraint. `ref_table` is resolved like a table
    /// name (see [`test_relation`]) and displayed as written.
    pub fn fk(
        &mut self,
        name: &str,
        columns: &[&str],
        ref_table: &str,
        ref_columns: &[&str],
    ) -> &mut Self {
        self.state.constraints.push(ConstraintState::ForeignKey {
            name: name.to_string(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            ref_table: test_relation(ref_table),
            ref_table_display: ref_table.to_string(),
            ref_columns: ref_columns.iter().map(|s| s.to_string()).collect(),
            not_valid: false,
        });
        self
    }

    /// Add a unique constraint
    pub fn unique(&mut self, name: &str, columns: &[&str]) -> &mut Self {
        self.state.constraints.push(ConstraintState::Unique {
            name: name.to_string(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Add a CHECK constraint with expression text. Without a name it gets
    /// the one Postgres would generate, from the columns
    /// [`extract_column_refs_from_expr_text`] finds in the expression.
    pub fn check_constraint(
        &mut self,
        name: Option<&str>,
        expression: &str,
        not_valid: bool,
    ) -> &mut Self {
        let name = name.map_or_else(
            || {
                let referenced_columns = extract_column_refs_from_expr_text(expression);
                GeneratedName::check(self.bare_table_name(), &referenced_columns).unsuffixed()
            },
            str::to_string,
        );
        self.state.constraints.push(ConstraintState::Check {
            name,
            expression: expression.to_string(),
            not_valid,
        });
        self
    }

    /// Add an EXCLUDE constraint. Without a name it gets the one Postgres
    /// would generate.
    ///
    /// Entries prefixed with `"expr:"` are treated as expressions; all others
    /// are plain column names — same convention, and the same `expr` name part
    /// for expressions, as [`Self::expression_index`].
    pub fn exclude_constraint(&mut self, name: Option<&str>, elements_spec: &[&str]) -> &mut Self {
        let elements: Vec<IndexColumn> = elements_spec
            .iter()
            .map(|s| {
                if let Some(expr) = s.strip_prefix("expr:") {
                    IndexColumn::Expression {
                        text: expr.to_string(),
                        referenced_columns: extract_column_refs_from_expr_text(expr),
                        generated_name_part: IndexColumn::UNNAMED_EXPRESSION_NAME_PART.to_string(),
                    }
                } else {
                    IndexColumn::Column(s.to_string())
                }
            })
            .collect();
        let name = name.map_or_else(
            || GeneratedName::exclude(self.bare_table_name(), &elements, &[]).unsuffixed(),
            str::to_string,
        );
        self.state
            .constraints
            .push(ConstraintState::Exclude { name, elements });
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

    /// Mark this table as a partition child of the given parent, a table
    /// name that [`test_relation`] resolves.
    pub fn partition_of(&mut self, parent: &str) -> &mut Self {
        self.state.parent_table = Some(test_relation(parent));
        self
    }

    /// Override the display name shown in finding messages (defaults to the
    /// name passed to [`TableBuilder::new`]).
    pub fn display_name(&mut self, name: &str) -> &mut Self {
        self.state.display_name = name.to_string();
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

        catalog.assert_table("users");
        let table = catalog.table("users");
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

        let orders = catalog.table("orders");
        assert_eq!(orders.columns.len(), 3);
        assert_eq!(orders.indexes.len(), 1);
        assert_eq!(orders.constraints.len(), 2); // PK + FK
        assert!(orders.has_primary_key);
    }

    #[test]
    fn test_has_indexed_fk_column_exact_match_and_leading_subset() {
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

        let orders = catalog.table("orders");

        // Exact match
        assert!(
            orders.has_indexed_fk_column(&["customer_id".to_string(), "product_id".to_string()])
        );

        // Leading single-column subset
        assert!(orders.has_indexed_fk_column(&["customer_id".to_string()]));
    }

    #[test]
    fn test_has_indexed_fk_column_reversed_order() {
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

        let orders = catalog.table("orders");

        assert!(
            orders.has_indexed_fk_column(&["product_id".to_string(), "customer_id".to_string()]),
            "Reversed order still fully covers -- column order doesn't matter for equality lookups"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_superset_any_order() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("customer_id", "integer", false)
                    .column("product_id", "integer", false)
                    .column("status", "text", false)
                    .index("idx_multi", &["customer_id", "product_id", "status"], false);
            })
            .build();

        let orders = catalog.table("orders");

        assert!(
            orders.has_indexed_fk_column(&["product_id".to_string(), "customer_id".to_string()]),
            "FK columns present as a subset of a longer index, in reversed order relative to the index, still counts as coverage"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_single_column_subset_non_leading() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("status", "text", false)
                    .column("product_id", "integer", false)
                    .index("idx_status_product", &["status", "product_id"], false);
            })
            .build();

        let orders = catalog.table("orders");

        assert!(
            orders.has_indexed_fk_column(&["customer_id".to_string(), "product_id".to_string()]),
            "customer_id isn't even a column on this table/index, but product_id matches at a non-leading index position -- this is the shape of the real production bug"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_no_overlap() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .column("x", "integer", false)
                    .column("y", "integer", false)
                    .index("idx_xy", &["x", "y"], false);
            })
            .build();

        let orders = catalog.table("orders");

        assert!(
            !orders.has_indexed_fk_column(&["a".to_string(), "b".to_string()]),
            "An index exists on the table, but shares no column with the FK at all, so this must still fire -- the genuine tier-3 seq-scan case"
        );
    }

    #[test]
    fn test_has_unique_not_null() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"]);
            })
            .build();

        let users = catalog.table("users");
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

        let users = catalog.table("users");
        assert!(users.has_unique_not_null());
    }

    #[test]
    fn test_has_indexed_fk_column_skips_partial_index() {
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
        let orders = catalog.table("orders");
        assert!(
            !orders.has_indexed_fk_column(&["status".to_string()]),
            "Partial index should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_skips_only_index() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("ref_id", "integer", false)
                    .only_index("idx_ref", &["ref_id"], false);
            })
            .build();
        let orders = catalog.table("orders");
        assert!(
            !orders.has_indexed_fk_column(&["ref_id".to_string()]),
            "ON ONLY index should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_hash_index_counts() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("customer_id", "integer", false).index_with_method(
                    "idx_customer_hash",
                    &["customer_id"],
                    false,
                    "hash",
                );
            })
            .build();
        let orders = catalog.table("orders");
        assert!(
            orders.has_indexed_fk_column(&["customer_id".to_string()]),
            "Hash index supports equality lookups, so it should satisfy FK coverage"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_skips_gin() {
        let catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("tags", "text[]", false).index_with_method(
                    "idx_tags_gin",
                    &["tags"],
                    false,
                    "gin",
                );
            })
            .build();
        let orders = catalog.table("orders");
        assert!(
            !orders.has_indexed_fk_column(&["tags".to_string()]),
            "GIN index does not provide equality point lookups, so it should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_skips_expression_only() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false).expression_index(
                    "idx_email_lower",
                    &["expr:lower(email)"],
                    false,
                );
            })
            .build();
        let users = catalog.table("users");
        assert!(
            !users.has_indexed_fk_column(&["email".to_string()]),
            "Expression index should NOT satisfy FK coverage for column 'email'"
        );
    }

    #[test]
    fn test_has_indexed_fk_column_column_before_expression() {
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
        let items = catalog.table("items");
        // FK (tenant_id) is covered by index (tenant_id, lower(email)) -- first entry is a plain column match.
        assert!(
            items.has_indexed_fk_column(&["tenant_id".to_string()]),
            "FK (tenant_id) should be covered by index (tenant_id, lower(email))"
        );
        // FK (tenant_id, email) is still covered -- tenant_id is a plain-column match,
        // even though email only appears via an expression.
        assert!(
            items.has_indexed_fk_column(&["tenant_id".to_string(), "email".to_string()]),
            "FK (tenant_id, email) is still covered -- tenant_id is a plain-column match, even though email only appears via an expression"
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
        let users = catalog.table("users");
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
        let users = catalog.table("users");
        assert!(
            !users.has_unique_not_null(),
            "Expression unique index should NOT count as PK substitute"
        );
    }

    #[test]
    fn test_has_unique_not_null_skips_non_btree_index() {
        let catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false).index_with_method(
                    "idx_email_hash",
                    &["email"],
                    true,
                    "hash",
                );
            })
            .build();
        let users = catalog.table("users");
        assert!(
            !users.has_unique_not_null(),
            "Non-btree unique index should NOT count as PK substitute"
        );
    }

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

        let table = catalog.table("measurements");
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

        let table = catalog.table("child");
        assert_eq!(table.parent_table, Some(rel("parent")));
    }

    #[test]
    fn test_partition_children_computed_from_parent_table() {
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

        catalog.assert_partition_children("parent", &["child_a", "child_b"]);
    }

    fn catalog_with_public_items_and_audit_orders() -> Catalog {
        CatalogBuilder::new()
            .table("items", |t| {
                t.column("sku", "text", false)
                    .index("idx_items_sku", &["sku"], false);
            })
            .table("audit.orders", |t| {
                t.column("id", "integer", false);
            })
            .build()
    }

    fn catalog_with_parent_partitioned_into_child_a_and_child_b() -> Catalog {
        CatalogBuilder::new()
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
            .build()
    }

    #[test]
    fn test_table_assertion_resolves_bare_name_to_public_and_keeps_explicit_schema() {
        let catalog = catalog_with_public_items_and_audit_orders();

        assert_eq!(catalog.table("items").name, rel("items"));
        assert_eq!(
            catalog.table("audit.orders").name,
            rel_in("audit", "orders")
        );
    }

    #[test]
    #[should_panic(expected = "no table public.missing; catalog has [audit.orders, public.items]")]
    fn test_table_assertion_panic_lists_catalog_tables() {
        catalog_with_public_items_and_audit_orders().table("missing");
    }

    #[test]
    fn test_assert_table_accepts_existing_table() {
        catalog_with_public_items_and_audit_orders().assert_table("audit.orders");
    }

    #[test]
    #[should_panic(expected = "no table public.orders; catalog has [audit.orders, public.items]")]
    fn test_assert_table_panic_lists_catalog_tables() {
        catalog_with_public_items_and_audit_orders().assert_table("orders");
    }

    #[test]
    fn test_assert_no_table_accepts_absent_table() {
        catalog_with_public_items_and_audit_orders().assert_no_table("orders");
    }

    #[test]
    #[should_panic(
        expected = "unexpected table public.items; catalog has [audit.orders, public.items]"
    )]
    fn test_assert_no_table_panic_lists_catalog_tables() {
        catalog_with_public_items_and_audit_orders().assert_no_table("items");
    }

    #[test]
    fn test_assert_partition_children_ignores_order() {
        let catalog = catalog_with_parent_partitioned_into_child_a_and_child_b();

        catalog.assert_partition_children("parent", &["child_b", "child_a"]);
        catalog.assert_partition_children("child_a", &[]);
    }

    #[test]
    #[should_panic(
        expected = "partition children of public.parent: expected [public.child_a], actual [public.child_a, public.child_b]"
    )]
    fn test_assert_partition_children_panic_shows_expected_and_actual() {
        catalog_with_parent_partitioned_into_child_a_and_child_b()
            .assert_partition_children("parent", &["child_a"]);
    }

    #[test]
    fn test_assert_index_owner_accepts_owning_table() {
        catalog_with_public_items_and_audit_orders().assert_index_owner("idx_items_sku", "items");
    }

    #[test]
    #[should_panic(expected = "index idx_items_sku belongs to public.items, expected audit.orders")]
    fn test_assert_index_owner_panic_shows_actual_owner() {
        catalog_with_public_items_and_audit_orders()
            .assert_index_owner("idx_items_sku", "audit.orders");
    }

    #[test]
    #[should_panic(
        expected = "index idx_missing is unregistered, expected it to belong to public.items"
    )]
    fn test_assert_index_owner_panic_reports_unregistered_index() {
        catalog_with_public_items_and_audit_orders().assert_index_owner("idx_missing", "items");
    }

    #[test]
    fn test_assert_index_unregistered_accepts_unknown_index() {
        catalog_with_public_items_and_audit_orders().assert_index_unregistered("idx_missing");
    }

    #[test]
    #[should_panic(
        expected = "index idx_items_sku should be unregistered but belongs to public.items"
    )]
    fn test_assert_index_unregistered_panic_shows_owner() {
        catalog_with_public_items_and_audit_orders().assert_index_unregistered("idx_items_sku");
    }
}
