//! PGM023 — `VACUUM FULL` on existing table
//!
//! Detects `VACUUM FULL` targeting a table that exists in `catalog_before`.
//! `VACUUM FULL` rewrites the entire table under an ACCESS EXCLUSIVE lock,
//! blocking all reads and writes for the full duration. Use `pg_repack` or
//! `pg_squeeze` for online compaction.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "VACUUM FULL on existing table";

pub(super) const EXPLAIN: &str = "PGM023 — VACUUM FULL on existing table\n\
         \n\
         What it detects:\n\
         A VACUUM FULL statement targeting a table that already exists in the\n\
         database (i.e., the table was not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         VACUUM FULL rewrites the entire table into a new data file, holding\n\
         an ACCESS EXCLUSIVE lock for the full duration. Unlike regular VACUUM,\n\
         which runs concurrently with reads and writes, VACUUM FULL blocks\n\
         everything. On large tables this causes complete unavailability\n\
         (all reads and writes blocked) for minutes to hours.\n\
         \n\
         Example:\n\
           VACUUM FULL orders;\n\
         \n\
         Recommended approach:\n\
         1. Use pg_repack or pg_squeeze for online table compaction.\n\
         2. Schedule VACUUM FULL during a maintenance window when downtime is acceptable.\n\
         3. For new tables, VACUUM FULL is fine — this rule only fires on existing tables.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::VacuumFull(ref v) = stmt.node {
            match &v.table {
                Some(table) => {
                    if ctx.is_existing_table(table.catalog_key()) {
                        findings.push(rule.make_finding(
                            format!(
                                "VACUUM FULL on table '{}' rewrites the entire table under \
                                 ACCESS EXCLUSIVE lock, blocking all reads and writes. \
                                 Use pg_repack or pg_squeeze for online compaction.",
                                table.display_name(),
                            ),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
                None => {
                    findings.push(
                        rule.make_finding(
                            "VACUUM FULL without a table list targets every table in the \
                         database, each rewritten under ACCESS EXCLUSIVE lock. \
                         Use pg_repack or pg_squeeze for online compaction."
                                .to_string(),
                            ctx.file,
                            &stmt.span,
                        ),
                    );
                }
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
    fn vacuum_full_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn vacuum_full_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn vacuum_full_schema_qualified_fires() {
        let before = CatalogBuilder::new()
            .table("myschema.orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::qualified("myschema", "orders")).into(),
        )];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("myschema.orders"),
            "message should include schema-qualified name, got: {}",
            findings[0].message,
        );
    }

    #[test]
    fn vacuum_full_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::unqualified("nonexistent")).into(),
        )];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn vacuum_full_all_tables_always_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(VacuumFull::test_all().into())];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("every table"),
            "message should mention all tables, got: {}",
            findings[0].message,
        );
    }
}
