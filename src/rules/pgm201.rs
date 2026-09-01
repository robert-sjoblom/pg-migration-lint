//! PGM201 — `DROP TABLE` on existing table
//!
//! Detects `DROP TABLE` targeting a table that exists in `catalog_before`.
//! The data is permanently lost and anything referencing the table breaks.
//! For a partition, the drop also takes `ACCESS EXCLUSIVE` on the parent
//! partitioned table and holds it until commit, blocking the whole hierarchy.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, existing_table_check};

pub(super) const DESCRIPTION: &str = "DROP TABLE on existing table";

pub(super) const EXPLAIN: &str = "PGM201 — DROP TABLE on existing table\n\
         \n\
         What it detects:\n\
         A DROP TABLE statement targeting a table that already exists in the\n\
         database (i.e., the table was not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         Dropping a table is intentional but destructive and irreversible in\n\
         production. All data in the table is permanently lost, and any\n\
         queries, views, foreign keys, or application code referencing the\n\
         table will break.\n\
         \n\
         The drop can ALSO be a downtime risk, and the case to look for is a\n\
         table that is part of a partitioned hierarchy.\n\
         \n\
         Table outside any partitioned hierarchy is NOT a downtime risk:\n\
         The DDL itself is instant. PostgreSQL does not scan the table or\n\
         hold an extended lock, so the risk is data loss, not downtime.\n\
         \n\
         Table inside a partitioned hierarchy IS a downtime risk:\n\
         DROP TABLE child takes ACCESS EXCLUSIVE on the PARENT partitioned\n\
         table, not just on the child, and holds it until the transaction\n\
         commits.\n\
         - A reader or writer holding the parent does not make the drop fail\n\
         outright: the drop waits for ACCESS EXCLUSIVE on the parent. With a\n\
         migration-grade lock_timeout that wait ends as SQLSTATE 55P03 and\n\
         the transaction rolls back; without one it waits for as long as the\n\
         traffic holds the parent.\n\
         - While the drop is queued, its pending ACCESS EXCLUSIVE already blocks\n\
         new readers that the current lock holder would have let through, so an\n\
         unbounded wait stalls the whole parent table.\n\
         - One transaction holds ACCESS EXCLUSIVE on every parent it has\n\
         touched until COMMIT, so a single lock it cannot get discards all\n\
         the work the migration had already done.\n\
         - Traffic aimed straight at a sibling partition does NOT block the\n\
         drop. It is traffic routed through the parent that does, so an\n\
         idle-looking partition is no evidence that the drop is safe.\n\
         \n\
         Example:\n\
           DROP TABLE orders;\n\
         \n\
         Recommended approach:\n\
         1. Ensure no application code, views, or foreign keys reference the table.\n\
         2. Consider renaming the table first and waiting before dropping.\n\
         3. Take a backup of the table data if it may be needed later.\n\
         4. If the table is a partition, DETACH PARTITION ... CONCURRENTLY\n\
         first, then DROP the now-standalone table. See PGM004.\n\
         \n\
         Note: DETACH PARTITION ... CONCURRENTLY cannot run inside a\n\
         transaction block, and it still waits for traffic on the parent — it\n\
         can fail with SQLSTATE 55P03 too. An interrupted detach leaves the\n\
         partition pending detach, where DROP TABLE still takes ACCESS\n\
         EXCLUSIVE on the parent.\n\
         \n\
         This rule is MINOR severity to flag the operation for human review.";

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    existing_table_check::check_existing_table(statements, ctx, rule, |node| {
        if let IrNode::DropTable(dt) = node {
            Some((
                &dt.name,
                format!(
                    "DROP TABLE '{}' removes an existing table. \
                     This is irreversible and all data will be lost.",
                    dt.name.display_name()
                ),
            ))
        } else {
            None
        }
    })
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
    fn test_drop_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm201.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm201.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm201.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
