#![doc = include_str!("docs/pgm025.md")]

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{IndexColumn, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, drop_column_check};

pub(super) const DESCRIPTION: &str = "DROP COLUMN silently removes EXCLUDE constraint";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm025.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

/// Render EXCLUDE elements for the finding message: plain columns by name,
/// expressions by their deparsed SQL text.
fn format_elements(elements: &[IndexColumn]) -> String {
    elements
        .iter()
        .map(|e| match e {
            IndexColumn::Column(name) => name.clone(),
            IndexColumn::Expression { text, .. } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    drop_column_check::check_drop_column_constraints(
        statements,
        ctx,
        |name, at, table, stmt, ctx| {
            let mut findings = Vec::new();

            // Check Exclude constraints that include this column.
            for constraint in table.constraints_involving_column(name) {
                if let ConstraintState::Exclude {
                    name: constraint_name,
                    elements,
                } = constraint
                {
                    let constraint_description = match constraint_name {
                        Some(n) => format!("'{n}'"),
                        None => format!("EXCLUDE({})", format_elements(elements)),
                    };
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' silently \
                             removes EXCLUDE constraint {constraint}. Verify that the \
                             exclusion guarantee is no longer needed.",
                            col = name,
                            table = at.name.display_name(),
                            constraint = constraint_description,
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                }
            }

            findings
        },
    )
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
    fn test_drop_exclude_column_fires() {
        let before = CatalogBuilder::new()
            .table("bookings", |t| {
                t.column("room", "integer", false)
                    .column("during", "tsrange", false)
                    .exclude_constraint(Some("excl_bookings_room_during"), &["room", "during"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("bookings"),
            actions: vec![AlterTableAction::DropColumn {
                name: "room".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm025.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_unnamed_exclude_shows_element_list() {
        let before = CatalogBuilder::new()
            .table("bookings", |t| {
                t.column("room", "integer", false)
                    .column("during", "tsrange", false)
                    .exclude_constraint(None, &["room", "during"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("bookings"),
            actions: vec![AlterTableAction::DropColumn {
                name: "during".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm025.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_non_exclude_column_no_finding() {
        let before = CatalogBuilder::new()
            .table("bookings", |t| {
                t.column("room", "integer", false)
                    .column("during", "tsrange", false)
                    .column("notes", "text", true)
                    .exclude_constraint(Some("excl_bookings_room_during"), &["room", "during"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("bookings"),
            actions: vec![AlterTableAction::DropColumn {
                name: "notes".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm025.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_before_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("nonexistent"),
            actions: vec![AlterTableAction::DropColumn {
                name: "col".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm025.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_column_after_exclude_already_dropped_no_finding() {
        // EXCLUDE constraint was explicitly removed via DROP CONSTRAINT in an
        // earlier migration. catalog_before reflects no EXCLUDE constraint, so
        // DROP COLUMN should NOT warn about silently removing it.
        let before = CatalogBuilder::new()
            .table("bookings", |t| {
                t.column("room", "integer", false)
                    .column("during", "tsrange", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("bookings"),
            actions: vec![AlterTableAction::DropColumn {
                name: "room".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm025.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
