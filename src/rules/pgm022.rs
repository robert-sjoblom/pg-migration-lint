//! PGM022 — `DROP TABLE` on existing table
//!
//! Detects `DROP TABLE` targeting a table that exists in `catalog_before`.
//! Dropping a table is intentional but destructive and irreversible in
//! production. The DDL itself is instant (no table scan, no extended lock),
//! so this is not a downtime risk — it is a data loss risk.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "DROP TABLE on existing table";

pub(super) const EXPLAIN: &str = "PGM022 — DROP TABLE on existing table\n\
         \n\
         What it detects:\n\
         A DROP TABLE statement targeting a table that already exists in the\n\
         database (i.e., the table was not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         Dropping a table is intentional but destructive and irreversible in\n\
         production. The DDL itself is instant — PostgreSQL does not scan the\n\
         table or hold an extended lock — so this is not a downtime risk.\n\
         However, all data in the table is permanently lost, and any queries,\n\
         views, foreign keys, or application code referencing the table will\n\
         break.\n\
         \n\
         Example:\n\
           DROP TABLE orders;\n\
         \n\
         Recommended approach:\n\
         1. Ensure no application code, views, or foreign keys reference the table.\n\
         2. Consider renaming the table first and waiting before dropping.\n\
         3. Take a backup of the table data if it may be needed later.\n\
         \n\
         This rule is MINOR severity to flag the operation for human review.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::DropTable(ref dt) = stmt.node {
            let table_key = dt.name.catalog_key();

            if ctx.is_existing_table(table_key) {
                findings.push(rule.make_finding(
                    format!(
                        "DROP TABLE '{}' removes an existing table. \
                             This is irreversible and all data will be lost.",
                        dt.name.display_name()
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
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{MigrationRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_drop_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(DropTable {
            name: QualifiedName::unqualified("orders"),
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm022).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(DropTable {
            name: QualifiedName::unqualified("orders"),
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm022).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(DropTable {
            name: QualifiedName::unqualified("orders"),
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm022).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
