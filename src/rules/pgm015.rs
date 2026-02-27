//! PGM015 — ADD CHECK on existing table without NOT VALID
//!
//! Detects `ALTER TABLE ... ADD CONSTRAINT ... CHECK ... ` without `NOT VALID`
//! on tables that already exist. Adding a CHECK constraint without NOT VALID
//! requires scanning the entire table while holding a SHARE ROW EXCLUSIVE lock,
//! which blocks concurrent data modifications (INSERT, UPDATE, DELETE) but
//! allows reads.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD CHECK on existing table without NOT VALID";

pub(super) const EXPLAIN: &str = "PGM015 — ADD CHECK on existing table without NOT VALID\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD CONSTRAINT ... CHECK (...) on a table that already\n\
         exists, without the NOT VALID modifier.\n\
         \n\
         Why it's dangerous:\n\
         Adding a CHECK constraint without NOT VALID acquires a SHARE ROW\n\
         EXCLUSIVE lock and scans the entire table to verify all existing rows\n\
         satisfy the constraint. This blocks concurrent data modifications\n\
         (INSERT, UPDATE, DELETE) for the duration. On large tables this can\n\
         cause significant disruption.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD CONSTRAINT orders_status_check\n\
             CHECK (status IN ('pending', 'shipped', 'delivered'));\n\
         \n\
         Fix (safe two-step pattern):\n\
           -- Step 1: Add with NOT VALID (instant, no scan)\n\
           ALTER TABLE orders ADD CONSTRAINT orders_status_check\n\
             CHECK (status IN ('pending', 'shipped', 'delivered')) NOT VALID;\n\
           -- Step 2: Validate (SHARE UPDATE EXCLUSIVE lock, concurrent reads OK)\n\
           ALTER TABLE orders VALIDATE CONSTRAINT orders_status_check;";

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
            if let AlterTableAction::AddConstraint(TableConstraint::Check {
                not_valid: false,
                ..
            }) = action
            {
                vec![rule.make_finding(
                    format!(
                        "Adding CHECK constraint on existing table '{table}' without \
                         NOT VALID will scan the entire table while holding a SHARE \
                         ROW EXCLUSIVE lock, blocking concurrent writes. Use ADD \
                         CONSTRAINT ... NOT VALID, then VALIDATE CONSTRAINT in a \
                         separate statement.",
                        table = at.name.display_name(),
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

    #[test]
    fn test_check_without_not_valid_on_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm015).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_check_with_not_valid_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: true,
            })],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm015).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_check_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm015).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_check_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm015).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
