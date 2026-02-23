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
        IrNode::RenameTable { name, new_name } => apply_rename_table(catalog, name, new_name),
        IrNode::RenameColumn {
            table,
            old_name,
            new_name,
        } => apply_rename_column(catalog, table, old_name, new_name),
        IrNode::TruncateTable(_) | IrNode::Cluster(_) => { /* no schema state change */ }
        IrNode::InsertInto(_) | IrNode::UpdateTable(_) | IrNode::DeleteFrom(_) => {
            /* DML: no schema change */
        }
        IrNode::Unparseable { table_hint, .. } => apply_unparseable(catalog, table_hint),
        IrNode::Ignored { .. } => { /* no-op */ }
    }
}

/// Handle CREATE TABLE: insert a new table into the catalog with columns,
/// constraints, and indexes derived from the statement.
///
/// When `IF NOT EXISTS` is used and the table already exists, the statement
/// is a no-op in PostgreSQL. We mirror that by keeping the existing catalog
/// state and emitting a warning — the migration chain is ambiguous at that
/// point (which definition is the truth?).
fn apply_create_table(catalog: &mut Catalog, ct: &CreateTable) {
    let table_key = ct.name.catalog_key().to_string();

    if catalog.has_table(&table_key) {
        if ct.if_not_exists {
            eprintln!(
                "warning: CREATE TABLE IF NOT EXISTS `{}` skipped — table already exists in catalog. \
                 The migration chain may be inconsistent.",
                ct.name.display_name()
            );
            return;
        }
        eprintln!(
            "warning: CREATE TABLE `{}` overwrites existing table in catalog. \
             The table may have been dropped outside tracked migrations, or this is a duplicate definition.",
            ct.name.display_name()
        );
    }

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
                    using_index: None,
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
        let Some(table) = catalog.get_table_mut(&table_key) else {
            return;
        };

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
                                using_index: None,
                            },
                        );
                        indexes_to_register.push(format!("{}_pkey", table.name));
                    }
                }
                AlterTableAction::DropColumn { name } => {
                    // Collect index names that will be removed by the column drop.
                    for idx in &table.indexes {
                        if idx.column_names().any(|c| c == name) {
                            indexes_to_unregister.push(idx.name.clone());
                        }
                    }
                    table.remove_column(name);
                }
                AlterTableAction::AddConstraint(constraint) => {
                    // Track synthetic PK indexes created by apply_table_constraint.
                    // Only when there's no USING INDEX (with USING INDEX the index already exists).
                    if matches!(
                        constraint,
                        TableConstraint::PrimaryKey {
                            using_index: None,
                            ..
                        }
                    ) {
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
                AlterTableAction::SetNotNull { column_name } => {
                    if let Some(col) = table.get_column_mut(column_name) {
                        col.nullable = false;
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
///
/// When `IF NOT EXISTS` is used and a same-named index already exists,
/// PostgreSQL treats it as a no-op. We keep the existing index and warn.
fn apply_create_index(catalog: &mut Catalog, ci: &CreateIndex) {
    let table_key = ci.table_name.catalog_key().to_string();

    let index_name = ci.index_name.clone().unwrap_or_default();

    if !index_name.is_empty() && catalog.get_index(&index_name).is_some() {
        if ci.if_not_exists {
            eprintln!(
                "warning: CREATE INDEX IF NOT EXISTS `{}` skipped — index already exists in catalog. \
                 The migration chain may be inconsistent.",
                index_name
            );
            return;
        }
        eprintln!(
            "warning: CREATE INDEX `{}` overwrites existing index in catalog. \
             The index may have been dropped outside tracked migrations, or this is a duplicate definition.",
            index_name
        );
    }

    let Some(table) = catalog.get_table_mut(&table_key) else {
        return;
    };

    let entries: Vec<IndexEntry> = ci.columns.iter().map(IndexEntry::from).collect();

    table.indexes.push(IndexState {
        name: index_name.clone(),
        entries,
        unique: ci.unique,
        where_clause: ci.where_clause.clone(),
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

/// Handle RENAME TABLE: move the table state to a new key.
fn apply_rename_table(catalog: &mut Catalog, name: &QualifiedName, new_name: &str) {
    let old_key = name.catalog_key().to_string();
    if let Some(mut table) = catalog.remove_table(&old_key) {
        // Build the new key using the same schema as the old name.
        let new_key = match &name.schema {
            Some(schema) => format!("{}.{}", schema, new_name),
            None => {
                // After normalization, schema should always be set. Fall back to
                // extracting it from the old catalog key ("schema.name" format).
                if let Some(dot) = old_key.find('.') {
                    format!("{}.{}", &old_key[..dot], new_name)
                } else {
                    new_name.to_string()
                }
            }
        };
        table.name = new_key.clone();
        table.display_name = new_name.to_string();
        catalog.insert_table(table);
    }
}

/// Handle RENAME COLUMN: rename a column in a table, updating indexes and constraints.
fn apply_rename_column(
    catalog: &mut Catalog,
    table_name: &QualifiedName,
    old_name: &str,
    new_name: &str,
) {
    let table_key = table_name.catalog_key().to_string();
    let Some(table) = catalog.get_table_mut(&table_key) else {
        return;
    };

    // Rename the column itself.
    if let Some(col) = table.get_column_mut(old_name) {
        col.name = new_name.to_string();
    }

    // Update indexes that reference the old column name.
    // Expression entries are left as-is (they contain deparsed SQL, not column names).
    // In PostgreSQL, expression text is updated internally on rename, so our catalog's
    // expression text may be stale after a rename — a known simplification.
    for idx in &mut table.indexes {
        for entry in &mut idx.entries {
            if let IndexEntry::Column(col) = entry
                && *col == old_name
            {
                *col = new_name.to_string();
            }
        }
    }

    // Update constraints that reference the old column name.
    let table_name_key = table.name.clone();
    for constraint in &mut table.constraints {
        match constraint {
            ConstraintState::PrimaryKey { columns } | ConstraintState::Unique { columns, .. } => {
                for col in columns {
                    if *col == old_name {
                        *col = new_name.to_string();
                    }
                }
            }
            ConstraintState::ForeignKey {
                columns,
                ref_table,
                ref_columns,
                ..
            } => {
                for col in columns.iter_mut() {
                    if *col == old_name {
                        *col = new_name.to_string();
                    }
                }
                // For self-referencing FKs, also rename matching ref_columns.
                if *ref_table == table_name_key {
                    for col in ref_columns.iter_mut() {
                        if *col == old_name {
                            *col = new_name.to_string();
                        }
                    }
                }
            }
            ConstraintState::Check { .. } => {}
        }
    }
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
        TableConstraint::PrimaryKey {
            columns,
            using_index,
        } => {
            table.has_primary_key = true;
            // When USING INDEX is set, IR columns are empty — resolve from the
            // referenced index so downstream rules (e.g. PGM014) see the real columns.
            // Expression entries are intentionally excluded (column_names() skips them)
            // since PK constraints reference plain columns; in practice PostgreSQL
            // rejects ADD PRIMARY KEY USING INDEX on expression indexes.
            let resolved_columns = if columns.is_empty() {
                using_index
                    .as_ref()
                    .and_then(|idx_name| {
                        table
                            .indexes
                            .iter()
                            .find(|idx| idx.name == *idx_name)
                            .map(|idx| idx.column_names().map(str::to_string).collect())
                    })
                    .unwrap_or_default()
            } else {
                columns.clone()
            };
            table.constraints.push(ConstraintState::PrimaryKey {
                columns: resolved_columns,
            });
            // Only create a synthetic PK index when there's no USING INDEX.
            // With USING INDEX, the referenced index already exists in the catalog.
            if using_index.is_none() {
                table.indexes.push(IndexState {
                    name: format!("{}_pkey", table.name),
                    entries: columns
                        .iter()
                        .map(|c| IndexEntry::Column(c.clone()))
                        .collect(),
                    unique: true,
                    where_clause: None,
                });
            }
        }
        TableConstraint::ForeignKey {
            name,
            columns,
            ref_table,
            ref_columns,
            not_valid,
        } => {
            table.constraints.push(ConstraintState::ForeignKey {
                name: name.clone(),
                columns: columns.clone(),
                ref_table: ref_table.catalog_key().to_string(),
                ref_table_display: ref_table.display_name(),
                ref_columns: ref_columns.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Unique {
            name,
            columns,
            using_index,
        } => {
            // When USING INDEX is set, IR columns are empty — resolve from the
            // referenced index so downstream constraint checks see the real columns.
            let resolved_columns = if columns.is_empty() {
                using_index
                    .as_ref()
                    .and_then(|idx_name| {
                        table
                            .indexes
                            .iter()
                            .find(|idx| idx.name == *idx_name)
                            .map(|idx| idx.column_names().map(str::to_string).collect())
                    })
                    .unwrap_or_default()
            } else {
                columns.clone()
            };
            table.constraints.push(ConstraintState::Unique {
                name: name.clone(),
                columns: resolved_columns,
            });
        }
        TableConstraint::Check {
            name, not_valid, ..
        } => {
            table.constraints.push(ConstraintState::Check {
                name: name.clone(),
                not_valid: *not_valid,
            });
        }
    }
}

#[cfg(test)]
mod tests {
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
                ],
            }
            .into(),
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
                .with_columns(vec![IndexColumn::Expression("lower(email)".to_string())])
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
        // Column entry should be renamed; expression entry should be unchanged.
        assert_eq!(
            idx.entries,
            vec![
                IndexEntry::Column("order_status".to_string()),
                IndexEntry::Expression("lower(active::text)".to_string()),
            ]
        );
    }

    #[test]
    fn test_drop_column_retains_expression_index_referencing_column() {
        // Known limitation: dropping a column does not remove expression indexes
        // that reference it (e.g. `lower(email)`), because expression text is opaque.
        // PostgreSQL would drop such indexes, but our catalog retains them.
        let mut catalog = Catalog::new();

        let unit = make_unit(vec![
            CreateTable::test(qname("users"))
                .with_columns(vec![
                    col("id", "integer", false),
                    col("email", "text", false),
                ])
                .into(),
            CreateIndex::test(Some("idx_email_lower".to_string()), qname("users"))
                .with_columns(vec![IndexColumn::Expression("lower(email)".to_string())])
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
        // Expression index is retained because column_names() doesn't see "email"
        // inside the expression text "lower(email)".
        assert_eq!(
            table.indexes.len(),
            1,
            "Expression index is retained (known limitation: expression text is opaque)"
        );
    }
}
