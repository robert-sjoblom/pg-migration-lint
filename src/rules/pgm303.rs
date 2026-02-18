//! PGM303 — `DELETE FROM` existing table in migration
//!
//! Detects `DELETE FROM` statements targeting tables that already exist in
//! the database. Unbatched deletes hold row locks and generate significant
//! WAL volume, which can spike replication lag.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "DELETE FROM existing table in migration";

pub(super) const EXPLAIN: &str = "PGM303 — DELETE FROM existing table in migration\n\
         \n\
         What it detects:\n\
         A DELETE FROM statement targeting a table that already exists in the\n\
         database (i.e., not created in the same set of changed files).\n\
         \n\
         Why it matters:\n\
         DELETE statements in migrations remove existing data. On large tables\n\
         this can be problematic:\n\
         - Row locks are held for the full statement duration.\n\
         - Each deleted row generates WAL, which can spike replication lag.\n\
         - ON DELETE triggers fire for every row, adding overhead.\n\
         - Long-running deletes may time out under migration tool limits.\n\
         - Deleted rows become dead tuples until autovacuum runs.\n\
         \n\
         Example (flagged):\n\
           DELETE FROM audit_log WHERE created_at < '2020-01-01';\n\
         \n\
         Recommended approach:\n\
         1. Verify the row count is bounded.\n\
         2. For large deletes, batch in chunks (e.g., 10k rows per iteration).\n\
         3. If no triggers need to fire, consider TRUNCATE instead.\n\
         \n\
         Not flagged:\n\
         - DELETE from a table created in the same migration file.\n\
         \n\
         This rule is MINOR severity.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::DeleteFrom(ref df) = stmt.node {
            let table_key = df.table_name.catalog_key();

            if ctx.is_existing_table(table_key) {
                findings.push(rule.make_finding(
                    format!(
                        "DELETE FROM existing table '{}' in a migration. Unbatched deletes \
                         hold row locks and generate significant WAL. Verify row volume \
                         and consider batched execution.",
                        df.table_name.display_name()
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
    use crate::rules::{DmlRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn rule_id() -> RuleId {
        RuleId::Dml(DmlRule::Pgm303)
    }

    #[test]
    fn test_delete_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("audit_log", |t| {
                t.column("id", "bigint", false)
                    .column("created_at", "timestamptz", false)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/005.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DeleteFrom::test(QualifiedName::unqualified("audit_log")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_delete_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("audit_log", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("audit_log".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DeleteFrom::test(QualifiedName::unqualified("audit_log")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_delete_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DeleteFrom::test(QualifiedName::unqualified("audit_log")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
