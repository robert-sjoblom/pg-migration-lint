//! Catalog replay engine — applies IR nodes to mutate the catalog.
//!
//! The replay engine processes migration units in order, applying each
//! IR statement to build up the table catalog. This is the core of the
//! single-pass replay strategy: the pipeline calls [`apply`] for each
//! migration unit, and the catalog accumulates state over time.

use crate::catalog::types::*;
use crate::input::MigrationUnit;
use crate::parser::ir::*;

/// Apply a single migration unit's IR nodes to mutate the catalog.
///
/// Called by the pipeline for each unit in order. Each statement in the
/// unit is applied sequentially. Statements that reference tables not
/// present in the catalog are silently skipped (the table may belong
/// to a different schema or be managed outside the tracked migrations).
pub fn apply(catalog: &mut Catalog, unit: &MigrationUnit) {
    for located in &unit.statements {
        apply_node(catalog, &located.node);
    }
}

/// Apply a single IR node to the catalog.
fn apply_node(catalog: &mut Catalog, node: &IrNode) {
    match node {
        IrNode::CreateTable(ct) => apply_create_table(catalog, ct),
        IrNode::AlterTable(at) => apply_alter_table(catalog, at),
        IrNode::CreateIndex(ci) => apply_create_index(catalog, ci),
        IrNode::DropIndex(di) => apply_drop_index(catalog, di),
        IrNode::DropTable(dt) => apply_drop_table(catalog, dt),
        IrNode::Unparseable { table_hint, .. } => apply_unparseable(catalog, table_hint),
        IrNode::Ignored { .. } => { /* no-op */ }
    }
}

/// Handle CREATE TABLE: insert a new table into the catalog with columns,
/// constraints, and indexes derived from the statement.
fn apply_create_table(catalog: &mut Catalog, ct: &CreateTable) {
    let table_key = ct.name.catalog_key().to_string();

    let mut table = TableState {
        name: table_key,
        display_name: ct.name.display_name(),
        columns: Vec::new(),
        indexes: Vec::new(),
        constraints: Vec::new(),
        has_primary_key: false,
        incomplete: false,
    };

    // Convert columns
    for col in &ct.columns {
        table.columns.push(column_def_to_state(col));

        // Handle inline PK on the column definition
        if col.is_inline_pk {
            apply_table_constraint(
                &mut table,
                &TableConstraint::PrimaryKey {
                    columns: vec![col.name.clone()],
                },
            );
        }
    }

    // Convert table-level constraints
    for constraint in &ct.constraints {
        apply_table_constraint(&mut table, constraint);
    }

    catalog.insert_table(table);
}

/// Handle ALTER TABLE: apply each action to the existing table.
/// If the table does not exist in the catalog, silently skip.
fn apply_alter_table(catalog: &mut Catalog, at: &AlterTable) {
    let table_key = at.name.catalog_key().to_string();

    // If the table doesn't exist, silently skip. It may be in a different
    // schema or managed outside our tracked migrations.
    if !catalog.has_table(&table_key) {
        return;
    }

    // Collect indexes to register/unregister so we can update the reverse map
    // after releasing the mutable borrow on the table.
    let mut indexes_to_register: Vec<String> = Vec::new();
    let mut indexes_to_unregister: Vec<String> = Vec::new();

    {
        let table = catalog.get_table_mut(&table_key).unwrap();

        for action in &at.actions {
            match action {
                AlterTableAction::AddColumn(col_def) => {
                    table.columns.push(column_def_to_state(col_def));

                    // Handle inline PK on the added column
                    if col_def.is_inline_pk {
                        apply_table_constraint(
                            table,
                            &TableConstraint::PrimaryKey {
                                columns: vec![col_def.name.clone()],
                            },
                        );
                        indexes_to_register.push(format!("{}_pkey", table.name));
                    }
                }
                AlterTableAction::DropColumn { name } => {
                    // Collect index names that will be removed by the column drop.
                    for idx in &table.indexes {
                        if idx.columns.iter().any(|c| c == name) {
                            indexes_to_unregister.push(idx.name.clone());
                        }
                    }
                    table.remove_column(name);
                }
                AlterTableAction::AddConstraint(constraint) => {
                    // Track synthetic PK indexes created by apply_table_constraint.
                    if matches!(constraint, TableConstraint::PrimaryKey { .. }) {
                        indexes_to_register.push(format!("{}_pkey", table.name));
                    }
                    apply_table_constraint(table, constraint);
                }
                AlterTableAction::AlterColumnType {
                    column_name,
                    new_type,
                    ..
                } => {
                    if let Some(col) = table.get_column_mut(column_name) {
                        col.type_name = new_type.clone();
                    }
                }
                AlterTableAction::Other { .. } => { /* ignore unmodeled actions */ }
            }
        }
    }

    // Update reverse map outside the table borrow.
    for name in indexes_to_unregister {
        catalog.unregister_index(&name);
    }
    for name in indexes_to_register {
        catalog.register_index(&name, &table_key);
    }
}

/// Handle CREATE INDEX: add an index to the target table.
/// If the table does not exist in the catalog, silently skip.
fn apply_create_index(catalog: &mut Catalog, ci: &CreateIndex) {
    let table_key = ci.table_name.catalog_key().to_string();

    let Some(table) = catalog.get_table_mut(&table_key) else {
        return;
    };

    let index_name = ci.index_name.clone().unwrap_or_default();
    let columns: Vec<String> = ci.columns.iter().map(|ic| ic.name.clone()).collect();

    table.indexes.push(IndexState {
        name: index_name.clone(),
        columns,
        unique: ci.unique,
    });

    // Register after confirming the table exists, to avoid ghost entries.
    catalog.register_index(&index_name, &table_key);
}

/// Handle DROP INDEX: find and remove the named index from whichever table has it.
/// If no table has the index, silently skip.
fn apply_drop_index(catalog: &mut Catalog, di: &DropIndex) {
    let Some(table_name) = catalog.table_for_index(&di.index_name).map(str::to_string) else {
        return;
    };

    catalog.unregister_index(&di.index_name);

    if let Some(table) = catalog.get_table_mut(&table_name) {
        table.indexes.retain(|idx| idx.name != di.index_name);
    }
}

/// Handle DROP TABLE: remove the table from the catalog entirely.
fn apply_drop_table(catalog: &mut Catalog, dt: &DropTable) {
    let table_key = dt.name.catalog_key();
    catalog.remove_table(table_key);
}

/// Handle Unparseable: if a table_hint is provided, mark that table as incomplete.
fn apply_unparseable(catalog: &mut Catalog, table_hint: &Option<String>) {
    if let Some(hint) = table_hint
        && let Some(table) = catalog.get_table_mut(hint)
    {
        table.incomplete = true;
    }
}

/// Convert an IR ColumnDef to a catalog ColumnState.
fn column_def_to_state(col: &ColumnDef) -> ColumnState {
    ColumnState {
        name: col.name.clone(),
        type_name: col.type_name.clone(),
        nullable: col.nullable,
        has_default: col.default_expr.is_some(),
        default_expr: col.default_expr.clone(),
    }
}

/// Apply a table constraint to the table state, updating constraints list
/// and has_primary_key flag as needed.
fn apply_table_constraint(table: &mut TableState, constraint: &TableConstraint) {
    match constraint {
        TableConstraint::PrimaryKey { columns } => {
            table.has_primary_key = true;
            table.constraints.push(ConstraintState::PrimaryKey {
                columns: columns.clone(),
            });
            // PostgreSQL automatically creates a unique index for PKs.
            // Track it so has_covering_index() finds PK coverage for FKs.
            table.indexes.push(IndexState {
                name: format!("{}_pkey", table.name),
                columns: columns.clone(),
                unique: true,
            });
        }
        TableConstraint::ForeignKey {
            name,
            columns,
            ref_table,
            ref_columns,
        } => {
            table.constraints.push(ConstraintState::ForeignKey {
                name: name.clone(),
                columns: columns.clone(),
                ref_table: ref_table.catalog_key().to_string(),
                ref_table_display: ref_table.display_name(),
                ref_columns: ref_columns.clone(),
            });
        }
        TableConstraint::Unique { name, columns } => {
            table.constraints.push(ConstraintState::Unique {
                name: name.clone(),
                columns: columns.clone(),
            });
        }
        TableConstraint::Check { name, .. } => {
            table
                .constraints
                .push(ConstraintState::Check { name: name.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        ColumnDef {
            name: name.to_string(),
            type_name: simple_type(type_name),
            nullable,
            default_expr: None,
            is_inline_pk: false,
            is_serial: false,
        }
    }

    /// Helper: create a ColumnDef with inline PK.
    fn col_pk(name: &str, type_name: &str) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            type_name: simple_type(type_name),
            nullable: false,
            default_expr: None,
            is_inline_pk: true,
            is_serial: false,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Create then drop
    // -----------------------------------------------------------------------
    #[test]
    fn test_create_then_drop() {
        let mut catalog = Catalog::new();

        let unit = make_unit(vec![
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col_pk("id", "integer")],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_id".to_string()),
                table_name: qname("t"),
                columns: vec![IndexColumn {
                    name: "id".to_string(),
                }],
                unique: false,
                concurrent: false,
            }),
            IrNode::DropTable(DropTable { name: qname("t") }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::AddColumn(col("name", "text", true))],
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_name".to_string()),
                table_name: qname("t"),
                columns: vec![IndexColumn {
                    name: "name".to_string(),
                }],
                unique: false,
                concurrent: false,
            }),
        ]);

        apply(&mut catalog, &unit);

        let table = catalog.get_table("t").expect("table should exist");
        assert_eq!(table.columns.len(), 2, "Should have 2 columns after ALTER");
        assert_eq!(table.columns[0].name, "id");
        assert_eq!(table.columns[1].name, "name");
        assert_eq!(table.indexes.len(), 1, "Should have 1 index");
        assert_eq!(table.indexes[0].name, "idx_name");
        assert_eq!(table.indexes[0].columns, vec!["name".to_string()]);
    }

    // -----------------------------------------------------------------------
    // Test 3: FK tracks referencing columns
    // -----------------------------------------------------------------------
    #[test]
    fn test_fk_tracks_referencing_columns() {
        let mut catalog = Catalog::new();

        // Create parent table with PK
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("parent"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // Create child table with FK to parent
        let unit2 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("child"),
            columns: vec![col("pid", "integer", false)],
            constraints: vec![TableConstraint::ForeignKey {
                name: Some("fk_parent".to_string()),
                columns: vec!["pid".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
            }],
            temporary: false,
        })]);
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("a", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_a".to_string()),
                table_name: qname("t"),
                columns: vec![IndexColumn {
                    name: "a".to_string(),
                }],
                unique: false,
                concurrent: false,
            }),
            IrNode::DropIndex(DropIndex {
                index_name: "idx_a".to_string(),
                concurrent: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("x", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::AlterColumnType {
                    column_name: "x".to_string(),
                    new_type: simple_type("bigint"),
                    old_type: Some(simple_type("integer")),
                }],
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("a", "integer", false), col("b", "text", true)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_ab".to_string()),
                table_name: qname("t"),
                columns: vec![
                    IndexColumn {
                        name: "a".to_string(),
                    },
                    IndexColumn {
                        name: "b".to_string(),
                    },
                ],
                unique: false,
                concurrent: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "b".to_string(),
                }],
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::DropTable(DropTable { name: qname("t") }),
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "bigint", false)],
                constraints: vec![],
                temporary: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![
                    col("a", "integer", false),
                    col("b", "integer", false),
                    col("c", "integer", false),
                ],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_abc".to_string()),
                table_name: qname("t"),
                columns: vec![
                    IndexColumn {
                        name: "a".to_string(),
                    },
                    IndexColumn {
                        name: "b".to_string(),
                    },
                    IndexColumn {
                        name: "c".to_string(),
                    },
                ],
                unique: false,
                concurrent: false,
            }),
        ]);

        apply(&mut catalog, &unit);

        let table = catalog.get_table("t").expect("table should exist");
        assert_eq!(table.indexes.len(), 1);
        assert_eq!(
            table.indexes[0].columns,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    // -----------------------------------------------------------------------
    // Test 10: Inline PK normalizes
    // -----------------------------------------------------------------------
    #[test]
    fn test_inline_pk_normalizes() {
        let mut catalog = Catalog::new();

        let unit = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("t"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);

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

        let unit = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("t"),
            columns: vec![col("id", "integer", false)],
            constraints: vec![TableConstraint::PrimaryKey {
                columns: vec!["id".to_string()],
            }],
            temporary: false,
        })]);

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
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("parent"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // The parser normalizes inline FK (REFERENCES) into a TableConstraint.
        // So this simulates: CREATE TABLE child(pid int REFERENCES parent(id))
        let unit2 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("child"),
            columns: vec![col("pid", "integer", false)],
            constraints: vec![TableConstraint::ForeignKey {
                name: None,
                columns: vec!["pid".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
            }],
            temporary: false,
        })]);
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
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("parent"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // Table-level FK: FOREIGN KEY (pid) REFERENCES parent(id)
        let unit2 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("child"),
            columns: vec![col("pid", "integer", false)],
            constraints: vec![TableConstraint::ForeignKey {
                name: Some("fk_parent".to_string()),
                columns: vec!["pid".to_string()],
                ref_table: qname("parent"),
                ref_columns: vec!["id".to_string()],
            }],
            temporary: false,
        })]);
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

        let unit = make_unit(vec![IrNode::AlterTable(AlterTable {
            name: qname("nonexistent"),
            actions: vec![AlterTableAction::AddColumn(col("x", "integer", false))],
        })]);

        apply(&mut catalog, &unit);

        assert!(
            !catalog.has_table("nonexistent"),
            "Should not create table from ALTER"
        );
    }

    #[test]
    fn test_create_index_on_nonexistent_table_silently_skips() {
        let mut catalog = Catalog::new();

        let unit = make_unit(vec![IrNode::CreateIndex(CreateIndex {
            index_name: Some("idx_x".to_string()),
            table_name: qname("nonexistent"),
            columns: vec![IndexColumn {
                name: "x".to_string(),
            }],
            unique: false,
            concurrent: false,
        })]);

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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("a", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_a".to_string()),
                table_name: qname("t"),
                columns: vec![IndexColumn {
                    name: "a".to_string(),
                }],
                unique: false,
                concurrent: false,
            }),
            IrNode::DropIndex(DropIndex {
                index_name: "idx_nonexistent".to_string(),
                concurrent: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
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

        let unit = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("t"),
            columns: vec![ColumnDef {
                name: "created_at".to_string(),
                type_name: simple_type("timestamptz"),
                nullable: false,
                default_expr: Some(DefaultExpr::FunctionCall {
                    name: "now".to_string(),
                    args: vec![],
                }),
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        })]);

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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("email", "text", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: Some("idx_email_unique".to_string()),
                table_name: qname("t"),
                columns: vec![IndexColumn {
                    name: "email".to_string(),
                }],
                unique: true,
                concurrent: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false), col("email", "text", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![
                    AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                        columns: vec!["id".to_string()],
                    }),
                    AlterTableAction::AddConstraint(TableConstraint::Unique {
                        name: Some("uk_email".to_string()),
                        columns: vec!["email".to_string()],
                    }),
                    AlterTableAction::AddConstraint(TableConstraint::Check {
                        name: Some("ck_email".to_string()),
                        expression: "email <> ''".to_string(),
                    }),
                ],
            }),
        ]);

        apply(&mut catalog, &unit);

        let table = catalog.get_table("t").expect("table should exist");
        assert!(table.has_primary_key, "PK should be set via ALTER TABLE");
        assert_eq!(
            table.constraints.len(),
            3,
            "Should have PK, Unique, and Check constraints"
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
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("users"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // Unit 2: Add column
        let unit2 = make_unit(vec![IrNode::AlterTable(AlterTable {
            name: qname("users"),
            actions: vec![AlterTableAction::AddColumn(col("name", "text", true))],
        })]);
        apply(&mut catalog, &unit2);

        // Unit 3: Add index
        let unit3 = make_unit(vec![IrNode::CreateIndex(CreateIndex {
            index_name: Some("idx_users_name".to_string()),
            table_name: qname("users"),
            columns: vec![IndexColumn {
                name: "name".to_string(),
            }],
            unique: false,
            concurrent: true,
        })]);
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::Other {
                    description: "SET TABLESPACE fast_ssd".to_string(),
                }],
            }),
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
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: QualifiedName::qualified("public", "orders"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // Create "audit.orders" — same table name, different schema
        let unit2 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: QualifiedName::qualified("audit", "orders"),
            columns: vec![col("log_id", "integer", false), col("data", "text", true)],
            constraints: vec![],
            temporary: false,
        })]);
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("a", "integer", false)],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::CreateIndex(CreateIndex {
                index_name: None,
                table_name: qname("t"),
                columns: vec![IndexColumn {
                    name: "a".to_string(),
                }],
                unique: false,
                concurrent: false,
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col_pk("id", "integer")],
                constraints: vec![],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "id".to_string(),
                }],
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("a", "integer", false), col("b", "integer", false)],
                constraints: vec![TableConstraint::PrimaryKey {
                    columns: vec!["a".to_string(), "b".to_string()],
                }],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "a".to_string(),
                }],
            }),
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
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("parent"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // Create child table with FK, then drop the FK column
        let unit2 = make_unit(vec![
            IrNode::CreateTable(CreateTable {
                name: qname("child"),
                columns: vec![
                    col("id", "integer", false),
                    col("customer_id", "integer", true),
                ],
                constraints: vec![TableConstraint::ForeignKey {
                    name: Some("fk_customer".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: qname("parent"),
                    ref_columns: vec!["id".to_string()],
                }],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("child"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "customer_id".to_string(),
                }],
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![col("id", "integer", false), col("email", "text", false)],
                constraints: vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                }],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "email".to_string(),
                }],
            }),
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
        let unit1 = make_unit(vec![IrNode::CreateTable(CreateTable {
            name: qname("parent"),
            columns: vec![col_pk("id", "integer")],
            constraints: vec![],
            temporary: false,
        })]);
        apply(&mut catalog, &unit1);

        // Create table with PK on (id) and FK on (customer_id), then drop customer_id
        let unit2 = make_unit(vec![
            IrNode::CreateTable(CreateTable {
                name: qname("orders"),
                columns: vec![col_pk("id", "integer"), col("customer_id", "integer", true)],
                constraints: vec![TableConstraint::ForeignKey {
                    name: Some("fk_customer".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: qname("parent"),
                    ref_columns: vec!["id".to_string()],
                }],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("orders"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "customer_id".to_string(),
                }],
            }),
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
            IrNode::CreateTable(CreateTable {
                name: qname("t"),
                columns: vec![
                    col("id", "integer", false),
                    col("amount", "integer", false),
                    col("extra", "text", true),
                ],
                constraints: vec![TableConstraint::Check {
                    name: Some("chk_positive".to_string()),
                    expression: "amount > 0".to_string(),
                }],
                temporary: false,
            }),
            IrNode::AlterTable(AlterTable {
                name: qname("t"),
                actions: vec![AlterTableAction::DropColumn {
                    name: "extra".to_string(),
                }],
            }),
        ]);

        apply(&mut catalog, &unit);

        let table = catalog.get_table("t").expect("table should exist");
        assert!(
            table.constraints.iter().any(
                |c| matches!(c, ConstraintState::Check { name: Some(n) } if n == "chk_positive")
            ),
            "Check constraint should be preserved when dropping an unrelated column"
        );
    }
}
