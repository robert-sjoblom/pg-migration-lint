//! PGM506 — `CREATE UNLOGGED TABLE`
//!
//! Detects `CREATE UNLOGGED TABLE` statements. Unlogged tables are not
//! written to the write-ahead log, which makes them faster for write-heavy
//! workloads but means they are truncated after a crash and are not
//! replicated to standby servers.

use crate::parser::ir::{IrNode, Located, TablePersistence};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "CREATE UNLOGGED TABLE";

pub(super) const EXPLAIN: &str = "PGM506 — CREATE UNLOGGED TABLE\n\
         \n\
         What it detects:\n\
         A CREATE TABLE statement that uses the UNLOGGED keyword.\n\
         \n\
         Why it matters:\n\
         Unlogged tables offer better write performance because they skip\n\
         the write-ahead log, but come with significant trade-offs:\n\
         - Data is TRUNCATED after a crash or unclean shutdown.\n\
         - The table is NOT replicated to standby servers.\n\
         - They cannot participate in logical replication.\n\
         These characteristics make unlogged tables unsuitable for any\n\
         data that must survive a crash or be available on replicas.\n\
         \n\
         Example (flagged):\n\
           CREATE UNLOGGED TABLE scratch_data (id int, payload text);\n\
         \n\
         When unlogged tables are appropriate:\n\
         - Ephemeral staging/import data that can be re-derived.\n\
         - Materialised caches where the source of truth lives elsewhere.\n\
         - ETL scratch space within a batch job.\n\
         \n\
         This rule is INFO severity — it flags the table for review rather\n\
         than treating it as a defect.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::CreateTable(ref ct) = stmt.node
            && ct.persistence == TablePersistence::Unlogged
        {
            findings.push(rule.make_finding(
                format!(
                    "CREATE UNLOGGED TABLE '{}'. Unlogged tables are truncated on \
                     crash recovery and are not replicated to standbys.",
                    ct.name.display_name()
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
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{RuleId, SchemaDesignRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn rule_id() -> RuleId {
        RuleId::SchemaDesign(SchemaDesignRule::Pgm506)
    }

    #[test]
    fn test_unlogged_table_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            CreateTable::test(QualifiedName::unqualified("scratch"))
                .with_columns(vec![ColumnDef::test("id", "integer")])
                .with_persistence(TablePersistence::Unlogged)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_permanent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![ColumnDef::test("id", "integer")])
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_temporary_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            CreateTable::test(QualifiedName::unqualified("tmp_data"))
                .with_columns(vec![ColumnDef::test("id", "integer")])
                .with_temporary(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
