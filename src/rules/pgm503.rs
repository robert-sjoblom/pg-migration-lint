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
         for the same table, since the situation is already flagged.\n\
         \n\
         Partition children (CREATE TABLE ... PARTITION OF parent) inherit the\n\
         primary key from their parent table. This rule is suppressed for\n\
         partition children when the parent already has a PK or when the\n\
         parent is not in the catalog (common in incremental CI where only\n\
         new migrations are analyzed).";

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

            // Partition children inherit PK from their parent.
            if ctx.partition_child_inherits_pk(ct.partition_of.as_ref(), table_key) {
                continue;
            }
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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::*;
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

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_unique_index_not_null_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .column("name", "text", true)
                    .index("idx_email_unique", &["email"], true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users")).with_columns(vec![
                ColumnDef::test("email", "text").with_nullable(false),
                ColumnDef::test("name", "text"),
            ]),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
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

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partition_child_parent_has_pk_suppressed() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("email", "text", false)
                    .pk(&["email"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["email"]);
            })
            .table("child", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"])
                    .partition_of("parent");
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }])
                .with_partition_of(QualifiedName::unqualified("parent")),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partition_child_parent_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("email", "text", false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["email"]);
            })
            .table("child", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"])
                    .partition_of("parent");
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }])
                .with_partition_of(QualifiedName::unqualified("parent")),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "Should fire when parent lacks PK");
    }

    #[test]
    fn test_partition_child_parent_not_in_catalog_suppressed() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"])
                    .partition_of("unknown_parent");
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }])
                .with_partition_of(QualifiedName::unqualified("unknown_parent")),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_attach_partition_parent_has_pk_suppressed() {
        // Table created without PARTITION OF, but ATTACHed as partition.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("email", "text", false)
                    .pk(&["email"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["email"]);
            })
            .table("child", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"])
                    .partition_of("parent");
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        // IR has no partition_of — the table was ATTACHed, not created with PARTITION OF.
        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "ATTACH PARTITION child with parent PK should be suppressed"
        );
    }

    #[test]
    fn test_partitioned_parent_unique_not_null_no_pk_fires() {
        // Partitioned parent with UNIQUE NOT NULL but no PK — PGM503 fires.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("parent"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }])
                .with_partition_by(
                    crate::parser::ir::PartitionStrategy::Range,
                    vec!["email".to_string()],
                ),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partitioned parent with UNIQUE NOT NULL but no PK should fire PGM503"
        );
    }

    #[test]
    fn test_drop_not_null_disqualifies_unique_not_null() {
        // CREATE TABLE has UNIQUE NOT NULL in IR, but a subsequent DROP NOT NULL
        // made the column nullable. catalog_after reflects nullable column,
        // so has_unique_not_null() returns false and PGM503 should NOT fire.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("settings", |t| {
                t.column("key", "text", true) // nullable after DROP NOT NULL
                    .column("value", "text", true)
                    .unique("uq_key", &["key"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("settings"))
                .with_columns(vec![
                    ColumnDef::test("key", "text").with_nullable(false),
                    ColumnDef::test("value", "text"),
                ])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uq_key".to_string()),
                    columns: vec!["key".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "PGM503 should NOT fire after DROP NOT NULL made the UNIQUE column nullable"
        );
    }

    #[test]
    fn test_partial_unique_not_null_does_not_trigger_pgm503() {
        let before = Catalog::new();
        // Table has partial unique index on NOT NULL column — should NOT trigger PGM503
        // because partial indexes don't count as PK substitutes.
        let after = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .column("name", "text", true)
                    .partial_index("idx_email_unique", &["email"], true, "deleted_at IS NULL");
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users")).with_columns(vec![
                ColumnDef::test("email", "text").with_nullable(false),
                ColumnDef::test("name", "text"),
            ]),
        ))];

        let findings = RuleId::Pgm503.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Partial unique index should NOT trigger PGM503"
        );
    }
}
