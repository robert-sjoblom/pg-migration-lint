//! PGM008 — Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`
//!
//! Detects `DROP TABLE` or `DROP INDEX` without the `IF EXISTS` clause.
//! Without `IF EXISTS`, the statement fails if the object does not exist.
//! In migration pipelines that may be re-run, this causes hard failures.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Missing IF EXISTS on DROP TABLE / DROP INDEX";

pub(super) const EXPLAIN: &str = "PGM008 — Missing IF EXISTS on DROP TABLE / DROP INDEX\n\
         \n\
         What it detects:\n\
         A DROP TABLE or DROP INDEX statement that does not include the\n\
         IF EXISTS clause.\n\
         \n\
         Why it matters:\n\
         Without IF EXISTS, the statement fails if the object does not exist.\n\
         In migration pipelines that may be re-run (e.g., idempotent migrations,\n\
         manual re-execution after partial failure), this causes hard failures.\n\
         Adding IF EXISTS makes the statement idempotent.\n\
         \n\
         Example:\n\
           -- Fails if 'orders' does not exist:\n\
           DROP TABLE orders;\n\
           DROP INDEX idx_orders_status;\n\
         \n\
         Recommended fix:\n\
           DROP TABLE IF EXISTS orders;\n\
           DROP INDEX IF EXISTS idx_orders_status;";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    _ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::DropTable(dt) if !dt.if_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "DROP TABLE '{}': add IF EXISTS for idempotent migrations.",
                        dt.name.display_name()
                    ),
                    _ctx.file,
                    &stmt.span,
                ));
            }
            IrNode::DropIndex(di) if !di.if_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "DROP INDEX '{}': add IF EXISTS for idempotent migrations.",
                        di.index_name
                    ),
                    _ctx.file,
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
    fn test_drop_table_without_if_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(DropTable {
            name: QualifiedName::unqualified("orders"),
            if_exists: false,
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm008).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_table_with_if_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(DropTable {
            name: QualifiedName::unqualified("orders"),
            if_exists: true,
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm008).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_index_without_if_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropIndex(DropIndex {
            index_name: "idx_orders_status".to_string(),
            concurrent: false,
            if_exists: false,
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm008).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_index_with_if_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropIndex(DropIndex {
            index_name: "idx_orders_status".to_string(),
            concurrent: false,
            if_exists: true,
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm008).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
