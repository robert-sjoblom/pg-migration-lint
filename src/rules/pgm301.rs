//! PGM301 — `INSERT INTO` existing table in migration
//!
//! Detects `INSERT INTO` statements targeting tables that already exist in
//! the database. Seed data in migrations is common but should be bounded
//! in volume and clearly intentional.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "INSERT INTO existing table in migration";

pub(super) const EXPLAIN: &str = "PGM301 — INSERT INTO existing table in migration\n\
         \n\
         What it detects:\n\
         An INSERT INTO statement targeting a table that already exists in the\n\
         database (i.e., not created in the same set of changed files).\n\
         \n\
         Why it matters:\n\
         INSERT statements in migrations are sometimes used for seed data,\n\
         lookup table population, or data backfill. While these are valid\n\
         use cases, they deserve human review because:\n\
         - Large inserts can cause lock contention and WAL pressure.\n\
         - Unbounded inserts may time out under migration tool timeouts.\n\
         - Seed data should be idempotent (check for duplicates).\n\
         \n\
         Example (flagged):\n\
           INSERT INTO config (key, value) VALUES ('feature_x', 'enabled');\n\
         \n\
         Not flagged:\n\
         - INSERT into a table created in the same migration file\n\
           (populating a brand-new table is expected).\n\
         \n\
         This rule is INFO severity — it flags the DML for awareness rather\n\
         than treating it as a defect.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::InsertInto(ref ii) = stmt.node {
            let table_key = ii.table_name.catalog_key();

            if ctx.is_existing_table(table_key) {
                findings.push(rule.make_finding(
                    format!(
                        "INSERT INTO existing table '{}' in a migration. \
                         Ensure this is intentional seed data and that row volume is bounded.",
                        ii.table_name.display_name()
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
        RuleId::Pgm301
    }

    #[test]
    fn test_insert_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("config", |t| {
                t.column("key", "text", false)
                    .column("value", "text", true)
                    .pk(&["key"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/005.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            InsertInto::test(QualifiedName::unqualified("config")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_insert_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("config", |t| {
                t.column("key", "text", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("config".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            InsertInto::test(QualifiedName::unqualified("config")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_insert_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            InsertInto::test(QualifiedName::unqualified("config")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
