#![doc = include_str!("docs/pgm020.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located, TriggerDisableScope};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "DISABLE TRIGGER on table suppresses FK enforcement";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm020.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for stmt in statements {
        let IrNode::AlterTable(ref at) = stmt.node else {
            continue;
        };
        let table_key = at.name.catalog_key();

        // Existing table → default severity (Minor).
        // New table or unknown table → Info.
        let severity = if ctx.is_existing_table(table_key) {
            rule.default_severity()
        } else {
            Severity::Info
        };

        for action in &at.actions {
            let AlterTableAction::DisableTrigger { scope } = action else {
                continue;
            };
            let (label, detail) = match scope {
                TriggerDisableScope::Named(name) => (
                    format!("'{name}'"),
                    "suppresses the named trigger. If this trigger enforces \
                     business logic and is not re-enabled in the same migration, \
                     those guarantees are lost.",
                ),
                TriggerDisableScope::All => (
                    "ALL".to_string(),
                    "suppresses all triggers including foreign key enforcement. \
                     If this is not re-enabled in the same migration, \
                     referential integrity guarantees are lost.",
                ),
                TriggerDisableScope::User => (
                    "USER".to_string(),
                    "suppresses user-defined triggers (FK enforcement triggers \
                     are not affected). If this is not re-enabled in the same \
                     migration, business logic guarantees are lost.",
                ),
            };
            let message = format!(
                "DISABLE TRIGGER {label} on table '{table}' {detail}",
                table = at.name.display_name(),
            );
            findings.push(Finding::new(
                rule.id(),
                severity,
                message,
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
    use rstest::rstest;

    /// Helper to build an ALTER TABLE ... DISABLE TRIGGER statement.
    fn disable_trigger_stmt(table: &str, scope: TriggerDisableScope) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::DisableTrigger { scope }],
        }))
    }

    fn existing_orders_catalogs() -> (crate::catalog::Catalog, crate::catalog::Catalog) {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true);
            })
            .build();
        let after = before.clone();
        (before, after)
    }

    #[rstest]
    #[case::all("all", TriggerDisableScope::All)]
    #[case::named("named", TriggerDisableScope::Named("my_trigger".to_string()))]
    #[case::user("user", TriggerDisableScope::User)]
    fn fires_on_existing_table(#[case] name: &str, #[case] scope: TriggerDisableScope) {
        let (before, after) = existing_orders_catalogs();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![disable_trigger_stmt("orders", scope)];

        let findings = RuleId::Pgm020.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(format!("fires_on_existing_table_{name}"), findings);
    }

    #[test]
    fn test_fires_at_info_on_new_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![disable_trigger_stmt("orders", TriggerDisableScope::All)];

        let findings = RuleId::Pgm020.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fires_at_info_on_unknown_table() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![disable_trigger_stmt("orders", TriggerDisableScope::All)];

        let findings = RuleId::Pgm020.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
