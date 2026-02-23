//! PGM502 — Table without primary key
//!
//! Detects `CREATE TABLE` statements (non-temporary) that result in a table
//! without a primary key. Checks the catalog state AFTER the entire file is
//! processed, so `ALTER TABLE ... ADD PRIMARY KEY` later in the same file
//! avoids a false positive.

use crate::parser::ir::{IrNode, Located, TablePersistence};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Table without primary key";

pub(super) const EXPLAIN: &str = "PGM502 — Table without primary key\n\
         \n\
         What it detects:\n\
         A CREATE TABLE statement (non-temporary) that does not define a\n\
         PRIMARY KEY constraint, and no ALTER TABLE ... ADD PRIMARY KEY\n\
         follows in the same file.\n\
         \n\
         Why it's dangerous:\n\
         Tables without primary keys:\n\
         - Cannot be reliably targeted by logical replication.\n\
         - May cause issues with ORMs that require a PK for identity.\n\
         - Make it harder to deduplicate or reference specific rows.\n\
         - Are a strong code smell indicating incomplete schema design.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE events (event_type text, payload jsonb);\n\
         \n\
         Fix:\n\
           CREATE TABLE events (\n\
             id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,\n\
             event_type text,\n\
             payload jsonb\n\
           );\n\
         \n\
         Note: Temporary tables are excluded. If PGM503 fires (UNIQUE NOT NULL\n\
         used instead of PK), PGM502 does NOT fire for the same table.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::CreateTable(ref ct) = stmt.node {
            // Skip temporary tables.
            if ct.persistence == TablePersistence::Temporary {
                continue;
            }

            let table_key = ct.name.catalog_key();

            // Post-file check: look at catalog_after to see if a PK was added.
            let table_state = ctx.catalog_after.get_table(table_key);
            let has_pk = table_state.map(|t| t.has_primary_key).unwrap_or(false);

            if !has_pk {
                // Check for PGM503 condition: UNIQUE NOT NULL substitute.
                // If PGM503 would fire, suppress PGM502.
                let has_unique_not_null = table_state
                    .map(|t| t.has_unique_not_null())
                    .unwrap_or(false);

                if !has_unique_not_null {
                    findings.push(rule.make_finding(
                        format!("Table '{}' has no primary key.", ct.name.display_name()),
                        ctx.file,
                        &stmt.span,
                    ));
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
    use crate::rules::test_helpers::*;
    use crate::rules::{RuleId, SchemaDesignRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("event_type", "text", true)
                    .column("payload", "jsonb", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("event_type", "text"),
                ColumnDef::test("payload", "jsonb"),
            ]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_with_pk_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("id", "bigint", false)
                    .pk(&["id"])
                    .column("event_type", "text", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![
                    ColumnDef::test("id", "bigint")
                        .with_nullable(false)
                        .with_inline_pk(),
                    ColumnDef::test("event_type", "text"),
                ])
                .with_constraints(vec![TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_pk_added_later_in_file_no_finding() {
        let before = Catalog::new();
        // catalog_after has PK because replay already processed the ALTER TABLE
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("id", "integer", false)
                    .column("name", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("id", "integer").with_nullable(false),
                ColumnDef::test("name", "text"),
            ]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_temp_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("tmp_data"))
                .with_columns(vec![ColumnDef::test("val", "text")])
                .with_temporary(true),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_unique_index_not_null_suppresses_pgm502() {
        let before = Catalog::new();
        // Table has unique index on NOT NULL column but no PK — PGM503 fires, PGM502 should not.
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("email", "text", false)
                    .index("idx_email_unique", &["email"], true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_unique_not_null_suppresses_pgm004() {
        let before = Catalog::new();
        // Table has UNIQUE NOT NULL but no PK — PGM503 fires, PGM502 should not.
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partial_unique_index_does_not_suppress_pgm502() {
        let before = Catalog::new();
        // Table has a partial unique index on NOT NULL column — should NOT suppress PGM502
        // because partial indexes don't count as PK substitutes.
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("email", "text", false).partial_index(
                    "idx_email_unique",
                    &["email"],
                    true,
                    "deleted_at IS NULL",
                );
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm502).check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partial unique index should NOT suppress PGM502"
        );
    }
}
