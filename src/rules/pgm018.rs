//! PGM018 — `CLUSTER` on existing table
//!
//! Detects `CLUSTER table_name [USING index_name]` targeting a table that
//! exists in `catalog_before`. `CLUSTER` rewrites the entire table and all
//! indexes under `ACCESS EXCLUSIVE` lock, blocking all reads and writes for
//! the full duration. There is no online alternative.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "CLUSTER on existing table";

pub(super) const EXPLAIN: &str = "PGM018 — CLUSTER on existing table\n\
         \n\
         What it detects:\n\
         A CLUSTER statement targeting a table that already exists in the\n\
         database (i.e., the table was not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         CLUSTER rewrites the entire table and all its indexes in a new\n\
         physical order, holding an ACCESS EXCLUSIVE lock for the full\n\
         duration of the rewrite. Unlike VACUUM FULL, there is no online\n\
         alternative. On large tables this causes complete unavailability\n\
         (all reads and writes blocked) for the duration — typically minutes\n\
         to hours. It is almost never appropriate in an online migration.\n\
         \n\
         Example:\n\
           CLUSTER orders USING idx_orders_created_at;\n\
         \n\
         Recommended approach:\n\
         1. Schedule CLUSTER during a maintenance window when downtime is acceptable.\n\
         2. Consider pg_repack or pg_squeeze for online table rewrites.\n\
         3. For new tables, CLUSTER is fine — this rule only fires on existing tables.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::Cluster(ref c) = stmt.node {
            let table_key = c.table.catalog_key();

            if ctx.is_existing_table(table_key) {
                let message = match &c.index {
                    Some(idx) => format!(
                        "CLUSTER on table '{}' USING '{}' rewrites the entire table \
                         under ACCESS EXCLUSIVE lock for the full duration. \
                         All reads and writes are blocked. \
                         This is rarely appropriate in an online migration.",
                        c.table.display_name(),
                        idx,
                    ),
                    None => format!(
                        "CLUSTER on table '{}' rewrites the entire table \
                         under ACCESS EXCLUSIVE lock for the full duration. \
                         All reads and writes are blocked. \
                         This is rarely appropriate in an online migration.",
                        c.table.display_name(),
                    ),
                };

                findings.push(rule.make_finding(message, ctx.file, &stmt.span));
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

    #[test]
    fn cluster_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .index("idx_customers_email", &["email"], false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("customers"))
                .with_index("idx_customers_email")
                .into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn cluster_without_index_fires() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("customers")).into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn cluster_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("customers".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("customers"))
                .with_index("idx_customers_email")
                .into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn cluster_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("nonexistent"))
                .with_index("idx_foo")
                .into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
