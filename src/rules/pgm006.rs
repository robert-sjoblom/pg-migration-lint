//! PGM006 — `CONCURRENTLY` inside transaction
//!
//! Detects `CREATE INDEX CONCURRENTLY` or `DROP INDEX CONCURRENTLY` inside
//! a migration unit that runs in a transaction. PostgreSQL does not allow
//! concurrent index operations inside a transaction block; the command
//! will fail at runtime.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags concurrent index operations inside a transaction.
pub struct Pgm006;

impl Rule for Pgm006 {
    fn id(&self) -> &'static str {
        "PGM006"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "CONCURRENTLY inside transaction"
    }

    fn explain(&self) -> &'static str {
        "PGM006 — CONCURRENTLY inside transaction\n\
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
         the migration file header."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
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
                findings.push(Finding::new(
                    self.id(),
                    self.default_severity(),
                    "CONCURRENTLY cannot run inside a transaction. \
                         Set runInTransaction=\"false\" (Liquibase) or disable \
                         transactions for this migration."
                        .to_string(),
                    ctx.file,
                    &stmt.span,
                ));
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, true);

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex {
            index_name: Some("idx_foo".to_string()),
            table_name: QualifiedName::unqualified("bar"),
            columns: vec![IndexColumn {
                name: "col".to_string(),
            }],
            unique: false,
            concurrent: true,
        }))];

        let findings = Pgm006.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM006");
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].message.contains("CONCURRENTLY"));
    }

    #[test]
    fn test_concurrent_no_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, false);

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex {
            index_name: Some("idx_foo".to_string()),
            table_name: QualifiedName::unqualified("bar"),
            columns: vec![IndexColumn {
                name: "col".to_string(),
            }],
            unique: false,
            concurrent: true,
        }))];

        let findings = Pgm006.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_concurrent_in_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, true);

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex {
            index_name: Some("idx_foo".to_string()),
            table_name: QualifiedName::unqualified("bar"),
            columns: vec![IndexColumn {
                name: "col".to_string(),
            }],
            unique: false,
            concurrent: false,
        }))];

        let findings = Pgm006.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_index_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx_with_txn(&before, &after, &file, &created, true);

        let stmts = vec![located(IrNode::DropIndex(DropIndex {
            index_name: "idx_foo".to_string(),
            concurrent: true,
        }))];

        let findings = Pgm006.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM006");
    }
}
