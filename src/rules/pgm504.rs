#![doc = include_str!("docs/pgm504.md")]

use std::collections::HashSet;

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "RENAME TABLE on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm504.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    // Pass 1: collect all CREATE TABLE catalog keys in this unit.
    // These represent "replacement" tables that re-create a renamed-away name.
    let created_in_unit: HashSet<&str> = statements
        .iter()
        .filter_map(|stmt| match &stmt.node {
            IrNode::CreateTable(ct) => Some(ct.name.catalog_key()),
            _ => None,
        })
        .collect();

    // Pass 2: find RenameTable on existing tables.
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::RenameTable {
            ref name,
            ref new_name,
        } = stmt.node
        {
            let table_key = name.catalog_key();

            // Only flag if the table pre-exists (not created in the current changeset).
            if !ctx.is_existing_table(table_key) {
                continue;
            }

            // Replacement detection: if a CREATE TABLE in this unit re-creates
            // the old name, the rename is part of a safe swap pattern.
            if created_in_unit.contains(table_key) {
                continue;
            }

            findings.push(rule.make_finding(
                format!(
                    "Renaming existing table '{}' to '{}' will break all \
                         queries, views, and functions referencing the old name.",
                    name.display_name(),
                    new_name,
                ),
                ctx.file,
                &stmt.span,
            ));
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    #[test]
    fn test_rename_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::RenameTable {
            name: QualifiedName::unqualified("orders"),
            new_name: "orders_archive".to_string(),
        })];

        let findings = RuleId::Pgm504.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_rename_new_table_no_finding() {
        // Table does not exist in catalog_before, so the rename is harmless.
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::RenameTable {
            name: QualifiedName::unqualified("temp_table"),
            new_name: "real_table".to_string(),
        })];

        let findings = RuleId::Pgm504.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_rename_with_replacement_table_no_finding() {
        // Pattern: rename orders -> orders_old, then CREATE TABLE orders.
        // The old name is re-created, so existing queries still work.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located(IrNode::RenameTable {
                name: QualifiedName::unqualified("orders"),
                new_name: "orders_old".to_string(),
            }),
            located(IrNode::CreateTable(
                CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                    ColumnDef::test("id", "integer")
                        .with_nullable(false)
                        .with_inline_pk(),
                ]),
            )),
        ];

        let findings = RuleId::Pgm504.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Expected no findings for replacement pattern, got: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>(),
        );
    }
}
