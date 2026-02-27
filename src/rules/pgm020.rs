//! PGM020 — `DISABLE TRIGGER` on existing table
//!
//! Detects `ALTER TABLE ... DISABLE TRIGGER` on tables that already exist.
//! Disabling triggers bypasses FK enforcement and business logic. If the
//! re-enable is missing or the migration fails partway, referential integrity
//! is silently lost.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TriggerDisableScope};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "DISABLE TRIGGER on existing table suppresses FK enforcement";

pub(super) const EXPLAIN: &str = "PGM020 \u{2014} DISABLE TRIGGER on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... DISABLE TRIGGER (specific name, ALL, or USER) on a\n\
         table that already exists in the database.\n\
         \n\
         Why it\u{2019}s dangerous:\n\
         Disabling triggers in a migration bypasses business logic and \u{2014}\n\
         critically \u{2014} foreign key enforcement triggers. DISABLE TRIGGER ALL\n\
         suppresses FK checks for the duration between the disable and the\n\
         corresponding re-enable. If the re-enable is missing, omitted due\n\
         to a migration failure, or placed in a separate migration that is\n\
         never run, the integrity guarantee is permanently lost. Even\n\
         intentional disables for bulk load performance are high-risk in\n\
         migration files.\n\
         \n\
         Safe alternative:\n\
         Avoid disabling triggers in migrations. If you must disable\n\
         triggers for bulk data loading, ensure the DISABLE and ENABLE\n\
         are in the same migration and wrapped in a transaction.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders DISABLE TRIGGER ALL;\n\
           INSERT INTO orders SELECT * FROM staging;\n\
         \n\
         Fix:\n\
           ALTER TABLE orders DISABLE TRIGGER ALL;\n\
           INSERT INTO orders SELECT * FROM staging;\n\
           ALTER TABLE orders ENABLE TRIGGER ALL;";

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
            if let AlterTableAction::DisableTrigger { scope } = action {
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
                    "DISABLE TRIGGER {label} on existing table '{table}' {detail}",
                    table = at.name.display_name(),
                );
                vec![rule.make_finding(message, ctx.file, &stmt.span)]
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

    /// Helper to build an ALTER TABLE ... DISABLE TRIGGER statement.
    fn disable_trigger_stmt(table: &str, scope: TriggerDisableScope) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::DisableTrigger { scope }],
        }))
    }

    fn existing_orders_ctx() -> (
        crate::catalog::Catalog,
        crate::catalog::Catalog,
        PathBuf,
        HashSet<String>,
    ) {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true);
            })
            .build();
        let after = before.clone();
        (
            before,
            after,
            PathBuf::from("migrations/002.sql"),
            HashSet::new(),
        )
    }

    #[test]
    fn test_fires_on_existing_table_all() {
        let (before, after, file, created) = existing_orders_ctx();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![disable_trigger_stmt("orders", TriggerDisableScope::All)];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm020).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fires_on_existing_table_named() {
        let (before, after, file, created) = existing_orders_ctx();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![disable_trigger_stmt(
            "orders",
            TriggerDisableScope::Named("my_trigger".to_string()),
        )];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm020).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fires_on_existing_table_user() {
        let (before, after, file, created) = existing_orders_ctx();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![disable_trigger_stmt("orders", TriggerDisableScope::User)];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm020).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_on_new_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![disable_trigger_stmt("orders", TriggerDisableScope::All)];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm020).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_table_not_in_catalog() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![disable_trigger_stmt("orders", TriggerDisableScope::All)];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm020).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
