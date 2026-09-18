#![doc = include_str!("docs/pgm021.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "VACUUM FULL on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm021.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

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
    use crate::rules::test_helpers::{lint_ctx, located};

    #[test]
    fn vacuum_full_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = RuleId::Pgm021.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = RuleId::Pgm021.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::qualified("myschema", "orders")).into(),
        )];

        let findings = RuleId::Pgm021.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            VacuumFull::test(QualifiedName::unqualified("nonexistent")).into(),
        )];

        let findings = RuleId::Pgm021.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn vacuum_full_all_tables_always_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(VacuumFull::test_all().into())];

        let findings = RuleId::Pgm021.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("every table"),
            "message should mention all tables, got: {}",
            findings[0].message,
        );
    }
}
