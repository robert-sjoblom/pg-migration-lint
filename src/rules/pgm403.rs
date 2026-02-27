//! PGM403 — `CREATE TABLE IF NOT EXISTS` for already-existing table
//!
//! Detects `CREATE TABLE IF NOT EXISTS` where the table already exists in the
//! migration history. The statement is a silent no-op in PostgreSQL, meaning
//! the column definitions in this statement are ignored. If they differ from
//! the actual table state, the migration chain is ambiguous and misleading.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str =
    "CREATE TABLE IF NOT EXISTS for already-existing table is a misleading no-op";

pub(super) const EXPLAIN: &str = "PGM403 — CREATE TABLE IF NOT EXISTS for already-existing table\n\
         \n\
         What it detects:\n\
         A CREATE TABLE IF NOT EXISTS statement targeting a table that already\n\
         exists in the migration history (i.e. was created by an earlier migration).\n\
         \n\
         Why it matters:\n\
         IF NOT EXISTS makes the statement a silent no-op when the table already exists.\n\
         If the column definitions in the CREATE TABLE differ from the actual table state\n\
         (built up from the original CREATE TABLE plus subsequent ALTER TABLE statements),\n\
         the migration author may believe the table has the shape described in this\n\
         statement, when in reality PostgreSQL ignores it entirely. The migration chain\n\
         is ambiguous — two competing definitions of the same table exist in the history,\n\
         and only the first one (plus its alterations) is truth.\n\
         \n\
         Example:\n\
           -- V001: original table\n\
           CREATE TABLE orders (id bigint PRIMARY KEY);\n\
           ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';\n\
           \n\
           -- V010: redundant re-creation (silently ignored)\n\
           CREATE TABLE IF NOT EXISTS orders (\n\
               id bigint PRIMARY KEY,\n\
               status text NOT NULL DEFAULT 'pending',\n\
               created_at timestamptz DEFAULT now()  -- this column will NOT be added\n\
           );\n\
         \n\
         Recommended fix:\n\
           Remove the redundant CREATE TABLE IF NOT EXISTS. If the intent is to\n\
           add columns, use ALTER TABLE ... ADD COLUMN instead.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::CreateTable(ct) = &stmt.node
            && ct.if_not_exists
        {
            let key = ct.name.catalog_key();
            if ctx.catalog_before.has_table(key) {
                findings.push(rule.make_finding(
                    format!(
                        "CREATE TABLE IF NOT EXISTS '{}' is a no-op \u{2014} the table already \
                             exists in the migration history. The definition in this statement is \
                             silently ignored by PostgreSQL. If the column definitions differ from \
                             the actual table state, this migration is misleading.",
                        ct.name.display_name()
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

    #[test]
    fn fires_when_table_exists_in_catalog_before() {
        let before = CatalogBuilder::new()
            .table("public.customers", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::qualified("public", "customers"))
                .with_if_not_exists(true),
        ))];

        let findings = RuleId::Pgm403.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn no_finding_when_table_does_not_exist() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_if_not_exists(true),
        ))];

        let findings = RuleId::Pgm403.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn no_finding_without_if_not_exists() {
        let before = CatalogBuilder::new()
            .table("public.customers", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        // CREATE TABLE without IF NOT EXISTS — PGM403 should not fire
        // (this would be caught by PGM402 instead)
        let stmts = vec![located(IrNode::CreateTable(CreateTable::test(
            QualifiedName::qualified("public", "customers"),
        )))];

        let findings = RuleId::Pgm403.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
