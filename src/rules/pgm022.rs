//! PGM022 — Missing `CONCURRENTLY` on `REINDEX`
//!
//! Detects `REINDEX TABLE|INDEX|SCHEMA|DATABASE|SYSTEM` without `CONCURRENTLY`.
//! `REINDEX` without `CONCURRENTLY` acquires an ACCESS EXCLUSIVE lock on the
//! target table (or parent table for `REINDEX INDEX`), blocking all reads and
//! writes. Use `REINDEX ... CONCURRENTLY` (PostgreSQL 12+).

use crate::parser::ir::{IrNode, Located, ReindexTarget};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Missing CONCURRENTLY on REINDEX";

pub(super) const EXPLAIN: &str = "PGM022 — Missing CONCURRENTLY on REINDEX\n\
         \n\
         What it detects:\n\
         A REINDEX statement (TABLE, INDEX, SCHEMA, DATABASE, or SYSTEM) that\n\
         does not use the CONCURRENTLY option.\n\
         \n\
         Why it's dangerous:\n\
         REINDEX without CONCURRENTLY acquires an ACCESS EXCLUSIVE lock on the\n\
         table being reindexed (or the parent table for REINDEX INDEX), blocking\n\
         all reads and writes for the duration of the rebuild. On large tables\n\
         this causes complete unavailability for minutes to hours.\n\
         \n\
         Example (bad):\n\
           REINDEX TABLE orders;\n\
           REINDEX INDEX idx_orders_status;\n\
         \n\
         Fix:\n\
           REINDEX TABLE CONCURRENTLY orders;\n\
           REINDEX INDEX CONCURRENTLY idx_orders_status;\n\
         \n\
         The CONCURRENTLY option (PostgreSQL 12+) rebuilds the index without\n\
         holding an exclusive lock for the entire operation. It takes longer\n\
         but allows normal reads and writes to continue.\n\
         \n\
         Note: CONCURRENTLY cannot run inside a transaction. If your migration\n\
         framework wraps each file in a transaction (e.g., Liquibase default),\n\
         you must also disable that. See PGM003.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::Reindex(ref r) = stmt.node {
            if r.concurrent {
                continue;
            }

            let target_display = match &r.target {
                ReindexTarget::Relation(name) => name.display_name().to_string(),
                ReindexTarget::Named(name) => name.clone(),
            };

            findings.push(rule.make_finding(
                format!(
                    "REINDEX {} '{target_display}' should use CONCURRENTLY to avoid \
                     holding an ACCESS EXCLUSIVE lock. \
                     Use REINDEX {} CONCURRENTLY '{target_display}' (PostgreSQL 12+).",
                    r.kind, r.kind,
                ),
                ctx.file,
                &stmt.span,
            ));
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn reindex_table_without_concurrently_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Reindex::test_table(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = RuleId::Pgm022.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn reindex_concurrently_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Reindex::test_table(QualifiedName::unqualified("orders"))
                .with_concurrent()
                .into(),
        )];

        let findings = RuleId::Pgm022.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn all_reindex_kinds_fire_without_concurrently() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let cases: Vec<(&str, IrNode)> = vec![
            (
                "INDEX",
                Reindex::test_index(QualifiedName::unqualified("idx_foo")).into(),
            ),
            ("SCHEMA", Reindex::test_schema("public").into()),
            ("DATABASE", Reindex::test_database("mydb").into()),
        ];

        for (kind_label, node) in cases {
            let stmts = vec![located(node)];
            let findings = RuleId::Pgm022.check(&stmts, &ctx);
            assert_eq!(
                findings.len(),
                1,
                "REINDEX {kind_label} should produce exactly one finding",
            );
            assert!(
                findings[0]
                    .message
                    .contains(&format!("REINDEX {kind_label}")),
                "message should mention REINDEX {kind_label}, got: {}",
                findings[0].message,
            );
        }
    }

    #[test]
    fn reindex_schema_qualified_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Reindex::test_table(QualifiedName::qualified("myschema", "orders")).into(),
        )];

        let findings = RuleId::Pgm022.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("myschema.orders"),
            "message should include schema-qualified name, got: {}",
            findings[0].message,
        );
    }
}
