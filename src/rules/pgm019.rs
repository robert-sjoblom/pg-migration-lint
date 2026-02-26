//! PGM019 — ADD EXCLUDE constraint on existing table
//!
//! Detects adding EXCLUDE constraints to tables that already exist.
//! Unlike CHECK and FK constraints, PostgreSQL does not support NOT VALID
//! for EXCLUDE constraints — there is no safe online path.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD EXCLUDE constraint on existing table";

pub(super) const EXPLAIN: &str = "PGM019 — ADD EXCLUDE constraint on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE (...) where the table\n\
         already exists.\n\
         \n\
         Why it's dangerous:\n\
         Adding an EXCLUDE constraint acquires an ACCESS EXCLUSIVE lock\n\
         (blocking all reads and writes) and scans all existing rows to\n\
         verify the exclusion condition. Unlike CHECK and FOREIGN KEY\n\
         constraints, PostgreSQL does not support NOT VALID for EXCLUDE\n\
         constraints. There is also no equivalent to ADD CONSTRAINT ...\n\
         USING INDEX for exclusion constraints. There is currently no\n\
         online path to add an exclusion constraint to a large existing\n\
         table without an ACCESS EXCLUSIVE lock for the duration of the scan.\n\
         \n\
         Safe alternative:\n\
         Schedule the migration during a maintenance window when\n\
         downtime is acceptable.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE reservations\n\
             ADD CONSTRAINT excl_overlap\n\
             EXCLUDE USING gist (room WITH =, period WITH &&);\n\
         \n\
         Fix:\n\
           There is no online alternative. Plan for a maintenance window.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::ExcludeCreatedInChange,
        |at, action, stmt, ctx| {
            if matches!(
                action,
                AlterTableAction::AddConstraint(TableConstraint::Exclude { .. })
            ) {
                vec![rule.make_finding(
                    format!(
                        "Adding EXCLUDE constraint on existing table '{}' acquires \
                         ACCESS EXCLUSIVE lock and scans all rows. There is no online \
                         alternative \u{2014} consider scheduling this during a maintenance \
                         window.",
                        at.name.display_name(),
                    ),
                    ctx.file,
                    &stmt.span,
                )]
            } else {
                vec![]
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{RuleId, UnsafeDdlRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Helper to build an ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE statement.
    fn add_exclude_stmt(table: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Exclude {
                name: Some("excl_orders".to_string()),
            })],
        }))
    }

    #[test]
    fn test_fires_on_existing_table() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_exclude_stmt("orders")];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm019).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_on_new_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_exclude_stmt("orders")];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm019).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_table_not_in_catalog() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_exclude_stmt("orders")];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm019).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_exclude_inside_create_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        // EXCLUDE inside a CreateTable, not an AlterTable
        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![
                    ColumnDef::test("id", "bigint").with_nullable(false),
                    ColumnDef::test("order_range", "tsrange"),
                ])
                .with_constraints(vec![TableConstraint::Exclude {
                    name: Some("excl_orders".to_string()),
                }]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm019).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fires_with_schema_qualified_name() {
        let before = CatalogBuilder::new()
            .table("myschema.orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::qualified("myschema", "orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Exclude {
                name: Some("excl_orders".to_string()),
            })],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm019).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("myschema.orders"),
            "message should include schema-qualified name, got: {}",
            findings[0].message,
        );
    }

    #[test]
    fn test_fires_in_multi_action_alter_table() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        // ALTER TABLE with multiple actions: ADD COLUMN + ADD EXCLUDE
        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![
                AlterTableAction::AddColumn(ColumnDef::test("extra", "text")),
                AlterTableAction::AddConstraint(TableConstraint::Exclude {
                    name: Some("excl_orders".to_string()),
                }),
            ],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm019).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id.as_str(), "PGM019");
    }
}
