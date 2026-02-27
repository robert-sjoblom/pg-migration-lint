use super::*;
use crate::catalog::builder::CatalogBuilder;
use std::path::PathBuf;

/// Helper to create a MigrationUnit from a list of IR nodes.
fn make_unit(nodes: Vec<IrNode>) -> MigrationUnit {
    MigrationUnit {
        id: "test".to_string(),
        statements: nodes
            .into_iter()
            .map(|node| Located {
                node,
                span: SourceSpan {
                    start_line: 1,
                    end_line: 1,
                    start_offset: 0,
                    end_offset: 0,
                },
            })
            .collect(),
        source_file: PathBuf::from("test.sql"),
        source_line_offset: 1,
        run_in_transaction: true,
        is_down: false,
    }
}

/// Helper: shorthand for creating a QualifiedName with just a table name.
fn qname(name: &str) -> QualifiedName {
    QualifiedName::unqualified(name)
}

/// Helper: shorthand for creating a TypeName with no modifiers.
fn simple_type(name: &str) -> TypeName {
    TypeName::simple(name)
}

/// Helper: create a basic ColumnDef.
fn col(name: &str, type_name: &str, nullable: bool) -> ColumnDef {
    ColumnDef::test(name, type_name).with_nullable(nullable)
}

/// Helper: create a ColumnDef with inline PK.
fn col_pk(name: &str, type_name: &str) -> ColumnDef {
    ColumnDef::test(name, type_name).with_inline_pk()
}

// -----------------------------------------------------------------------
// Test 1: Create then drop
// -----------------------------------------------------------------------
#[test]
fn test_create_then_drop() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
        CreateIndex::test(Some("idx_id".to_string()), qname("t"))
            .with_columns(vec![IndexColumn::Column("id".to_string())])
            .into(),
        DropTable::test(qname("t")).with_if_exists(false).into(),
    ]);

    apply(&mut catalog, &unit);

    assert!(
        !catalog.has_table("t"),
        "Table should be gone after DROP TABLE"
    );
    assert!(
        catalog.table_for_index("t_pkey").is_none(),
        "Reverse map should be cleared for PK index after DROP TABLE"
    );
    assert!(
        catalog.table_for_index("idx_id").is_none(),
        "Reverse map should be cleared for regular index after DROP TABLE"
    );
}

// -----------------------------------------------------------------------
// Test 2: Create, alter, add index
// -----------------------------------------------------------------------
#[test]
fn test_create_alter_add_index() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::AddColumn(col("name", "text", true))],
        }
        .into(),
        CreateIndex::test(Some("idx_name".to_string()), qname("t"))
            .with_columns(vec![IndexColumn::Column("name".to_string())])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(table.columns.len(), 2, "Should have 2 columns after ALTER");
    assert_eq!(table.columns[0].name, "id");
    assert_eq!(table.columns[1].name, "name");
    assert_eq!(table.indexes.len(), 1, "Should have 1 index");
    assert_eq!(table.indexes[0].name, "idx_name");
    assert_eq!(
        table.indexes[0].column_names().collect::<Vec<_>>(),
        vec!["name"]
    );
}

// -----------------------------------------------------------------------
// Test 3: FK tracks referencing columns
// -----------------------------------------------------------------------
#[test]
fn test_fk_tracks_referencing_columns() {
    let mut catalog = Catalog::new();

    // Create parent table with PK
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Create child table with FK to parent
    let unit2 = make_unit(vec![
        CreateTable::test(qname("child"))
            .with_columns(vec![col("pid", "integer", false)])
            .with_constraints(vec![TableConstraint::ForeignKey {
                name: Some("fk_parent".to_string()),
                columns: vec!["pid".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            }])
            .into(),
    ]);

    apply(&mut catalog, &unit2);

    let child = catalog.get_table("child").expect("child should exist");
    assert_eq!(child.constraints.len(), 1);
    match &child.constraints[0] {
        ConstraintState::ForeignKey {
            name,
            columns,
            ref_table,
            ref_columns,
            ..
        } => {
            assert_eq!(name.as_deref(), Some("fk_parent"));
            assert_eq!(columns, &["pid".to_string()]);
            assert_eq!(ref_table, "parent");
            assert_eq!(ref_columns, &["id".to_string()]);
        }
        other => panic!("Expected ForeignKey constraint, got {:?}", other),
    }
}

// -----------------------------------------------------------------------
// Test 4: Unparseable marks table incomplete
// -----------------------------------------------------------------------
#[test]
fn test_unparseable_marks_incomplete() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: Some("t".to_string()),
        },
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        table.incomplete,
        "Table should be marked incomplete after Unparseable with hint"
    );
}

// -----------------------------------------------------------------------
// Test 5: Index removal
// -----------------------------------------------------------------------
#[test]
fn test_index_removal() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("a", "integer", false)])
            .into(),
        CreateIndex::test(Some("idx_a".to_string()), qname("t"))
            .with_columns(vec![IndexColumn::Column("a".to_string())])
            .into(),
        DropIndex::test("idx_a").with_if_exists(false).into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        table.indexes.is_empty(),
        "Indexes should be empty after DROP INDEX"
    );
    assert!(
        catalog.table_for_index("idx_a").is_none(),
        "Reverse map should be cleared after DROP INDEX"
    );
}

// -----------------------------------------------------------------------
// Test 6: Column type change
// -----------------------------------------------------------------------
#[test]
fn test_column_type_change() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("x", "integer", false)])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "x".to_string(),
                new_type: simple_type("bigint"),
                old_type: Some(simple_type("integer")),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    let column = table.get_column("x").expect("column x should exist");
    assert_eq!(column.type_name.name, "bigint");
}

// -----------------------------------------------------------------------
// Test 7: Drop column removes associated indexes
// -----------------------------------------------------------------------
#[test]
fn test_drop_column_removes_indexes() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("a", "integer", false), col("b", "text", true)])
            .into(),
        CreateIndex::test(Some("idx_ab".to_string()), qname("t"))
            .with_columns(vec![
                IndexColumn::Column("a".to_string()),
                IndexColumn::Column("b".to_string()),
            ])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::DropColumn {
                name: "b".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(table.columns.len(), 1, "Only column 'a' should remain");
    assert_eq!(table.columns[0].name, "a");
    assert!(
        table.indexes.is_empty(),
        "Index referencing dropped column should be removed"
    );
    assert!(
        catalog.table_for_index("idx_ab").is_none(),
        "Reverse map should be cleared after DROP COLUMN removes index"
    );
}

// -----------------------------------------------------------------------
// Test 8: Re-create after drop
// -----------------------------------------------------------------------
#[test]
fn test_recreate_after_drop() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        DropTable::test(qname("t")).with_if_exists(false).into(),
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "bigint", false)])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("t")
        .expect("table should exist after re-create");
    assert_eq!(table.columns.len(), 1);
    assert_eq!(table.columns[0].type_name.name, "bigint");
}

// -----------------------------------------------------------------------
// Test 9: Composite index column order
// -----------------------------------------------------------------------
#[test]
fn test_composite_index_column_order() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![
                col("a", "integer", false),
                col("b", "integer", false),
                col("c", "integer", false),
            ])
            .into(),
        CreateIndex::test(Some("idx_abc".to_string()), qname("t"))
            .with_columns(vec![
                IndexColumn::Column("a".to_string()),
                IndexColumn::Column("b".to_string()),
                IndexColumn::Column("c".to_string()),
            ])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(
        table.indexes[0].column_names().collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}

// -----------------------------------------------------------------------
// Test 10: Inline PK normalizes
// -----------------------------------------------------------------------
#[test]
fn test_inline_pk_normalizes() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(table.has_primary_key, "has_primary_key should be true");
    assert!(
        table.constraints.iter().any(|c| matches!(
            c,
            ConstraintState::PrimaryKey { columns } if columns == &["id".to_string()]
        )),
        "Should have a PrimaryKey constraint for 'id'"
    );
}

// -----------------------------------------------------------------------
// Test 11: Table-level PK normalizes the same way
// -----------------------------------------------------------------------
#[test]
fn test_table_level_pk_normalizes() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .with_constraints(vec![TableConstraint::PrimaryKey {
                columns: vec!["id".to_string()],
                using_index: None,
            }])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(table.has_primary_key, "has_primary_key should be true");
    assert!(
        table.constraints.iter().any(|c| matches!(
            c,
            ConstraintState::PrimaryKey { columns } if columns == &["id".to_string()]
        )),
        "Should have a PrimaryKey constraint for 'id'"
    );
}

// -----------------------------------------------------------------------
// Test 12: Inline FK normalizes (via table constraints from parser)
// -----------------------------------------------------------------------
#[test]
fn test_inline_fk_normalizes() {
    let mut catalog = Catalog::new();

    // Create parent first
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // The parser normalizes inline FK (REFERENCES) into a TableConstraint.
    // So this simulates: CREATE TABLE child(pid int REFERENCES parent(id))
    let unit2 = make_unit(vec![
        CreateTable::test(qname("child"))
            .with_columns(vec![col("pid", "integer", false)])
            .with_constraints(vec![TableConstraint::ForeignKey {
                name: None,
                columns: vec!["pid".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            }])
            .into(),
    ]);

    apply(&mut catalog, &unit2);

    let child = catalog.get_table("child").expect("child should exist");
    assert!(
        child.constraints.iter().any(|c| matches!(
            c,
            ConstraintState::ForeignKey {
                columns, ref_table, ref_columns, ..
            } if columns == &["pid".to_string()]
                && ref_table == "parent"
                && ref_columns == &["id".to_string()]
        )),
        "Should have FK constraint with correct columns"
    );
}

// -----------------------------------------------------------------------
// Test 13: Table-level FK normalizes the same as inline
// -----------------------------------------------------------------------
#[test]
fn test_table_level_fk_normalizes() {
    let mut catalog = Catalog::new();

    // Create parent
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Table-level FK: FOREIGN KEY (pid) REFERENCES parent(id)
    let unit2 = make_unit(vec![
        CreateTable::test(qname("child"))
            .with_columns(vec![col("pid", "integer", false)])
            .with_constraints(vec![TableConstraint::ForeignKey {
                name: Some("fk_parent".to_string()),
                columns: vec!["pid".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            }])
            .into(),
    ]);

    apply(&mut catalog, &unit2);

    let child = catalog.get_table("child").expect("child should exist");
    assert!(
        child.constraints.iter().any(|c| matches!(
            c,
            ConstraintState::ForeignKey {
                name: Some(n),
                columns,
                ref_table,
                ref_columns,
                ..
            } if n == "fk_parent"
                && columns == &["pid".to_string()]
                && ref_table == "parent"
                && ref_columns == &["id".to_string()]
        )),
        "Should have named FK constraint with correct columns"
    );
}

// -----------------------------------------------------------------------
// Additional edge case tests
// -----------------------------------------------------------------------

#[test]
fn test_alter_nonexistent_table_silently_skips() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        AlterTable {
            name: qname("nonexistent"),
            actions: vec![AlterTableAction::AddColumn(col("x", "integer", false))],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    assert!(
        !catalog.has_table("nonexistent"),
        "Should not create table from ALTER"
    );
}

#[test]
fn test_create_index_on_nonexistent_table_silently_skips() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateIndex::test(Some("idx_x".to_string()), qname("nonexistent"))
            .with_columns(vec![IndexColumn::Column("x".to_string())])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    assert!(
        !catalog.has_table("nonexistent"),
        "Should not create table from CREATE INDEX"
    );
}

#[test]
fn test_drop_index_nonexistent_silently_skips() {
    let mut catalog = Catalog::new();

    // Create a table with an index, then drop a different (nonexistent) index
    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("a", "integer", false)])
            .into(),
        CreateIndex::test(Some("idx_a".to_string()), qname("t"))
            .with_columns(vec![IndexColumn::Column("a".to_string())])
            .into(),
        DropIndex::test("idx_nonexistent")
            .with_if_exists(false)
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(
        table.indexes.len(),
        1,
        "Original index should still be present"
    );
}

#[test]
fn test_unparseable_without_hint_is_noop() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        IrNode::Unparseable {
            raw_sql: "GRANT SELECT ON t TO user".to_string(),
            table_hint: None,
        },
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        !table.incomplete,
        "Table should NOT be marked incomplete when hint is None"
    );
}

#[test]
fn test_ignored_node_is_noop() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        IrNode::Ignored {
            raw_sql: "COMMENT ON TABLE t IS 'A table'".to_string(),
        },
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(
        table.columns.len(),
        1,
        "Ignored node should not alter catalog"
    );
}

#[test]
fn test_column_with_default_expr() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![
                ColumnDef::test("created_at", "timestamptz")
                    .with_nullable(false)
                    .with_default(DefaultExpr::FunctionCall {
                        name: "now".to_string(),
                        args: vec![],
                    }),
            ])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    let col = table.get_column("created_at").expect("column should exist");
    assert!(col.has_default, "Column should have default");
    assert!(
        matches!(&col.default_expr, Some(DefaultExpr::FunctionCall { name, .. }) if name == "now"),
        "Default should be now() function call"
    );
}

#[test]
fn test_unique_index_flag() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("email", "text", false)])
            .into(),
        CreateIndex::test(Some("idx_email_unique".to_string()), qname("t"))
            .with_columns(vec![IndexColumn::Column("email".to_string())])
            .with_unique(true)
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(table.indexes.len(), 1);
    assert!(table.indexes[0].unique, "Index should be marked as unique");
}

#[test]
fn test_add_constraint_via_alter_table() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![
                col("id", "integer", false),
                col("email", "text", false),
            ])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![
                AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                    using_index: None,
                }),
                AlterTableAction::AddConstraint(TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }),
                AlterTableAction::AddConstraint(TableConstraint::Check {
                    name: Some("ck_email".to_string()),
                    expression: "email <> ''".to_string(),
                    not_valid: false,
                }),
                AlterTableAction::AddConstraint(TableConstraint::Exclude {
                    name: Some("excl_email".to_string()),
                }),
            ],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(table.has_primary_key, "PK should be set via ALTER TABLE");
    assert_eq!(
        table.constraints.len(),
        4,
        "Should have PK, Unique, Check, and Exclude constraints"
    );
    assert!(
        table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::Exclude { name: Some(n) } if n == "excl_email")),
        "Exclude constraint should be present with correct name"
    );
    assert_eq!(
        catalog.table_for_index("t_pkey"),
        Some("t"),
        "Synthetic PK index should be in reverse map after ALTER TABLE ADD PRIMARY KEY"
    );
}

#[test]
fn test_multiple_units_build_catalog_incrementally() {
    let mut catalog = Catalog::new();

    // Unit 1: Create table
    let unit1 = make_unit(vec![
        CreateTable::test(qname("users"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Unit 2: Add column
    let unit2 = make_unit(vec![
        AlterTable {
            name: qname("users"),
            actions: vec![AlterTableAction::AddColumn(col("name", "text", true))],
        }
        .into(),
    ]);
    apply(&mut catalog, &unit2);

    // Unit 3: Add index
    let unit3 = make_unit(vec![
        CreateIndex::test(Some("idx_users_name".to_string()), qname("users"))
            .with_columns(vec![IndexColumn::Column("name".to_string())])
            .with_concurrent(true)
            .into(),
    ]);
    apply(&mut catalog, &unit3);

    let table = catalog.get_table("users").expect("users should exist");
    assert_eq!(table.columns.len(), 2);
    assert!(table.has_primary_key);
    assert_eq!(table.indexes.len(), 2);
    assert_eq!(table.indexes[0].name, "users_pkey");
    assert_eq!(table.indexes[1].name, "idx_users_name");
}

#[test]
fn test_alter_other_action_is_noop() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::Other {
                description: "SET TABLESPACE fast_ssd".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(
        table.columns.len(),
        1,
        "Other action should not modify columns"
    );
}

#[test]
fn test_different_schemas_stored_separately() {
    let mut catalog = Catalog::new();

    // Create "public.orders"
    let unit1 = make_unit(vec![
        CreateTable::test(QualifiedName::qualified("public", "orders"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Create "audit.orders" — same table name, different schema
    let unit2 = make_unit(vec![
        CreateTable::test(QualifiedName::qualified("audit", "orders"))
            .with_columns(vec![
                col("log_id", "integer", false),
                col("data", "text", true),
            ])
            .into(),
    ]);
    apply(&mut catalog, &unit2);

    // Both should exist as separate entries
    assert!(
        catalog.has_table("public.orders"),
        "public.orders should exist"
    );
    assert!(
        catalog.has_table("audit.orders"),
        "audit.orders should exist"
    );

    // Verify they have different column structures
    let public_orders = catalog
        .get_table("public.orders")
        .expect("public.orders should exist");
    assert_eq!(public_orders.columns.len(), 1);
    assert_eq!(public_orders.columns[0].name, "id");
    assert!(public_orders.has_primary_key);

    let audit_orders = catalog
        .get_table("audit.orders")
        .expect("audit.orders should exist");
    assert_eq!(audit_orders.columns.len(), 2);
    assert_eq!(audit_orders.columns[0].name, "log_id");
    assert!(!audit_orders.has_primary_key);
}

#[test]
fn test_create_index_without_name() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("a", "integer", false)])
            .into(),
        CreateIndex::test(None, qname("t"))
            .with_columns(vec![IndexColumn::Column("a".to_string())])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(
        table.indexes[0].name, "",
        "Unnamed index should have empty string name"
    );
}

// -----------------------------------------------------------------------
// Tests: DROP COLUMN constraint cleanup
// -----------------------------------------------------------------------

#[test]
fn test_drop_column_removes_pk_constraint() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::DropColumn {
                name: "id".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        !table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::PrimaryKey { .. })),
        "PK constraint should be removed after dropping PK column"
    );
    assert!(
        !table.has_primary_key,
        "has_primary_key should be false after dropping PK column"
    );
}

#[test]
fn test_drop_column_removes_multi_column_pk() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![col("a", "integer", false), col("b", "integer", false)])
            .with_constraints(vec![TableConstraint::PrimaryKey {
                columns: vec!["a".to_string(), "b".to_string()],
                using_index: None,
            }])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::DropColumn {
                name: "a".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        !table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::PrimaryKey { .. })),
        "Entire multi-column PK constraint should be removed when one column is dropped"
    );
    assert!(
        !table.has_primary_key,
        "has_primary_key should be false after dropping a column from composite PK"
    );
}

#[test]
fn test_drop_column_removes_fk_constraint() {
    let mut catalog = Catalog::new();

    // Create referenced table first
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Create child table with FK, then drop the FK column
    let unit2 = make_unit(vec![
        CreateTable::test(qname("child"))
            .with_columns(vec![
                col("id", "integer", false),
                col("customer_id", "integer", true),
            ])
            .with_constraints(vec![TableConstraint::ForeignKey {
                name: Some("fk_customer".to_string()),
                columns: vec!["customer_id".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            }])
            .into(),
        AlterTable {
            name: qname("child"),
            actions: vec![AlterTableAction::DropColumn {
                name: "customer_id".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit2);

    let child = catalog.get_table("child").expect("child should exist");
    assert!(
        !child
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::ForeignKey { .. })),
        "FK constraint should be removed after dropping the FK column"
    );
}

#[test]
fn test_drop_column_removes_unique_constraint() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![
                col("id", "integer", false),
                col("email", "text", false),
            ])
            .with_constraints(vec![TableConstraint::Unique {
                name: Some("uk_email".to_string()),
                columns: vec!["email".to_string()],
                using_index: None,
            }])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        !table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::Unique { .. })),
        "Unique constraint should be removed after dropping the constrained column"
    );
}

#[test]
fn test_drop_column_preserves_unrelated_constraints() {
    let mut catalog = Catalog::new();

    // Create referenced table
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col_pk("id", "integer")])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Create table with PK on (id) and FK on (customer_id), then drop customer_id
    let unit2 = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![
                col_pk("id", "integer"),
                col("customer_id", "integer", true),
            ])
            .with_constraints(vec![TableConstraint::ForeignKey {
                name: Some("fk_customer".to_string()),
                columns: vec!["customer_id".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            }])
            .into(),
        AlterTable {
            name: qname("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "customer_id".to_string(),
            }],
        }
        .into(),
    ]);
    apply(&mut catalog, &unit2);

    let table = catalog.get_table("orders").expect("orders should exist");
    assert!(
        !table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::ForeignKey { .. })),
        "FK constraint should be removed"
    );
    assert!(
        table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::PrimaryKey { .. })),
        "PK constraint should be preserved (unrelated to dropped column)"
    );
    assert!(
        table.has_primary_key,
        "has_primary_key should still be true"
    );
}

#[test]
fn test_drop_column_preserves_check_constraint() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![
                col("id", "integer", false),
                col("amount", "integer", false),
                col("extra", "text", true),
            ])
            .with_constraints(vec![TableConstraint::Check {
                name: Some("chk_positive".to_string()),
                expression: "amount > 0".to_string(),
                not_valid: false,
            }])
            .into(),
        AlterTable {
            name: qname("t"),
            actions: vec![AlterTableAction::DropColumn {
                name: "extra".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("t").expect("table should exist");
    assert!(
        table.constraints.iter().any(
            |c| matches!(c, ConstraintState::Check { name: Some(n), .. } if n == "chk_positive")
        ),
        "Check constraint should be preserved when dropping an unrelated column"
    );
}

// -----------------------------------------------------------------------
// Test: SET NOT NULL via ALTER TABLE
// -----------------------------------------------------------------------
#[test]
fn test_apply_set_not_null() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("id", "integer", false)
                .column("status", "text", true);
        })
        .build();

    let unit = make_unit(vec![
        AlterTable {
            name: QualifiedName::qualified("public", "orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("public.orders")
        .expect("table should exist");
    let col = table.get_column("status").expect("column should exist");
    assert!(
        !col.nullable,
        "Column 'status' should be NOT NULL after SET NOT NULL"
    );
}

// -----------------------------------------------------------------------
// Test: RENAME TABLE
// -----------------------------------------------------------------------
#[test]
fn test_apply_rename_table() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("id", "integer", false)
                .column("status", "text", true)
                .index("idx_orders_status", &["status"], false);
        })
        .build();

    let unit = make_unit(vec![IrNode::RenameTable {
        name: QualifiedName::qualified("public", "orders"),
        new_name: "orders_archive".to_string(),
    }]);

    apply(&mut catalog, &unit);

    assert!(
        !catalog.has_table("public.orders"),
        "Old table name should no longer exist"
    );
    assert!(
        catalog.has_table("public.orders_archive"),
        "New table name should exist"
    );

    let table = catalog
        .get_table("public.orders_archive")
        .expect("renamed table should exist");
    assert_eq!(table.columns.len(), 2, "Renamed table should keep columns");
    assert_eq!(table.columns[0].name, "id");
    assert_eq!(table.columns[1].name, "status");
    assert_eq!(
        table.indexes.len(),
        1,
        "Renamed table should keep its indexes"
    );
    assert_eq!(table.indexes[0].name, "idx_orders_status");
}

// -----------------------------------------------------------------------
// Test: RENAME COLUMN updates column and index references
// -----------------------------------------------------------------------
#[test]
fn test_apply_rename_column() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("id", "integer", false)
                .column("status", "text", true)
                .index("idx_orders_status", &["status"], false);
        })
        .build();

    let unit = make_unit(vec![IrNode::RenameColumn {
        table: QualifiedName::qualified("public", "orders"),
        old_name: "status".to_string(),
        new_name: "order_status".to_string(),
    }]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("public.orders")
        .expect("table should exist");

    // Column should be renamed
    assert!(
        table.get_column("status").is_none(),
        "Old column name should not exist"
    );
    let col = table
        .get_column("order_status")
        .expect("renamed column should exist");
    assert_eq!(col.name, "order_status");

    // Index should reference the new column name
    assert_eq!(
        table.indexes[0].column_names().collect::<Vec<_>>(),
        vec!["order_status"],
        "Index should reference the new column name"
    );
}

// -----------------------------------------------------------------------
// Test: RENAME COLUMN updates FK constraint references
// -----------------------------------------------------------------------
#[test]
fn test_apply_rename_column_updates_constraints() {
    let mut catalog = CatalogBuilder::new()
        .table("public.customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("public.orders", |t| {
            t.column("id", "integer", false)
                .column("customer_id", "integer", false)
                .fk("fk_customer", &["customer_id"], "public.customers", &["id"]);
        })
        .build();

    let unit = make_unit(vec![IrNode::RenameColumn {
        table: QualifiedName::qualified("public", "orders"),
        old_name: "customer_id".to_string(),
        new_name: "cust_id".to_string(),
    }]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("public.orders")
        .expect("table should exist");

    // Find the FK constraint and check that its columns were updated
    let fk = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::ForeignKey { .. }))
        .expect("FK constraint should exist");

    match fk {
        ConstraintState::ForeignKey { columns, .. } => {
            assert_eq!(
                columns,
                &["cust_id".to_string()],
                "FK constraint columns should reference 'cust_id' instead of 'customer_id'"
            );
        }
        other => panic!("Expected ForeignKey constraint, got {:?}", other),
    }
}

// -----------------------------------------------------------------------
// Test: RENAME COLUMN updates ref_columns on self-referencing FK
// -----------------------------------------------------------------------
#[test]
fn test_apply_rename_column_updates_self_referencing_fk_ref_columns() {
    let mut catalog = CatalogBuilder::new()
        .table("public.employees", |t| {
            t.column("id", "bigint", false)
                .column("manager_id", "bigint", true)
                .pk(&["id"])
                .fk("fk_manager", &["manager_id"], "public.employees", &["id"]);
        })
        .build();

    let unit = make_unit(vec![IrNode::RenameColumn {
        table: QualifiedName::qualified("public", "employees"),
        old_name: "id".to_string(),
        new_name: "employee_id".to_string(),
    }]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("public.employees")
        .expect("table should exist");

    // The column should be renamed
    assert!(
        table.get_column("id").is_none(),
        "Old column name 'id' should not exist"
    );
    assert!(
        table.get_column("employee_id").is_some(),
        "New column name 'employee_id' should exist"
    );

    // The PK constraint should reference the new column name
    let pk = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::PrimaryKey { .. }))
        .expect("PK constraint should exist");
    match pk {
        ConstraintState::PrimaryKey { columns } => {
            assert_eq!(
                columns,
                &["employee_id".to_string()],
                "PK constraint should reference 'employee_id' (not 'id')"
            );
        }
        other => panic!("Expected PrimaryKey constraint, got {:?}", other),
    }

    // The FK constraint columns should be unchanged (we renamed "id", not "manager_id")
    let fk = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::ForeignKey { .. }))
        .expect("FK constraint should exist");
    match fk {
        ConstraintState::ForeignKey {
            columns,
            ref_columns,
            ..
        } => {
            assert_eq!(
                columns,
                &["manager_id".to_string()],
                "FK columns should still be 'manager_id' (unchanged)"
            );
            assert_eq!(
                ref_columns,
                &["employee_id".to_string()],
                "FK ref_columns should be 'employee_id' (not 'id') for self-referencing FK"
            );
        }
        other => panic!("Expected ForeignKey constraint, got {:?}", other),
    }
}

#[test]
fn test_add_pk_using_index_resolves_columns_from_index() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("id", "bigint", false)
                .index("idx_orders_pk", &["id"], true);
        })
        .build();

    let unit = make_unit(vec![IrNode::AlterTable(AlterTable {
        name: QualifiedName::qualified("public", "orders"),
        actions: vec![AlterTableAction::AddConstraint(
            TableConstraint::PrimaryKey {
                columns: vec![], // empty with USING INDEX
                using_index: Some("idx_orders_pk".to_string()),
            },
        )],
    })]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("public.orders").expect("table exists");
    let pk = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::PrimaryKey { .. }))
        .expect("PK constraint should exist");
    match pk {
        ConstraintState::PrimaryKey { columns } => {
            assert_eq!(
                columns,
                &["id".to_string()],
                "PK columns should be resolved from the index"
            );
        }
        other => panic!("Expected PrimaryKey, got {:?}", other),
    }
}

#[test]
fn test_add_unique_using_index_resolves_columns_from_index() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("email", "text", false)
                .index("idx_orders_email", &["email"], true);
        })
        .build();

    let unit = make_unit(vec![IrNode::AlterTable(AlterTable {
        name: QualifiedName::qualified("public", "orders"),
        actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
            name: Some("uq_orders_email".to_string()),
            columns: vec![], // empty with USING INDEX
            using_index: Some("idx_orders_email".to_string()),
        })],
    })]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("public.orders").expect("table exists");
    let uq = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::Unique { .. }))
        .expect("Unique constraint should exist");
    match uq {
        ConstraintState::Unique { columns, .. } => {
            assert_eq!(
                columns,
                &["email".to_string()],
                "Unique columns should be resolved from the index"
            );
        }
        other => panic!("Expected Unique, got {:?}", other),
    }
}

// -----------------------------------------------------------------------
// Tests: IF NOT EXISTS guards
// -----------------------------------------------------------------------

#[test]
fn test_create_table_if_not_exists_skips_when_table_exists() {
    let mut catalog = Catalog::new();

    // First: create the table normally with 2 columns and an index.
    let unit1 = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![col("id", "integer", false), col("name", "text", true)])
            .into(),
        CreateIndex::test(Some("idx_orders_name".to_string()), qname("orders"))
            .with_columns(vec![IndexColumn::Column("name".to_string())])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    let before = catalog.get_table("orders").expect("table should exist");
    assert_eq!(before.columns.len(), 2);
    assert_eq!(before.indexes.len(), 1);

    // Second: CREATE TABLE IF NOT EXISTS with a different column set.
    // Should be a no-op — existing state preserved.
    let unit2 = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![col("id", "integer", false)])
            .with_if_not_exists(true)
            .into(),
    ]);
    apply(&mut catalog, &unit2);

    let after = catalog
        .get_table("orders")
        .expect("table should still exist");
    assert_eq!(
        after.columns.len(),
        2,
        "Original columns should be preserved"
    );
    assert_eq!(after.indexes.len(), 1, "Original index should be preserved");
}

#[test]
fn test_create_table_if_not_exists_creates_when_new() {
    let mut catalog = Catalog::new();

    // IF NOT EXISTS on a table that doesn't exist → normal creation.
    let unit = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![col("id", "integer", false)])
            .with_if_not_exists(true)
            .into(),
    ]);
    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("orders")
        .expect("table should be created");
    assert_eq!(table.columns.len(), 1);
    assert_eq!(table.columns[0].name, "id");
}

#[test]
fn test_create_index_if_not_exists_skips_when_index_exists() {
    let mut catalog = Catalog::new();

    // Create table with an index.
    let unit1 = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![col("id", "integer", false), col("name", "text", true)])
            .into(),
        CreateIndex::test(Some("idx_orders_name".to_string()), qname("orders"))
            .with_columns(vec![IndexColumn::Column("name".to_string())])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    let before = catalog.get_table("orders").expect("table should exist");
    assert_eq!(before.indexes.len(), 1);
    assert!(!before.indexes[0].unique);

    // CREATE INDEX IF NOT EXISTS with same name but different properties.
    // Should be a no-op.
    let unit2 = make_unit(vec![
        CreateIndex::test(Some("idx_orders_name".to_string()), qname("orders"))
            .with_columns(vec![IndexColumn::Column("id".to_string())])
            .with_unique(true)
            .with_if_not_exists(true)
            .into(),
    ]);
    apply(&mut catalog, &unit2);

    let after = catalog
        .get_table("orders")
        .expect("table should still exist");
    assert_eq!(after.indexes.len(), 1, "Should still have 1 index");
    assert_eq!(
        after.indexes[0].column_names().collect::<Vec<_>>(),
        vec!["name"],
        "Original index columns should be preserved"
    );
    assert!(
        !after.indexes[0].unique,
        "Original index uniqueness should be preserved"
    );
}

#[test]
fn test_create_index_if_not_exists_creates_when_new() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
        CreateIndex::test(Some("idx_orders_id".to_string()), qname("orders"))
            .with_columns(vec![IndexColumn::Column("id".to_string())])
            .with_if_not_exists(true)
            .into(),
    ]);
    apply(&mut catalog, &unit);

    let table = catalog.get_table("orders").expect("table should exist");
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(table.indexes[0].name, "idx_orders_id");
}

#[test]
fn test_add_pk_using_index_missing_index_stays_empty() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("id", "bigint", false);
        })
        .build();

    let unit = make_unit(vec![IrNode::AlterTable(AlterTable {
        name: QualifiedName::qualified("public", "orders"),
        actions: vec![AlterTableAction::AddConstraint(
            TableConstraint::PrimaryKey {
                columns: vec![],
                using_index: Some("idx_nonexistent".to_string()),
            },
        )],
    })]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("public.orders").expect("table exists");
    let pk = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::PrimaryKey { .. }))
        .expect("PK constraint should exist");
    match pk {
        ConstraintState::PrimaryKey { columns } => {
            assert!(
                columns.is_empty(),
                "Columns should be empty when referenced index doesn't exist"
            );
        }
        other => panic!("Expected PrimaryKey, got {:?}", other),
    }
}

// -----------------------------------------------------------------------
// Tests: Partial and expression indexes in catalog replay
// -----------------------------------------------------------------------

#[test]
fn test_create_partial_index_stored_in_catalog() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("orders"))
            .with_columns(vec![
                col("status", "text", false),
                col("active", "boolean", false),
            ])
            .into(),
        CreateIndex::test(Some("idx_status_active".to_string()), qname("orders"))
            .with_columns(vec![IndexColumn::Column("status".to_string())])
            .with_where_clause("(active = true)")
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("orders").expect("table should exist");
    assert_eq!(table.indexes.len(), 1);
    let idx = &table.indexes[0];
    assert_eq!(idx.name, "idx_status_active");
    assert!(idx.is_partial(), "Index should be partial");
    assert_eq!(idx.where_clause.as_deref(), Some("(active = true)"));
    assert_eq!(idx.column_names().collect::<Vec<_>>(), vec!["status"]);
}

#[test]
fn test_create_expression_index_stored_in_catalog() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("users"))
            .with_columns(vec![col("email", "text", false)])
            .into(),
        CreateIndex::test(Some("idx_email_lower".to_string()), qname("users"))
            .with_columns(vec![IndexColumn::Expression {
                text: "lower(email)".to_string(),
                referenced_columns: vec!["email".to_string()],
            }])
            .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("users").expect("table should exist");
    assert_eq!(table.indexes.len(), 1);
    let idx = &table.indexes[0];
    assert_eq!(idx.name, "idx_email_lower");
    assert!(!idx.is_partial(), "Index should not be partial");
    assert!(idx.has_expressions(), "Index should have expressions");
    assert_eq!(idx.column_names().count(), 0, "No plain column names");
}

#[test]
fn test_rename_column_updates_index_entries() {
    let mut catalog = CatalogBuilder::new()
        .table("public.orders", |t| {
            t.column("status", "text", true)
                .column("active", "boolean", false)
                .expression_index("idx_mixed", &["status", "expr:lower(active::text)"], false);
        })
        .build();

    let unit = make_unit(vec![IrNode::RenameColumn {
        table: QualifiedName::qualified("public", "orders"),
        old_name: "status".to_string(),
        new_name: "order_status".to_string(),
    }]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("public.orders")
        .expect("table should exist");
    let idx = &table.indexes[0];
    // Column entry should be renamed; expression text should be unchanged,
    // but referenced_columns should stay current.
    assert_eq!(
        idx.entries,
        vec![
            IndexColumn::Column("order_status".to_string()),
            IndexColumn::Expression {
                text: "lower(active::text)".to_string(),
                referenced_columns: vec!["active".to_string()],
            },
        ]
    );
}

#[test]
fn test_drop_column_removes_expression_index_referencing_column() {
    // Expression indexes that reference a dropped column are now removed,
    // matching PostgreSQL behavior. The catalog tracks referenced_columns
    // extracted at parse time.
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("users"))
            .with_columns(vec![
                col("id", "integer", false),
                col("email", "text", false),
            ])
            .into(),
        CreateIndex::test(Some("idx_email_lower".to_string()), qname("users"))
            .with_columns(vec![IndexColumn::Expression {
                text: "lower(email)".to_string(),
                referenced_columns: vec!["email".to_string()],
            }])
            .into(),
        AlterTable {
            name: qname("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("users").expect("table should exist");
    assert_eq!(table.columns.len(), 1, "Only 'id' should remain");
    assert_eq!(
        table.indexes.len(),
        0,
        "Expression index referencing dropped column should be removed"
    );
}

#[test]
fn test_rename_column_updates_expression_referenced_columns() {
    // Renaming a column that is referenced inside an expression should
    // update referenced_columns while leaving the expression text unchanged.
    let mut catalog = CatalogBuilder::new()
        .table("public.users", |t| {
            t.column("email", "text", false).expression_index(
                "idx_email_lower",
                &["expr:lower(email)"],
                false,
            );
        })
        .build();

    let unit = make_unit(vec![IrNode::RenameColumn {
        table: QualifiedName::qualified("public", "users"),
        old_name: "email".to_string(),
        new_name: "mail".to_string(),
    }]);

    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("public.users")
        .expect("table should exist");
    let idx = &table.indexes[0];
    assert_eq!(
        idx.entries,
        vec![IndexColumn::Expression {
            text: "lower(email)".to_string(),
            referenced_columns: vec!["mail".to_string()],
        }],
        "referenced_columns should be updated, text should stay stale"
    );
}

#[test]
fn test_drop_plain_column_from_mixed_index_removes_index() {
    // Dropping a plain column from a mixed index (tenant_id, lower(email))
    // should remove the entire index, matching PostgreSQL behavior.
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("users"))
            .with_columns(vec![
                col("tenant_id", "integer", false),
                col("email", "text", false),
            ])
            .into(),
        CreateIndex::test(Some("idx_tenant_email".to_string()), qname("users"))
            .with_columns(vec![
                IndexColumn::Column("tenant_id".to_string()),
                IndexColumn::Expression {
                    text: "lower(email)".to_string(),
                    referenced_columns: vec!["email".to_string()],
                },
            ])
            .into(),
        AlterTable {
            name: qname("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "tenant_id".to_string(),
            }],
        }
        .into(),
    ]);

    apply(&mut catalog, &unit);

    let table = catalog.get_table("users").expect("table should exist");
    assert_eq!(table.columns.len(), 1, "Only 'email' should remain");
    assert_eq!(
        table.indexes.len(),
        0,
        "Mixed index should be removed when plain column is dropped"
    );
}

// -----------------------------------------------------------------------
// Partition support tests
// -----------------------------------------------------------------------

#[test]
fn test_create_partitioned_table() {
    let mut catalog = Catalog::new();
    let unit = make_unit(vec![
        CreateTable::test(qname("measurements"))
            .with_columns(vec![
                col("id", "integer", false),
                col("ts", "timestamptz", false),
            ])
            .with_partition_by(PartitionStrategy::Range, vec!["ts".to_string()])
            .into(),
    ]);
    apply(&mut catalog, &unit);

    let table = catalog
        .get_table("measurements")
        .expect("table should exist");
    assert!(table.is_partitioned);
    let pb = table
        .partition_by
        .as_ref()
        .expect("partition_by should be set");
    assert!(matches!(pb.strategy, PartitionStrategy::Range));
    assert_eq!(pb.columns, vec!["ts".to_string()]);
    assert!(table.parent_table.is_none());
}

#[test]
fn test_create_partition_of_with_parent_in_catalog() {
    let mut catalog = Catalog::new();

    // Create parent
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![
                col("id", "integer", false),
                col("ts", "timestamptz", false),
            ])
            .with_partition_by(PartitionStrategy::Range, vec!["ts".to_string()])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Create child PARTITION OF parent
    let unit2 = make_unit(vec![
        CreateTable::test(qname("child"))
            .with_partition_of(qname("parent"))
            .into(),
    ]);
    apply(&mut catalog, &unit2);

    let child = catalog.get_table("child").expect("child should exist");
    assert_eq!(child.parent_table.as_deref(), Some("parent"));
    // Child inherits parent's columns
    assert_eq!(child.columns.len(), 2);
    assert_eq!(child.columns[0].name, "id");
    assert_eq!(child.columns[1].name, "ts");

    assert_eq!(catalog.get_partition_children("parent"), &["child"]);
}

#[test]
fn test_create_partition_of_without_parent_in_catalog() {
    let mut catalog = Catalog::new();

    // Create child PARTITION OF non-existent parent
    let unit = make_unit(vec![
        CreateTable::test(qname("child"))
            .with_partition_of(qname("unknown_parent"))
            .into(),
    ]);
    apply(&mut catalog, &unit);

    let child = catalog.get_table("child").expect("child should exist");
    assert_eq!(child.parent_table.as_deref(), Some("unknown_parent"));
    // No inherited columns since parent doesn't exist
    assert_eq!(child.columns.len(), 0);
    // partition_children NOT registered when parent doesn't exist in catalog
    assert!(
        catalog.get_partition_children("unknown_parent").is_empty(),
        "No phantom entry should be created for unknown parent"
    );
}

#[test]
fn test_attach_partition_existing_child() {
    let mut catalog = Catalog::new();

    let unit = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col("id", "integer", false)])
            .with_partition_by(PartitionStrategy::Range, vec!["id".to_string()])
            .into(),
        CreateTable::test(qname("child"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
    ]);
    apply(&mut catalog, &unit);

    // Attach the child
    let unit2 = make_unit(vec![
        AlterTable {
            name: qname("parent"),
            actions: vec![AlterTableAction::AttachPartition {
                child: qname("child"),
            }],
        }
        .into(),
    ]);
    apply(&mut catalog, &unit2);

    let child = catalog.get_table("child").expect("child should exist");
    assert_eq!(child.parent_table.as_deref(), Some("parent"));
    assert_eq!(catalog.get_partition_children("parent"), &["child"]);
}

#[test]
fn test_attach_partition_missing_child() {
    let mut catalog = Catalog::new();

    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col("id", "integer", false)])
            .with_partition_by(PartitionStrategy::Range, vec!["id".to_string()])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // Attach a child that doesn't exist yet
    let unit2 = make_unit(vec![
        AlterTable {
            name: qname("parent"),
            actions: vec![AlterTableAction::AttachPartition {
                child: qname("missing"),
            }],
        }
        .into(),
    ]);
    apply(&mut catalog, &unit2);

    // Should not panic; no phantom entry created since child doesn't exist
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "No phantom entry should be created for missing child"
    );
}

#[test]
fn test_detach_partition_existing_child() {
    let mut catalog = CatalogBuilder::new()
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .table("child", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    let unit = make_unit(vec![
        AlterTable {
            name: qname("parent"),
            actions: vec![AlterTableAction::DetachPartition {
                child: qname("child"),
                concurrent: false,
            }],
        }
        .into(),
    ]);
    apply(&mut catalog, &unit);

    let child = catalog.get_table("child").expect("child should exist");
    assert!(
        child.parent_table.is_none(),
        "parent_table should be cleared"
    );
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "parent should have no children"
    );
}

#[test]
fn test_drop_parent_cascade_removes_children() {
    let mut catalog = CatalogBuilder::new()
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .table("child", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    let unit = make_unit(vec![
        DropTable::test(qname("parent"))
            .with_cascade(true)
            .with_if_exists(false)
            .into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(!catalog.has_table("parent"), "Parent should be removed");
    assert!(
        !catalog.has_table("child"),
        "Child should be removed by CASCADE"
    );
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "partition_children should be cleaned up"
    );
}

#[test]
fn test_drop_parent_cascade_recursive() {
    let mut catalog = CatalogBuilder::new()
        .table("grandparent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"])
                .partition_of("grandparent");
        })
        .table("child", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    let unit = make_unit(vec![
        DropTable::test(qname("grandparent"))
            .with_cascade(true)
            .with_if_exists(false)
            .into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(!catalog.has_table("grandparent"));
    assert!(!catalog.has_table("parent"));
    assert!(!catalog.has_table("child"));
    assert!(
        catalog.get_partition_children("grandparent").is_empty(),
        "partition_children for grandparent should be cleaned up"
    );
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "partition_children for parent should be cleaned up"
    );
}

#[test]
fn test_drop_parent_no_cascade_keeps_children() {
    let mut catalog = CatalogBuilder::new()
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .table("child", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    let unit = make_unit(vec![
        DropTable::test(qname("parent"))
            .with_cascade(false)
            .with_if_exists(false)
            .into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(!catalog.has_table("parent"), "Parent should be removed");
    assert!(
        catalog.has_table("child"),
        "Child should still exist without CASCADE"
    );
    // Child keeps stale parent_table
    let child = catalog.get_table("child").unwrap();
    assert_eq!(child.parent_table.as_deref(), Some("parent"));
    // partition_children entry for the dropped parent should be cleaned up
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "partition_children entry for dropped parent should be cleaned up"
    );
}

#[test]
fn test_drop_child_updates_parent_children() {
    let mut catalog = CatalogBuilder::new()
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .table("child", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    let unit = make_unit(vec![
        DropTable::test(qname("child")).with_if_exists(false).into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(catalog.has_table("parent"), "Parent should still exist");
    assert!(!catalog.has_table("child"), "Child should be removed");
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "Parent should have no children after child drop"
    );
}

#[test]
fn test_detach_partition_missing_child() {
    let mut catalog = CatalogBuilder::new()
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .build();

    // Detach a child that doesn't exist — should not panic
    let unit = make_unit(vec![
        AlterTable {
            name: qname("parent"),
            actions: vec![AlterTableAction::DetachPartition {
                child: qname("ghost"),
                concurrent: false,
            }],
        }
        .into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(catalog.has_table("parent"), "Parent should still exist");
    assert!(
        catalog.get_partition_children("parent").is_empty(),
        "No children should exist"
    );
}

#[test]
fn test_drop_parent_cascade_multiple_siblings() {
    let mut catalog = CatalogBuilder::new()
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
        .table("child_c", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    assert_eq!(catalog.get_partition_children("parent").len(), 3);

    let unit = make_unit(vec![
        DropTable::test(qname("parent"))
            .with_cascade(true)
            .with_if_exists(false)
            .into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(!catalog.has_table("parent"));
    assert!(!catalog.has_table("child_a"));
    assert!(!catalog.has_table("child_b"));
    assert!(!catalog.has_table("child_c"));
    assert!(catalog.get_partition_children("parent").is_empty());
}

// -----------------------------------------------------------------------
// DROP SCHEMA CASCADE catalog replay
// -----------------------------------------------------------------------

#[test]
fn test_drop_schema_cascade_removes_matching_tables() {
    let mut catalog = CatalogBuilder::new()
        .table("myschema.orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("myschema.customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("other.products", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();

    let unit = make_unit(vec![DropSchema::test("myschema").with_cascade(true).into()]);
    apply(&mut catalog, &unit);

    assert!(
        !catalog.has_table("myschema.orders"),
        "myschema.orders should be removed"
    );
    assert!(
        !catalog.has_table("myschema.customers"),
        "myschema.customers should be removed"
    );
    assert!(
        catalog.has_table("other.products"),
        "Tables in other schemas should be preserved"
    );
}

#[test]
fn test_drop_schema_no_cascade_is_noop() {
    let mut catalog = CatalogBuilder::new()
        .table("myschema.orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();

    let unit = make_unit(vec![DropSchema::test("myschema").into()]);
    apply(&mut catalog, &unit);

    assert!(
        catalog.has_table("myschema.orders"),
        "Table should remain when DROP SCHEMA has no CASCADE"
    );
}

#[test]
fn test_drop_schema_cascade_prefix_no_false_match() {
    let mut catalog = CatalogBuilder::new()
        .table("foo.t1", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("foobar.t2", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();

    let unit = make_unit(vec![DropSchema::test("foo").with_cascade(true).into()]);
    apply(&mut catalog, &unit);

    assert!(!catalog.has_table("foo.t1"), "foo.t1 should be removed");
    assert!(
        catalog.has_table("foobar.t2"),
        "foobar.t2 should NOT be removed by DROP SCHEMA foo CASCADE"
    );
}

#[test]
fn test_drop_schema_cascade_empty_schema_noop() {
    let mut catalog = CatalogBuilder::new()
        .table("other.orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();

    let unit = make_unit(vec![
        DropSchema::test("empty_schema").with_cascade(true).into(),
    ]);
    apply(&mut catalog, &unit);

    assert!(
        catalog.has_table("other.orders"),
        "Unrelated tables should be unaffected"
    );
}

#[test]
fn test_rename_partitioned_parent() {
    let mut catalog = CatalogBuilder::new()
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

    assert_eq!(catalog.get_partition_children("parent").len(), 2);

    let unit = make_unit(vec![IrNode::RenameTable {
        name: qname("parent"),
        new_name: "parent_v2".to_string(),
    }]);
    apply(&mut catalog, &unit);

    // Old key gone, new key exists
    assert!(!catalog.has_table("parent"));
    assert!(catalog.has_table("parent_v2"));

    // partition_children migrated to new key
    assert!(catalog.get_partition_children("parent").is_empty());
    let children = catalog.get_partition_children("parent_v2");
    assert_eq!(children.len(), 2);
    assert!(children.contains(&"child_a".to_string()));
    assert!(children.contains(&"child_b".to_string()));

    // Children's parent_table updated to new key
    let child_a = catalog.get_table("child_a").unwrap();
    assert_eq!(child_a.parent_table.as_deref(), Some("parent_v2"));
    let child_b = catalog.get_table("child_b").unwrap();
    assert_eq!(child_b.parent_table.as_deref(), Some("parent_v2"));

    // Renamed table retains its partition properties
    let parent = catalog.get_table("parent_v2").unwrap();
    assert!(parent.is_partitioned);
    assert_eq!(parent.display_name, "parent_v2");
}

#[test]
fn test_rename_partition_child() {
    let mut catalog = CatalogBuilder::new()
        .table("parent", |t| {
            t.column("id", "integer", false)
                .partitioned_by(PartitionStrategy::Range, &["id"]);
        })
        .table("child", |t| {
            t.column("id", "integer", false).partition_of("parent");
        })
        .build();

    assert_eq!(catalog.get_partition_children("parent"), &["child"]);

    let unit = make_unit(vec![IrNode::RenameTable {
        name: qname("child"),
        new_name: "child_v2".to_string(),
    }]);
    apply(&mut catalog, &unit);

    // Old key gone, new key exists
    assert!(!catalog.has_table("child"));
    assert!(catalog.has_table("child_v2"));

    // Parent's partition_children updated: old child removed, new child registered
    let children = catalog.get_partition_children("parent");
    assert_eq!(children, &["child_v2"]);

    // Renamed child retains parent_table
    let child = catalog.get_table("child_v2").unwrap();
    assert_eq!(child.parent_table.as_deref(), Some("parent"));
    assert_eq!(child.display_name, "child_v2");
}

// -----------------------------------------------------------------------
// ALTER INDEX ATTACH PARTITION
// -----------------------------------------------------------------------

#[test]
fn test_alter_index_attach_partition_flips_only() {
    let mut catalog = Catalog::new();

    // Create a partitioned table with an ON ONLY index
    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col("id", "integer", false)])
            .with_partition_by(PartitionStrategy::Range, vec!["id".to_string()])
            .into(),
        IrNode::CreateIndex(
            CreateIndex::test(Some("idx_parent".to_string()), qname("parent"))
                .with_columns(vec![IndexColumn::Column("id".to_string())])
                .with_only(true),
        ),
    ]);
    apply(&mut catalog, &unit1);

    // Verify the index starts as only=true
    let table = catalog.get_table("parent").unwrap();
    let idx = table
        .indexes
        .iter()
        .find(|i| i.name == "idx_parent")
        .unwrap();
    assert!(idx.only, "Index should start as ON ONLY");

    // ALTER INDEX ATTACH PARTITION flips only to false
    let unit2 = make_unit(vec![IrNode::AlterIndexAttachPartition {
        parent_index_name: "idx_parent".to_string(),
        child_index_name: QualifiedName::unqualified("idx_child"),
    }]);
    apply(&mut catalog, &unit2);

    let table = catalog.get_table("parent").unwrap();
    let idx = table
        .indexes
        .iter()
        .find(|i| i.name == "idx_parent")
        .unwrap();
    assert!(!idx.only, "ATTACH should flip only to false");
}

#[test]
fn test_alter_index_attach_partition_missing_index_is_noop() {
    let mut catalog = Catalog::new();

    let unit1 = make_unit(vec![
        CreateTable::test(qname("parent"))
            .with_columns(vec![col("id", "integer", false)])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    // ATTACH PARTITION on an index that doesn't exist — should not panic
    let unit2 = make_unit(vec![IrNode::AlterIndexAttachPartition {
        parent_index_name: "idx_nonexistent".to_string(),
        child_index_name: QualifiedName::unqualified("idx_child"),
    }]);
    apply(&mut catalog, &unit2);

    // Catalog unchanged
    assert!(catalog.has_table("parent"));
}

#[test]
fn test_rename_column_updates_check_expression() {
    let mut catalog = Catalog::new();

    let unit1 = make_unit(vec![
        CreateTable::test(qname("t"))
            .with_columns(vec![
                col("id", "integer", false),
                col("ts", "timestamptz", false),
            ])
            .with_constraints(vec![TableConstraint::Check {
                name: Some("chk_ts".to_string()),
                expression: "(ts >= '2024-01-01')".to_string(),
                not_valid: false,
            }])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    let unit2 = make_unit(vec![IrNode::RenameColumn {
        table: qname("t"),
        old_name: "ts".to_string(),
        new_name: "created_at".to_string(),
    }]);
    apply(&mut catalog, &unit2);

    let table = catalog.get_table("t").expect("table should exist");
    let check = table
        .constraints
        .iter()
        .find(|c| matches!(c, ConstraintState::Check { name: Some(n), .. } if n == "chk_ts"))
        .expect("CHECK constraint should still exist");

    if let ConstraintState::Check { expression, .. } = check {
        assert!(
            expression.contains("created_at"),
            "CHECK expression should contain 'created_at' after rename, got: {expression}"
        );
        assert!(
            !expression.contains("ts"),
            "CHECK expression should no longer contain 'ts' after rename, got: {expression}"
        );
    } else {
        panic!("expected Check constraint");
    }
}

#[test]
fn test_replace_column_in_expression_word_boundary() {
    // Simple replacement
    assert_eq!(
        replace_column_in_expression("(id > 0)", "id", "user_id"),
        "(user_id > 0)"
    );

    // Should NOT replace substring match: `id` inside `id_type`
    assert_eq!(
        replace_column_in_expression("(id_type = 'foo')", "id", "user_id"),
        "(id_type = 'foo')"
    );

    // Multiple occurrences
    assert_eq!(
        replace_column_in_expression("(ts >= '2024' AND ts < '2025')", "ts", "created_at"),
        "(created_at >= '2024' AND created_at < '2025')"
    );
}

#[test]
fn test_rename_column_updates_partition_by_columns() {
    let mut catalog = Catalog::new();

    let unit1 = make_unit(vec![
        CreateTable::test(qname("measurements"))
            .with_columns(vec![
                col("id", "bigint", false),
                col("ts", "timestamptz", false),
            ])
            .with_partition_by(PartitionStrategy::Range, vec!["ts".to_string()])
            .into(),
    ]);
    apply(&mut catalog, &unit1);

    let unit2 = make_unit(vec![IrNode::RenameColumn {
        table: qname("measurements"),
        old_name: "ts".to_string(),
        new_name: "created_at".to_string(),
    }]);
    apply(&mut catalog, &unit2);

    let table = catalog
        .get_table("measurements")
        .expect("table should exist");
    let pb = table
        .partition_by
        .as_ref()
        .expect("partition_by should be set");
    assert_eq!(
        pb.columns,
        vec!["created_at".to_string()],
        "partition_by columns should be updated after rename"
    );
}
