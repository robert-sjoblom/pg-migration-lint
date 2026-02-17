//! PGM023 — Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`
//!
//! Detects `CREATE TABLE` or `CREATE INDEX` without the `IF NOT EXISTS` clause.
//! Without `IF NOT EXISTS`, the statement fails if the object already exists.
//! In migration pipelines that may be re-run, this causes hard failures.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX";

pub(super) const EXPLAIN: &str = "PGM023 — Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX\n\
         \n\
         What it detects:\n\
         A CREATE TABLE or CREATE INDEX statement that does not include the\n\
         IF NOT EXISTS clause.\n\
         \n\
         Why it matters:\n\
         Without IF NOT EXISTS, the statement fails if the object already exists.\n\
         In migration pipelines that may be re-run (e.g., idempotent migrations,\n\
         manual re-execution after partial failure), this causes hard failures.\n\
         Adding IF NOT EXISTS makes the statement idempotent.\n\
         \n\
         Example:\n\
           -- Fails if 'orders' already exists:\n\
           CREATE TABLE orders (id bigint PRIMARY KEY);\n\
           CREATE INDEX idx_orders_status ON orders (status);\n\
         \n\
         Recommended fix:\n\
           CREATE TABLE IF NOT EXISTS orders (id bigint PRIMARY KEY);\n\
           CREATE INDEX IF NOT EXISTS idx_orders_status ON orders (status);";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) if !ct.if_not_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "CREATE TABLE '{}': add IF NOT EXISTS for idempotent migrations.",
                        ct.name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
            IrNode::CreateIndex(ci) if !ci.if_not_exists => {
                let index_name = ci.index_name.as_deref().unwrap_or("<unnamed>");
                findings.push(rule.make_finding(
                    format!(
                        "CREATE INDEX '{}': add IF NOT EXISTS for idempotent migrations.",
                        index_name
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
            _ => {}
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{MigrationRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_create_table_without_if_not_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable::test(
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Migration(MigrationRule::Pgm023).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_create_table_with_if_not_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_if_not_exists(true),
        ))];

        let findings = RuleId::Migration(MigrationRule::Pgm023).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_create_index_without_if_not_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex::test(
            Some("idx_orders_status".to_string()),
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Migration(MigrationRule::Pgm023).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn create_index_unnamed_without_if_not_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex::test(
            None,
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Migration(MigrationRule::Pgm023).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_create_index_with_if_not_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_if_not_exists(true),
        ))];

        let findings = RuleId::Migration(MigrationRule::Pgm023).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
