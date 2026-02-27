//! PGM302 — `UPDATE` on existing table in migration
//!
//! Detects `UPDATE` statements targeting tables that already exist in the
//! database. Unbatched updates hold row locks for the full statement
//! duration and can cause significant contention on busy tables.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "UPDATE on existing table in migration";

pub(super) const EXPLAIN: &str = "PGM302 — UPDATE on existing table in migration\n\
         \n\
         What it detects:\n\
         An UPDATE statement targeting a table that already exists in the\n\
         database (i.e., not created in the same set of changed files).\n\
         \n\
         Why it matters:\n\
         UPDATE statements in migrations typically backfill or transform\n\
         existing data. On large tables this can be problematic:\n\
         - Row locks are held for the full statement duration.\n\
         - The entire UPDATE generates WAL, which can spike replication lag.\n\
         - Long-running updates may time out under migration tool limits.\n\
         - They block autovacuum from processing dead tuples on the table.\n\
         \n\
         Example (flagged):\n\
           UPDATE orders SET status = 'pending' WHERE status IS NULL;\n\
         \n\
         Recommended approach:\n\
         1. Verify the row count is bounded (small lookup table = fine).\n\
         2. For large tables, batch the update in chunks.\n\
         3. Consider running the update outside the migration transaction.\n\
         \n\
         Not flagged:\n\
         - UPDATE on a table created in the same migration file.\n\
         \n\
         This rule is MINOR severity.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::UpdateTable(ref ut) = stmt.node {
            let table_key = ut.table_name.catalog_key();

            if ctx.is_existing_table(table_key) {
                findings.push(rule.make_finding(
                    format!(
                        "UPDATE on existing table '{}' in a migration. Unbatched updates \
                         hold row locks for the full statement duration. Verify row volume \
                         and consider batched execution.",
                        ut.table_name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
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
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn rule_id() -> RuleId {
        RuleId::Pgm302
    }

    #[test]
    fn test_update_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/005.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            UpdateTable::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_update_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            UpdateTable::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_update_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            UpdateTable::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
