//! Schema normalization for migration IR nodes.
//!
//! After parsing, unqualified table names (e.g., `orders`) lack a schema prefix.
//! This module walks every `QualifiedName` in the IR and assigns a configurable
//! default schema so that `orders` and `public.orders` resolve to the same
//! catalog key.

use crate::input::MigrationUnit;
use crate::parser::ir::*;

/// Assign the default schema to every unqualified `QualifiedName` in the
/// migration history.
///
/// Must be called **after** parsing and **before** catalog replay so that
/// all catalog keys are schema-qualified.
pub fn normalize_schemas(units: &mut [MigrationUnit], default_schema: &str) {
    for unit in units.iter_mut() {
        for located in &mut unit.statements {
            normalize_node(&mut located.node, default_schema);
        }
    }
}

/// Normalize a single IR node, calling `set_default_schema` on every
/// `QualifiedName` reachable from it.
fn normalize_node(node: &mut IrNode, default_schema: &str) {
    match node {
        IrNode::CreateTable(ct) => {
            ct.name.set_default_schema(default_schema);
            if let Some(ref mut parent) = ct.partition_of {
                parent.set_default_schema(default_schema);
            }
            for constraint in &mut ct.constraints {
                normalize_constraint(constraint, default_schema);
            }
        }
        IrNode::AlterTable(at) => {
            at.name.set_default_schema(default_schema);
            for action in &mut at.actions {
                match action {
                    AlterTableAction::AddConstraint(constraint) => {
                        normalize_constraint(constraint, default_schema);
                    }
                    AlterTableAction::AttachPartition { child } => {
                        child.set_default_schema(default_schema);
                    }
                    AlterTableAction::DetachPartition { child, .. } => {
                        child.set_default_schema(default_schema);
                    }
                    _ => {}
                }
            }
        }
        IrNode::CreateIndex(ci) => {
            ci.table_name.set_default_schema(default_schema);
        }
        IrNode::DropTable(dt) => {
            dt.name.set_default_schema(default_schema);
        }
        IrNode::TruncateTable(tt) => {
            tt.name.set_default_schema(default_schema);
        }
        IrNode::Unparseable { table_hint, .. } => {
            if let Some(hint) = table_hint
                && !hint.contains('.')
            {
                *hint = format!("{}.{}", default_schema, hint);
            }
        }
        IrNode::RenameTable { name, .. } => {
            name.set_default_schema(default_schema);
        }
        IrNode::RenameColumn { table, .. } => {
            table.set_default_schema(default_schema);
        }
        IrNode::InsertInto(ii) => {
            ii.table_name.set_default_schema(default_schema);
        }
        IrNode::UpdateTable(ut) => {
            ut.table_name.set_default_schema(default_schema);
        }
        IrNode::DeleteFrom(df) => {
            df.table_name.set_default_schema(default_schema);
        }
        IrNode::Cluster(c) => {
            c.table.set_default_schema(default_schema);
        }
        IrNode::AlterIndexAttachPartition {
            child_index_name, ..
        } => {
            child_index_name.set_default_schema(default_schema);
        }
        // DropIndex only has index_name: String — no QualifiedName to normalize.
        // AlterIndexAttachPartition parent_index_name is a plain String (like DropIndex).
        IrNode::DropIndex(_) | IrNode::Ignored { .. } => {}
    }
}

/// Normalize QualifiedName references inside a table constraint.
fn normalize_constraint(constraint: &mut TableConstraint, default_schema: &str) {
    if let TableConstraint::ForeignKey { ref_table, .. } = constraint {
        ref_table.set_default_schema(default_schema);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper to wrap IR nodes into a MigrationUnit for testing.
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

    #[test]
    fn test_normalize_create_table_name() {
        let mut units = vec![make_unit(vec![
            CreateTable::test(QualifiedName::unqualified("orders")).into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::CreateTable(ct) = &units[0].statements[0].node {
            assert_eq!(ct.name.catalog_key(), "public.orders");
            assert_eq!(ct.name.schema, Some("public".to_string()));
        } else {
            panic!("Expected CreateTable");
        }
    }

    #[test]
    fn test_normalize_preserves_existing_schema() {
        let mut units = vec![make_unit(vec![
            CreateTable::test(QualifiedName::qualified("myschema", "orders")).into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::CreateTable(ct) = &units[0].statements[0].node {
            assert_eq!(ct.name.catalog_key(), "myschema.orders");
            assert_eq!(ct.name.schema, Some("myschema".to_string()));
        } else {
            panic!("Expected CreateTable");
        }
    }

    #[test]
    fn test_normalize_fk_ref_table_in_create_table() {
        let mut units = vec![make_unit(vec![
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_constraints(vec![TableConstraint::ForeignKey {
                    name: None,
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                }])
                .into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::CreateTable(ct) = &units[0].statements[0].node {
            if let TableConstraint::ForeignKey { ref_table, .. } = &ct.constraints[0] {
                assert_eq!(ref_table.catalog_key(), "public.customers");
            } else {
                panic!("Expected ForeignKey constraint");
            }
        } else {
            panic!("Expected CreateTable");
        }
    }

    #[test]
    fn test_normalize_alter_table_name() {
        let mut units = vec![make_unit(vec![IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "col".to_string(),
                type_name: TypeName::simple("text"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        })])];

        normalize_schemas(&mut units, "public");

        if let IrNode::AlterTable(at) = &units[0].statements[0].node {
            assert_eq!(at.name.catalog_key(), "public.orders");
        } else {
            panic!("Expected AlterTable");
        }
    }

    #[test]
    fn test_normalize_alter_table_fk_constraint() {
        let mut units = vec![make_unit(vec![IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_customer".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        })])];

        normalize_schemas(&mut units, "public");

        if let IrNode::AlterTable(at) = &units[0].statements[0].node {
            if let AlterTableAction::AddConstraint(TableConstraint::ForeignKey {
                ref_table, ..
            }) = &at.actions[0]
            {
                assert_eq!(ref_table.catalog_key(), "public.customers");
            } else {
                panic!("Expected AddConstraint ForeignKey");
            }
        } else {
            panic!("Expected AlterTable");
        }
    }

    #[test]
    fn test_normalize_create_index_table_name() {
        let mut units = vec![make_unit(vec![
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_columns(vec![IndexColumn::Column("status".to_string())])
            .into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::CreateIndex(ci) = &units[0].statements[0].node {
            assert_eq!(ci.table_name.catalog_key(), "public.orders");
        } else {
            panic!("Expected CreateIndex");
        }
    }

    #[test]
    fn test_normalize_drop_table_name() {
        let mut units = vec![make_unit(vec![
            DropTable::test(QualifiedName::unqualified("orders"))
                .with_if_exists(false)
                .into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::DropTable(dt) = &units[0].statements[0].node {
            assert_eq!(dt.name.catalog_key(), "public.orders");
        } else {
            panic!("Expected DropTable");
        }
    }

    #[test]
    fn test_normalize_unparseable_table_hint() {
        let mut units = vec![make_unit(vec![IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: Some("orders".to_string()),
        }])];

        normalize_schemas(&mut units, "public");

        if let IrNode::Unparseable { table_hint, .. } = &units[0].statements[0].node {
            assert_eq!(table_hint.as_deref(), Some("public.orders"));
        } else {
            panic!("Expected Unparseable");
        }
    }

    #[test]
    fn test_normalize_unparseable_already_qualified_hint() {
        let mut units = vec![make_unit(vec![IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: Some("myschema.orders".to_string()),
        }])];

        normalize_schemas(&mut units, "public");

        if let IrNode::Unparseable { table_hint, .. } = &units[0].statements[0].node {
            assert_eq!(
                table_hint.as_deref(),
                Some("myschema.orders"),
                "Already qualified hint should not be changed"
            );
        } else {
            panic!("Expected Unparseable");
        }
    }

    #[test]
    fn test_normalize_unparseable_no_hint() {
        let mut units = vec![make_unit(vec![IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: None,
        }])];

        normalize_schemas(&mut units, "public");

        if let IrNode::Unparseable { table_hint, .. } = &units[0].statements[0].node {
            assert!(table_hint.is_none(), "None hint should remain None");
        } else {
            panic!("Expected Unparseable");
        }
    }

    #[test]
    fn test_normalize_drop_index_untouched() {
        let mut units = vec![make_unit(vec![IrNode::DropIndex(DropIndex {
            index_name: "idx_orders_status".to_string(),
            concurrent: false,
            if_exists: false,
        })])];

        normalize_schemas(&mut units, "public");

        if let IrNode::DropIndex(di) = &units[0].statements[0].node {
            assert_eq!(di.index_name, "idx_orders_status");
        } else {
            panic!("Expected DropIndex");
        }
    }

    #[test]
    fn test_normalize_ignored_untouched() {
        let mut units = vec![make_unit(vec![IrNode::Ignored {
            raw_sql: "GRANT SELECT ON orders TO user".to_string(),
        }])];

        normalize_schemas(&mut units, "public");

        assert!(matches!(
            &units[0].statements[0].node,
            IrNode::Ignored { .. }
        ));
    }

    #[test]
    fn test_normalize_custom_default_schema() {
        let mut units = vec![make_unit(vec![
            CreateTable::test(QualifiedName::unqualified("orders")).into(),
        ])];

        normalize_schemas(&mut units, "order");

        if let IrNode::CreateTable(ct) = &units[0].statements[0].node {
            assert_eq!(ct.name.catalog_key(), "order.orders");
            assert_eq!(ct.name.schema, Some("order".to_string()));
        } else {
            panic!("Expected CreateTable");
        }
    }

    #[test]
    fn test_normalize_rename_table() {
        let mut units = vec![make_unit(vec![IrNode::RenameTable {
            name: QualifiedName::unqualified("orders"),
            new_name: "orders_v2".to_string(),
        }])];

        normalize_schemas(&mut units, "public");

        if let IrNode::RenameTable { name, .. } = &units[0].statements[0].node {
            assert_eq!(name.schema, Some("public".to_string()));
            assert_eq!(name.catalog_key(), "public.orders");
        } else {
            panic!("Expected RenameTable");
        }
    }

    #[test]
    fn test_normalize_truncate_table_name() {
        let mut units = vec![make_unit(vec![
            TruncateTable::test(QualifiedName::unqualified("orders")).into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::TruncateTable(tt) = &units[0].statements[0].node {
            assert_eq!(tt.name.catalog_key(), "public.orders");
        } else {
            panic!("Expected TruncateTable");
        }
    }

    #[test]
    fn test_normalize_rename_column() {
        let mut units = vec![make_unit(vec![IrNode::RenameColumn {
            table: QualifiedName::unqualified("orders"),
            old_name: "status".to_string(),
            new_name: "order_status".to_string(),
        }])];

        normalize_schemas(&mut units, "public");

        if let IrNode::RenameColumn { table, .. } = &units[0].statements[0].node {
            assert_eq!(table.schema, Some("public".to_string()));
            assert_eq!(table.catalog_key(), "public.orders");
        } else {
            panic!("Expected RenameColumn");
        }
    }

    // -----------------------------------------------------------------------
    // Partition support tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_normalize_partition_of_name() {
        let mut units = vec![make_unit(vec![
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_partition_of(QualifiedName::unqualified("parent"))
                .into(),
        ])];

        normalize_schemas(&mut units, "public");

        if let IrNode::CreateTable(ct) = &units[0].statements[0].node {
            let parent = ct
                .partition_of
                .as_ref()
                .expect("partition_of should be set");
            assert_eq!(parent.catalog_key(), "public.parent");
            assert_eq!(parent.schema, Some("public".to_string()));
        } else {
            panic!("Expected CreateTable");
        }
    }

    #[test]
    fn test_normalize_attach_partition_child() {
        let mut units = vec![make_unit(vec![IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("parent"),
            actions: vec![AlterTableAction::AttachPartition {
                child: QualifiedName::unqualified("child"),
            }],
        })])];

        normalize_schemas(&mut units, "public");

        if let IrNode::AlterTable(at) = &units[0].statements[0].node {
            assert_eq!(at.name.catalog_key(), "public.parent");
            match &at.actions[0] {
                AlterTableAction::AttachPartition { child } => {
                    assert_eq!(child.catalog_key(), "public.child");
                }
                other => panic!("Expected AttachPartition, got {:?}", other),
            }
        } else {
            panic!("Expected AlterTable");
        }
    }

    #[test]
    fn test_normalize_detach_partition_child() {
        let mut units = vec![make_unit(vec![IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("parent"),
            actions: vec![AlterTableAction::DetachPartition {
                child: QualifiedName::unqualified("child"),
                concurrent: false,
            }],
        })])];

        normalize_schemas(&mut units, "public");

        if let IrNode::AlterTable(at) = &units[0].statements[0].node {
            match &at.actions[0] {
                AlterTableAction::DetachPartition { child, .. } => {
                    assert_eq!(child.catalog_key(), "public.child");
                }
                other => panic!("Expected DetachPartition, got {:?}", other),
            }
        } else {
            panic!("Expected AlterTable");
        }
    }
}
