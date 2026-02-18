//! PGM503 — `UNIQUE NOT NULL` used instead of primary key
//!
//! Detects tables that have no primary key but have at least one UNIQUE
//! constraint where all constituent columns are NOT NULL. This is functionally
//! equivalent to a PK but less conventional.

use crate::parser::ir::{IrNode, Located, TablePersistence};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "UNIQUE NOT NULL used instead of PRIMARY KEY";

pub(super) const EXPLAIN: &str = "PGM503 — UNIQUE NOT NULL used instead of PRIMARY KEY\n\
         \n\
         What it detects:\n\
         A table that has no PRIMARY KEY but has at least one UNIQUE constraint\n\
         where all constituent columns are NOT NULL. This combination is\n\
         functionally equivalent to a PK.\n\
         \n\
         Why it matters:\n\
         While UNIQUE NOT NULL is functionally equivalent to PRIMARY KEY,\n\
         using PRIMARY KEY is more conventional and explicit. Tools, ORMs,\n\
         and database administrators expect PK as the standard way to\n\
         identify rows. Using UNIQUE NOT NULL may confuse readers and\n\
         prevent some tools from auto-detecting the identity column.\n\
         \n\
         Example (flagged):\n\
           CREATE TABLE users (\n\
             email text NOT NULL UNIQUE,\n\
             name text\n\
           );\n\
         \n\
         Fix:\n\
           CREATE TABLE users (\n\
             email text PRIMARY KEY,\n\
             name text\n\
           );\n\
         \n\
         Note: When PGM503 fires, PGM502 (table without PK) does NOT fire\n\
         for the same table, since the situation is already flagged.";

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
            let table_state = ctx.catalog_after.get_table(table_key);

            let has_pk = table_state.map(|t| t.has_primary_key).unwrap_or(false);

            if !has_pk {
                let has_unique_not_null = table_state
                    .map(|t| t.has_unique_not_null())
                    .unwrap_or(false);

                if has_unique_not_null {
                    findings.push(rule.make_finding(
                        format!(
                            "Table '{}' uses UNIQUE NOT NULL instead of PRIMARY KEY. \
                                 Functionally equivalent but PRIMARY KEY is conventional \
                                 and more explicit.",
                            ct.name.display_name()
                        ),
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
    fn test_unique_not_null_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .column("name", "text", true)
                    .unique("uk_email", &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users"))
                .with_columns(vec![
                    ColumnDef::test("email", "text").with_nullable(false),
                    ColumnDef::test("name", "text"),
                ])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm503).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_with_pk_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .unique("uk_email", &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users"))
                .with_columns(vec![
                    ColumnDef::test("id", "integer")
                        .with_nullable(false)
                        .with_inline_pk(),
                    ColumnDef::test("email", "text").with_nullable(false),
                ])
                .with_constraints(vec![
                    TableConstraint::PrimaryKey {
                        columns: vec!["id".to_string()],
                        using_index: None,
                    },
                    TableConstraint::Unique {
                        name: Some("uk_email".to_string()),
                        columns: vec!["email".to_string()],
                        using_index: None,
                    },
                ]),
        ))];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm503).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
