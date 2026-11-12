//! Schema normalization: resolves the parser's names into catalog names.
//!
//! After parsing, unqualified relation names (e.g., `orders`) lack a schema.
//! This module rebuilds every parsed IR node with each `QualifiedName`
//! resolved against a configurable default schema, so that `orders` and
//! `public.orders` resolve to the same catalog key.
//!
//! Every variant and every struct is rebuilt with all of its fields spelled
//! out, without wildcards, so a new variant or a new name-bearing field fails
//! to compile until it is handled here.

use crate::input::MigrationUnit;
use crate::names::ResolvedName;
use crate::parser::ir::*;

/// Resolve every relation name in the migration history against
/// `default_schema`.
///
/// Must be called **after** parsing and **before** catalog replay, which
/// only accepts resolved units.
pub fn resolve_units(
    units: Vec<MigrationUnit<QualifiedName>>,
    default_schema: &str,
) -> Vec<MigrationUnit> {
    units
        .into_iter()
        .map(|unit| resolve_unit(unit, default_schema))
        .collect()
}

fn resolve_unit(unit: MigrationUnit<QualifiedName>, default_schema: &str) -> MigrationUnit {
    let MigrationUnit {
        id,
        statements,
        source_file,
        source_line_offset,
        run_in_transaction,
        is_down,
    } = unit;
    MigrationUnit {
        id,
        statements: statements
            .into_iter()
            .map(|Located { node, span }| Located {
                node: resolve_node(node, default_schema),
                span,
            })
            .collect(),
        source_file,
        source_line_offset,
        run_in_transaction,
        is_down,
    }
}

/// Resolve every relation name in a single parsed IR node.
pub(crate) fn resolve_node(node: IrNode<QualifiedName>, default_schema: &str) -> IrNode {
    let resolve = |name: QualifiedName| name.resolve(default_schema);
    match node {
        IrNode::CreateTable(CreateTable {
            name,
            columns,
            constraints,
            persistence,
            if_not_exists,
            partition_by,
            partition_of,
        }) => IrNode::CreateTable(CreateTable {
            name: resolve(name),
            columns,
            constraints: constraints
                .into_iter()
                .map(|constraint| resolve_constraint(constraint, default_schema))
                .collect(),
            persistence,
            if_not_exists,
            partition_by,
            partition_of: partition_of.map(resolve),
        }),
        IrNode::AlterTable(AlterTable { name, actions }) => IrNode::AlterTable(AlterTable {
            name: resolve(name),
            actions: actions
                .into_iter()
                .map(|action| resolve_alter_table_action(action, default_schema))
                .collect(),
        }),
        IrNode::CreateIndex(CreateIndex {
            index_name,
            table_name,
            columns,
            include_columns,
            unique,
            concurrent,
            if_not_exists,
            where_clause,
            only,
            access_method,
        }) => IrNode::CreateIndex(CreateIndex {
            index_name,
            table_name: resolve(table_name),
            columns,
            include_columns,
            unique,
            concurrent,
            if_not_exists,
            where_clause,
            only,
            access_method,
        }),
        IrNode::DropIndex(DropIndex {
            index_name,
            concurrent,
            if_exists,
        }) => IrNode::DropIndex(DropIndex {
            index_name: resolve(index_name),
            concurrent,
            if_exists,
        }),
        IrNode::DropTable(DropTable {
            name,
            if_exists,
            cascade,
        }) => IrNode::DropTable(DropTable {
            name: resolve(name),
            if_exists,
            cascade,
        }),
        IrNode::DropSchema(drop_schema) => IrNode::DropSchema(drop_schema),
        IrNode::TruncateTable(TruncateTable { name, cascade }) => {
            IrNode::TruncateTable(TruncateTable {
                name: resolve(name),
                cascade,
            })
        }
        IrNode::InsertInto(InsertInto { table_name }) => IrNode::InsertInto(InsertInto {
            table_name: resolve(table_name),
        }),
        IrNode::UpdateTable(UpdateTable { table_name }) => IrNode::UpdateTable(UpdateTable {
            table_name: resolve(table_name),
        }),
        IrNode::DeleteFrom(DeleteFrom { table_name }) => IrNode::DeleteFrom(DeleteFrom {
            table_name: resolve(table_name),
        }),
        IrNode::Cluster(Cluster { table, index }) => IrNode::Cluster(Cluster {
            table: resolve(table),
            index,
        }),
        IrNode::VacuumFull(VacuumFull { table }) => IrNode::VacuumFull(VacuumFull {
            table: table.map(resolve),
        }),
        IrNode::Reindex(Reindex {
            kind,
            target,
            concurrent,
        }) => IrNode::Reindex(Reindex {
            kind,
            target: match target {
                ReindexTarget::Relation(name) => ReindexTarget::Relation(resolve(name)),
                ReindexTarget::Named(name) => ReindexTarget::Named(name),
            },
            concurrent,
        }),
        IrNode::AlterIndexAttachPartition {
            parent_index_name,
            child_index_name,
        } => IrNode::AlterIndexAttachPartition {
            parent_index_name: resolve(parent_index_name),
            child_index_name: resolve(child_index_name),
        },
        IrNode::RenameTable { name, new_name } => IrNode::RenameTable {
            name: resolve(name),
            new_name,
        },
        IrNode::RenameColumn {
            table,
            old_name,
            new_name,
        } => IrNode::RenameColumn {
            table: resolve(table),
            old_name,
            new_name,
        },
        IrNode::RenameIndex { index, new_name } => IrNode::RenameIndex {
            index: resolve(index),
            new_name,
        },
        IrNode::Ignored { raw_sql } => IrNode::Ignored { raw_sql },
        IrNode::Unparseable {
            raw_sql,
            table_hint,
        } => IrNode::Unparseable {
            raw_sql,
            table_hint: table_hint.map(resolve),
        },
    }
}

fn resolve_alter_table_action(
    action: AlterTableAction<QualifiedName>,
    default_schema: &str,
) -> AlterTableAction<ResolvedName> {
    match action {
        AlterTableAction::AddColumn(column) => AlterTableAction::AddColumn(column),
        AlterTableAction::DropColumn { name } => AlterTableAction::DropColumn { name },
        AlterTableAction::AddConstraint(constraint) => {
            AlterTableAction::AddConstraint(resolve_constraint(constraint, default_schema))
        }
        AlterTableAction::AlterColumnType {
            column_name,
            new_type,
            old_type,
        } => AlterTableAction::AlterColumnType {
            column_name,
            new_type,
            old_type,
        },
        AlterTableAction::SetNotNull { column_name } => {
            AlterTableAction::SetNotNull { column_name }
        }
        AlterTableAction::DropNotNull { column_name } => {
            AlterTableAction::DropNotNull { column_name }
        }
        AlterTableAction::SetDefault {
            column_name,
            default_expr,
        } => AlterTableAction::SetDefault {
            column_name,
            default_expr,
        },
        AlterTableAction::DropDefault { column_name } => {
            AlterTableAction::DropDefault { column_name }
        }
        AlterTableAction::DropConstraint { constraint_name } => {
            AlterTableAction::DropConstraint { constraint_name }
        }
        AlterTableAction::ValidateConstraint { constraint_name } => {
            AlterTableAction::ValidateConstraint { constraint_name }
        }
        AlterTableAction::AttachPartition { child } => AlterTableAction::AttachPartition {
            child: child.resolve(default_schema),
        },
        AlterTableAction::DetachPartition { child, concurrent } => {
            AlterTableAction::DetachPartition {
                child: child.resolve(default_schema),
                concurrent,
            }
        }
        AlterTableAction::DisableTrigger { scope } => AlterTableAction::DisableTrigger { scope },
        AlterTableAction::Other { description } => AlterTableAction::Other { description },
    }
}

fn resolve_constraint(
    constraint: TableConstraint<QualifiedName>,
    default_schema: &str,
) -> TableConstraint<ResolvedName> {
    match constraint {
        TableConstraint::PrimaryKey {
            name,
            columns,
            include_columns,
            using_index,
        } => TableConstraint::PrimaryKey {
            name,
            columns,
            include_columns,
            using_index,
        },
        TableConstraint::ForeignKey {
            name,
            columns,
            ref_table,
            ref_columns,
            not_valid,
        } => TableConstraint::ForeignKey {
            name,
            columns,
            ref_table: ref_table.resolve(default_schema),
            ref_columns,
            not_valid,
        },
        TableConstraint::Unique {
            name,
            columns,
            include_columns,
            using_index,
        } => TableConstraint::Unique {
            name,
            columns,
            include_columns,
            using_index,
        },
        TableConstraint::Check {
            name,
            expression,
            referenced_columns,
            not_valid,
        } => TableConstraint::Check {
            name,
            expression,
            referenced_columns,
            not_valid,
        },
        TableConstraint::Exclude {
            name,
            elements,
            include_columns,
        } => TableConstraint::Exclude {
            name,
            elements,
            include_columns,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::RelationName;
    use std::path::PathBuf;

    const DEFAULT_SCHEMA: &str = "public";

    fn resolved(node: impl Into<IrNode<QualifiedName>>) -> IrNode {
        resolve_node(node.into(), DEFAULT_SCHEMA)
    }

    #[track_caller]
    fn assert_resolved_into_default_schema(name: &ResolvedName, bare_name: &str) {
        assert_eq!(
            name.relation(),
            &RelationName::new(DEFAULT_SCHEMA, bare_name)
        );
        assert_eq!(name.display_name(), bare_name);
    }

    #[track_caller]
    fn assert_resolved_into_written_schema(name: &ResolvedName, schema: &str, bare_name: &str) {
        assert_eq!(name.relation(), &RelationName::new(schema, bare_name));
        assert_eq!(name.display_name(), format!("{schema}.{bare_name}"));
    }

    fn parsed_unit(id: &str, nodes: Vec<IrNode<QualifiedName>>) -> MigrationUnit<QualifiedName> {
        MigrationUnit {
            id: id.to_string(),
            statements: nodes
                .into_iter()
                .map(|node| Located {
                    node,
                    span: SourceSpan::at(1, 1),
                })
                .collect(),
            source_file: PathBuf::from(format!("{id}.sql")),
            source_line_offset: 1,
            run_in_transaction: true,
            is_down: false,
        }
    }

    fn foreign_key_to(ref_table: QualifiedName) -> TableConstraint<QualifiedName> {
        TableConstraint::ForeignKey {
            name: Some("fk_customer".to_string()),
            columns: vec!["customer_id".to_string()],
            ref_table,
            ref_columns: vec!["id".to_string()],
            not_valid: false,
        }
    }

    #[test]
    fn unqualified_name_resolves_into_default_schema_and_displays_bare() {
        let IrNode::CreateTable(ct) =
            resolved(CreateTable::test(QualifiedName::unqualified("orders")))
        else {
            panic!("expected CreateTable");
        };

        assert_resolved_into_default_schema(&ct.name, "orders");
    }

    #[test]
    fn qualified_name_keeps_its_schema_and_displays_it() {
        let IrNode::CreateTable(ct) = resolved(CreateTable::test(QualifiedName::qualified(
            "myschema", "orders",
        ))) else {
            panic!("expected CreateTable");
        };

        assert_resolved_into_written_schema(&ct.name, "myschema", "orders");
    }

    #[test]
    fn name_qualified_with_default_schema_displays_the_schema_it_was_written_with() {
        let IrNode::CreateTable(ct) = resolved(CreateTable::test(QualifiedName::qualified(
            DEFAULT_SCHEMA,
            "orders",
        ))) else {
            panic!("expected CreateTable");
        };

        assert_resolved_into_written_schema(&ct.name, DEFAULT_SCHEMA, "orders");
    }

    #[test]
    fn resolve_units_resolves_against_the_given_default_schema() {
        let units = resolve_units(
            vec![parsed_unit(
                "001",
                vec![CreateTable::test(QualifiedName::unqualified("orders")).into()],
            )],
            "order",
        );

        let IrNode::CreateTable(ct) = &units[0].statements[0].node else {
            panic!("expected CreateTable");
        };
        assert_eq!(ct.name.relation(), &RelationName::new("order", "orders"));
        assert_eq!(ct.name.display_name(), "orders");
    }

    #[test]
    fn resolve_units_resolves_every_statement_of_every_unit_in_order() {
        let units = resolve_units(
            vec![
                parsed_unit(
                    "001",
                    vec![
                        CreateTable::test(QualifiedName::unqualified("customers")).into(),
                        CreateTable::test(QualifiedName::qualified("audit", "events")).into(),
                    ],
                ),
                parsed_unit(
                    "002",
                    vec![DropTable::test(QualifiedName::unqualified("orders")).into()],
                ),
            ],
            DEFAULT_SCHEMA,
        );

        let ids: Vec<&str> = units.iter().map(|unit| unit.id.as_str()).collect();
        assert_eq!(ids, ["001", "002"]);
        let [first, second] = units[0].statements.as_slice() else {
            panic!("expected two statements in the first unit");
        };
        let IrNode::CreateTable(customers) = &first.node else {
            panic!("expected CreateTable");
        };
        assert_resolved_into_default_schema(&customers.name, "customers");
        let IrNode::CreateTable(events) = &second.node else {
            panic!("expected CreateTable");
        };
        assert_resolved_into_written_schema(&events.name, "audit", "events");
        let [only] = units[1].statements.as_slice() else {
            panic!("expected one statement in the second unit");
        };
        let IrNode::DropTable(dt) = &only.node else {
            panic!("expected DropTable");
        };
        assert_resolved_into_default_schema(&dt.name, "orders");
    }

    #[test]
    fn resolve_units_keeps_unit_metadata_and_statement_spans() {
        let span = SourceSpan {
            start_line: 3,
            end_line: 5,
            start_offset: 40,
            end_offset: 97,
        };
        let parsed = MigrationUnit {
            id: "changeset-7".to_string(),
            statements: vec![Located {
                node: DropTable::test(QualifiedName::unqualified("orders")).into(),
                span: span.clone(),
            }],
            source_file: PathBuf::from("db/changelog.xml"),
            source_line_offset: 12,
            run_in_transaction: false,
            is_down: true,
        };

        let [resolved_unit]: [MigrationUnit; 1] = resolve_units(vec![parsed], DEFAULT_SCHEMA)
            .try_into()
            .expect("one unit in, one unit out");

        assert_eq!(resolved_unit.id, "changeset-7");
        assert_eq!(resolved_unit.source_file, PathBuf::from("db/changelog.xml"));
        assert_eq!(resolved_unit.source_line_offset, 12);
        assert!(!resolved_unit.run_in_transaction);
        assert!(resolved_unit.is_down);
        assert_eq!(resolved_unit.statements.len(), 1);
        assert_eq!(resolved_unit.statements[0].span, span);
    }

    #[test]
    fn create_table_foreign_key_ref_table_is_resolved() {
        let IrNode::CreateTable(ct) = resolved(
            CreateTable::test(QualifiedName::unqualified("orders")).with_constraints(vec![
                foreign_key_to(QualifiedName::unqualified("customers")),
            ]),
        ) else {
            panic!("expected CreateTable");
        };

        let [TableConstraint::ForeignKey { ref_table, .. }] = ct.constraints.as_slice() else {
            panic!("expected one ForeignKey constraint");
        };
        assert_resolved_into_default_schema(ref_table, "customers");
    }

    #[test]
    fn create_table_partition_of_parent_is_resolved() {
        let IrNode::CreateTable(ct) = resolved(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_partition_of(QualifiedName::unqualified("parent")),
        ) else {
            panic!("expected CreateTable");
        };

        let parent = ct
            .partition_of
            .as_ref()
            .expect("partition_of should be set");
        assert_resolved_into_default_schema(parent, "parent");
        assert_resolved_into_default_schema(&ct.name, "child");
    }

    #[test]
    fn alter_table_name_is_resolved() {
        let IrNode::AlterTable(at) = resolved(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test("col", "text"))],
        }) else {
            panic!("expected AlterTable");
        };

        assert_resolved_into_default_schema(&at.name, "orders");
    }

    #[test]
    fn alter_table_add_foreign_key_ref_table_is_resolved() {
        let IrNode::AlterTable(at) = resolved(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(foreign_key_to(
                QualifiedName::unqualified("customers"),
            ))],
        }) else {
            panic!("expected AlterTable");
        };

        let [AlterTableAction::AddConstraint(TableConstraint::ForeignKey { ref_table, .. })] =
            at.actions.as_slice()
        else {
            panic!("expected one AddConstraint ForeignKey action");
        };
        assert_resolved_into_default_schema(ref_table, "customers");
    }

    #[test]
    fn alter_table_attach_partition_child_is_resolved() {
        let IrNode::AlterTable(at) = resolved(AlterTable {
            name: QualifiedName::unqualified("parent"),
            actions: vec![AlterTableAction::AttachPartition {
                child: QualifiedName::unqualified("child"),
            }],
        }) else {
            panic!("expected AlterTable");
        };

        assert_resolved_into_default_schema(&at.name, "parent");
        let [AlterTableAction::AttachPartition { child }] = at.actions.as_slice() else {
            panic!("expected one AttachPartition action");
        };
        assert_resolved_into_default_schema(child, "child");
    }

    #[test]
    fn alter_table_detach_partition_child_is_resolved() {
        let IrNode::AlterTable(at) = resolved(AlterTable {
            name: QualifiedName::unqualified("parent"),
            actions: vec![AlterTableAction::DetachPartition {
                child: QualifiedName::unqualified("child"),
                concurrent: false,
            }],
        }) else {
            panic!("expected AlterTable");
        };

        let [AlterTableAction::DetachPartition { child, .. }] = at.actions.as_slice() else {
            panic!("expected one DetachPartition action");
        };
        assert_resolved_into_default_schema(child, "child");
    }

    #[test]
    fn create_index_table_name_is_resolved_and_index_name_stays_bare() {
        let IrNode::CreateIndex(ci) = resolved(
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_columns(vec![IndexColumn::Column("status".to_string())]),
        ) else {
            panic!("expected CreateIndex");
        };

        assert_resolved_into_default_schema(&ci.table_name, "orders");
        assert_eq!(ci.index_name.as_deref(), Some("idx_orders_status"));
    }

    #[test]
    fn unqualified_drop_index_name_is_resolved_into_default_schema() {
        let IrNode::DropIndex(di) = resolved(DropIndex::test(QualifiedName::unqualified(
            "idx_orders_status",
        ))) else {
            panic!("expected DropIndex");
        };

        assert_resolved_into_default_schema(&di.index_name, "idx_orders_status");
    }

    #[test]
    fn qualified_drop_index_name_keeps_its_schema() {
        let IrNode::DropIndex(di) = resolved(DropIndex::test(QualifiedName::qualified(
            "myschema",
            "idx_orders_status",
        ))) else {
            panic!("expected DropIndex");
        };

        assert_resolved_into_written_schema(&di.index_name, "myschema", "idx_orders_status");
    }

    #[test]
    fn drop_table_name_is_resolved() {
        let IrNode::DropTable(dt) =
            resolved(DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false))
        else {
            panic!("expected DropTable");
        };

        assert_resolved_into_default_schema(&dt.name, "orders");
    }

    #[test]
    fn truncate_table_name_is_resolved() {
        let IrNode::TruncateTable(tt) =
            resolved(TruncateTable::test(QualifiedName::unqualified("orders")))
        else {
            panic!("expected TruncateTable");
        };

        assert_resolved_into_default_schema(&tt.name, "orders");
    }

    #[test]
    fn insert_into_table_name_is_resolved() {
        let IrNode::InsertInto(ii) =
            resolved(InsertInto::test(QualifiedName::unqualified("orders")))
        else {
            panic!("expected InsertInto");
        };

        assert_resolved_into_default_schema(&ii.table_name, "orders");
    }

    #[test]
    fn update_table_name_is_resolved() {
        let IrNode::UpdateTable(ut) =
            resolved(UpdateTable::test(QualifiedName::unqualified("orders")))
        else {
            panic!("expected UpdateTable");
        };

        assert_resolved_into_default_schema(&ut.table_name, "orders");
    }

    #[test]
    fn delete_from_table_name_is_resolved() {
        let IrNode::DeleteFrom(df) =
            resolved(DeleteFrom::test(QualifiedName::unqualified("orders")))
        else {
            panic!("expected DeleteFrom");
        };

        assert_resolved_into_default_schema(&df.table_name, "orders");
    }

    #[test]
    fn cluster_table_is_resolved_and_index_stays_bare() {
        let IrNode::Cluster(c) = resolved(
            Cluster::test(QualifiedName::unqualified("orders")).with_index("idx_orders_status"),
        ) else {
            panic!("expected Cluster");
        };

        assert_resolved_into_default_schema(&c.table, "orders");
        assert_eq!(c.index.as_deref(), Some("idx_orders_status"));
    }

    #[test]
    fn vacuum_full_table_is_resolved() {
        let IrNode::VacuumFull(v) =
            resolved(VacuumFull::test(QualifiedName::unqualified("orders")))
        else {
            panic!("expected VacuumFull");
        };

        let table = v.table.as_ref().expect("table should be set");
        assert_resolved_into_default_schema(table, "orders");
    }

    #[test]
    fn vacuum_full_of_every_table_stays_without_a_table() {
        let IrNode::VacuumFull(v) = resolved(VacuumFull::test_all()) else {
            panic!("expected VacuumFull");
        };

        assert!(v.table.is_none());
    }

    #[test]
    fn reindex_relation_target_is_resolved() {
        let IrNode::Reindex(r) = resolved(Reindex::test_index(QualifiedName::unqualified(
            "idx_orders_status",
        ))) else {
            panic!("expected Reindex");
        };

        let ReindexTarget::Relation(index) = &r.target else {
            panic!("expected a relation target");
        };
        assert_resolved_into_default_schema(index, "idx_orders_status");
    }

    #[test]
    fn reindex_schema_target_stays_a_bare_schema_name() {
        let IrNode::Reindex(r) = resolved(Reindex::test_schema("audit")) else {
            panic!("expected Reindex");
        };

        assert_eq!(r.target, ReindexTarget::Named("audit".to_string()));
    }

    #[test]
    fn unqualified_alter_index_attach_partition_names_are_resolved_into_default_schema() {
        let IrNode::AlterIndexAttachPartition {
            parent_index_name,
            child_index_name,
        } = resolved(IrNode::AlterIndexAttachPartition {
            parent_index_name: QualifiedName::unqualified("idx_parent"),
            child_index_name: QualifiedName::unqualified("idx_child"),
        })
        else {
            panic!("expected AlterIndexAttachPartition");
        };

        assert_resolved_into_default_schema(&parent_index_name, "idx_parent");
        assert_resolved_into_default_schema(&child_index_name, "idx_child");
    }

    #[test]
    fn qualified_alter_index_attach_partition_names_keep_their_schemas() {
        let IrNode::AlterIndexAttachPartition {
            parent_index_name,
            child_index_name,
        } = resolved(IrNode::AlterIndexAttachPartition {
            parent_index_name: QualifiedName::qualified("myschema", "idx_parent"),
            child_index_name: QualifiedName::qualified("archive", "idx_child"),
        })
        else {
            panic!("expected AlterIndexAttachPartition");
        };

        assert_resolved_into_written_schema(&parent_index_name, "myschema", "idx_parent");
        assert_resolved_into_written_schema(&child_index_name, "archive", "idx_child");
    }

    #[test]
    fn rename_table_name_is_resolved_and_new_name_stays_bare() {
        let IrNode::RenameTable { name, new_name } = resolved(IrNode::RenameTable {
            name: QualifiedName::unqualified("orders"),
            new_name: "orders_v2".to_string(),
        }) else {
            panic!("expected RenameTable");
        };

        assert_resolved_into_default_schema(&name, "orders");
        assert_eq!(new_name, "orders_v2");
    }

    #[test]
    fn rename_column_table_is_resolved() {
        let IrNode::RenameColumn {
            table,
            old_name,
            new_name,
        } = resolved(IrNode::RenameColumn {
            table: QualifiedName::unqualified("orders"),
            old_name: "status".to_string(),
            new_name: "order_status".to_string(),
        })
        else {
            panic!("expected RenameColumn");
        };

        assert_resolved_into_default_schema(&table, "orders");
        assert_eq!(old_name, "status");
        assert_eq!(new_name, "order_status");
    }

    #[test]
    fn rename_index_name_is_resolved_and_new_name_stays_bare() {
        let IrNode::RenameIndex { index, new_name } = resolved(IrNode::RenameIndex {
            index: QualifiedName::unqualified("idx_orders_status"),
            new_name: "idx_orders_state".to_string(),
        }) else {
            panic!("expected RenameIndex");
        };

        assert_resolved_into_default_schema(&index, "idx_orders_status");
        assert_eq!(new_name, "idx_orders_state");
    }

    #[test]
    fn unqualified_unparseable_table_hint_is_resolved_into_default_schema() {
        let IrNode::Unparseable { table_hint, .. } = resolved(IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: Some(QualifiedName::unqualified("orders")),
        }) else {
            panic!("expected Unparseable");
        };

        let hint = table_hint.expect("table_hint should be set");
        assert_resolved_into_default_schema(&hint, "orders");
    }

    #[test]
    fn qualified_unparseable_table_hint_keeps_its_schema() {
        let IrNode::Unparseable { table_hint, .. } = resolved(IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: Some(QualifiedName::qualified("myschema", "orders")),
        }) else {
            panic!("expected Unparseable");
        };

        let hint = table_hint.expect("table_hint should be set");
        assert_resolved_into_written_schema(&hint, "myschema", "orders");
    }

    #[test]
    fn unparseable_without_table_hint_stays_without_one() {
        let IrNode::Unparseable { table_hint, .. } = resolved(IrNode::Unparseable {
            raw_sql: "DO $$ ... $$".to_string(),
            table_hint: None,
        }) else {
            panic!("expected Unparseable");
        };

        assert!(table_hint.is_none());
    }

    #[test]
    fn ignored_statement_passes_through_unchanged() {
        let raw_sql = "GRANT SELECT ON orders TO user".to_string();

        assert_eq!(
            resolved(IrNode::Ignored {
                raw_sql: raw_sql.clone()
            }),
            IrNode::Ignored { raw_sql }
        );
    }

    #[test]
    fn drop_schema_passes_through_unchanged() {
        let drop_schema = DropSchema::test("audit")
            .with_if_exists(true)
            .with_cascade(true);

        assert_eq!(
            resolved(drop_schema.clone()),
            IrNode::DropSchema(drop_schema)
        );
    }
}
