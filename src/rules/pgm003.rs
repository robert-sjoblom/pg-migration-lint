//! PGM003 — `CONCURRENTLY` inside transaction
//!
//! Detects `CREATE INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY` inside
//! a migration unit that runs in a transaction. PostgreSQL does not allow
//! concurrent index operations inside a transaction block; the command
//! will fail at runtime.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "CONCURRENTLY inside transaction";

pub(super) const EXPLAIN: &str = "PGM003 — CONCURRENTLY inside transaction\n\
         \n\
         What it detects:\n\
         A CREATE INDEX CONCURRENTLY or DROP INDEX CONCURRENTLY statement\n\
         inside a migration unit that runs in a transaction.\n\
         \n\
         Why it's dangerous:\n\
         PostgreSQL does not allow CONCURRENTLY operations inside a\n\
         transaction block. The command will fail with:\n\
           ERROR: CREATE INDEX CONCURRENTLY cannot run inside a transaction block\n\
         This means the migration will fail at deploy time.\n\
         \n\
         Example (bad — Liquibase changeset with default runInTransaction):\n\
           <changeSet id=\"1\" author=\"dev\">\n\
             <sql>CREATE INDEX CONCURRENTLY idx_foo ON bar (col);</sql>\n\
           </changeSet>\n\
         \n\
         Fix:\n\
           <changeSet id=\"1\" author=\"dev\" runInTransaction=\"false\">\n\
             <sql>CREATE INDEX CONCURRENTLY idx_foo ON bar (col);</sql>\n\
           </changeSet>\n\
         \n\
         For go-migrate, add `-- +goose NO TRANSACTION` or equivalent to\n\
         the migration file header.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    if !ctx.run_in_transaction {
        return Vec::new();
    }

    let mut findings = Vec::new();

    for stmt in statements {
        let is_concurrent = match &stmt.node {
            IrNode::CreateIndex(ci) => ci.concurrent,
            IrNode::DropIndex(di) => di.concurrent,
            _ => false,
        };

        if is_concurrent {
            findings.push(
                rule.make_finding(
                    "CONCURRENTLY cannot run inside a transaction. \
                         Set runInTransaction=\"false\" (Liquibase) or disable \
                         transactions for this migration."
                        .to_string(),
                    ctx.file,
                    &stmt.span,
                ),
            );
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::*;
    use crate::rules::{RuleId, UnsafeDdlRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, true);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_foo".to_string()),
                QualifiedName::unqualified("bar"),
            )
            .with_columns(vec![IndexColumn {
                name: "col".to_string(),
            }])
            .with_concurrent(true),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm003).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_concurrent_no_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, false);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_foo".to_string()),
                QualifiedName::unqualified("bar"),
            )
            .with_columns(vec![IndexColumn {
                name: "col".to_string(),
            }])
            .with_concurrent(true),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm003).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_concurrent_in_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, true);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_foo".to_string()),
                QualifiedName::unqualified("bar"),
            )
            .with_columns(vec![IndexColumn {
                name: "col".to_string(),
            }]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm003).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_index_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, true);

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_foo")
                .with_concurrent(true)
                .with_if_exists(false),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm003).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
