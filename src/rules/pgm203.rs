//! PGM203 — `TRUNCATE TABLE` on existing table
//!
//! Detects `TRUNCATE TABLE` targeting a table that exists in `catalog_before`.
//! TRUNCATE removes all rows instantly but is irreversible and does not fire
//! ON DELETE triggers. Unlike DELETE, there is no WHERE clause — every row is gone.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "TRUNCATE TABLE on existing table";

pub(super) const EXPLAIN: &str = "PGM203 — TRUNCATE TABLE on existing table\n\
         \n\
         What it detects:\n\
         A TRUNCATE TABLE statement targeting a table that already exists in the\n\
         database (i.e., the table was not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         TRUNCATE removes all rows from a table instantly without scanning them.\n\
         Unlike DELETE, it does not fire ON DELETE triggers, does not log\n\
         individual row deletions, and cannot be filtered with a WHERE clause.\n\
         The operation is irreversible once committed.\n\
         \n\
         Example:\n\
           TRUNCATE TABLE audit_trail;\n\
         \n\
         Recommended approach:\n\
         1. Ensure the data is truly disposable or has been backed up.\n\
         2. Consider whether ON DELETE triggers need to fire — if so, use DELETE.\n\
         3. If truncating for a schema migration, document the intent clearly.\n\
         \n\
         This rule is MINOR severity to flag the operation for human review.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::TruncateTable(ref tt) = stmt.node {
            let table_key = tt.name.catalog_key();

            if ctx.is_existing_table(table_key) {
                findings.push(rule.make_finding(
                    format!(
                        "TRUNCATE TABLE '{}' removes all rows from an existing table. \
                         This is irreversible and does not fire ON DELETE triggers.",
                        tt.name.display_name()
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
        RuleId::Pgm203
    }

    #[test]
    fn test_truncate_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("audit_trail", |t| {
                t.column("id", "bigint", false)
                    .column("action", "text", false)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/005.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("audit_trail")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_truncate_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("audit_trail".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("audit_trail")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_truncate_cascade_existing_also_fires() {
        let before = CatalogBuilder::new()
            .table("audit_trail", |t| {
                t.column("id", "bigint", false)
                    .column("action", "text", false)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/006.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("audit_trail"))
                .with_cascade(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_truncate_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("audit_trail")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
