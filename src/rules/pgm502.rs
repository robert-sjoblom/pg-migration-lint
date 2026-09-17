#![doc = include_str!("docs/pgm502.md")]

use crate::parser::ir::{IrNode, Located, TablePersistence};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Table without primary key";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm502.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::*;

    #[test]
    fn test_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("event_type", "text", true)
                    .column("payload", "jsonb", true);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("event_type", "text"),
                ColumnDef::test("payload", "jsonb"),
            ]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![
                    ColumnDef::test("id", "bigint")
                        .with_nullable(false)
                        .with_inline_pk(),
                    ColumnDef::test("event_type", "text"),
                ])
                .with_constraints(vec![TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["id".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("id", "integer").with_nullable(false),
                ColumnDef::test("name", "text"),
            ]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_temp_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("tmp_data"))
                .with_columns(vec![ColumnDef::test("val", "text")])
                .with_persistence(TablePersistence::Temporary),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)])
                .with_constraints(vec![TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_btree_unique_index_not_null_does_not_suppress_pgm502() {
        // A non-btree (hash) unique index on NOT NULL column should NOT suppress PGM502.
        // has_unique_not_null() filters non-btree, so PGM502 should fire.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("email", "text", false).index_with_method(
                    "idx_email_hash",
                    &["email"],
                    true,
                    "hash",
                );
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Non-btree unique index should NOT suppress PGM502"
        );
    }

    #[test]
    fn test_partition_child_parent_has_pk_suppressed() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)])
                .with_partition_of(QualifiedName::unqualified("parent")),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partition_child_parent_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)])
                .with_partition_of(QualifiedName::unqualified("parent")),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "Should fire when parent lacks PK");
    }

    #[test]
    fn test_partition_child_parent_not_in_catalog_suppressed() {
        let before = Catalog::new();
        // Parent is NOT in the catalog (incremental CI case).
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("id", "integer", false)
                    .partition_of("unknown_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)])
                .with_partition_of(QualifiedName::unqualified("unknown_parent")),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partitioned_parent_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("parent"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)])
                .with_partition_by(
                    crate::parser::ir::PartitionStrategy::Range,
                    vec!["id".to_string()],
                ),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partitioned parent without PK should fire normally"
        );
    }

    #[test]
    fn test_attach_partition_parent_has_pk_suppressed() {
        // Table created without PARTITION OF, but ATTACHed as partition.
        // The catalog's parent_table is set by replay of ATTACH PARTITION.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        // IR has no partition_of — the table was ATTACHed, not created with PARTITION OF.
        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "ATTACH PARTITION child with parent PK should be suppressed"
        );
    }

    #[test]
    fn test_attach_partition_parent_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false).partition_of("parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        // IR has no partition_of — ATTACHed child, parent lacks PK.
        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "ATTACH PARTITION child should fire when parent lacks PK"
        );
    }

    #[test]
    fn test_schema_qualified_partition_parent_suppressed() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("myschema.parent", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("myschema.child", |t| {
                t.column("id", "integer", false)
                    .partition_of("myschema.parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::qualified("myschema", "child"))
                .with_columns(vec![ColumnDef::test("id", "integer").with_nullable(false)])
                .with_partition_of(QualifiedName::qualified("myschema", "parent")),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Schema-qualified partition child with parent PK should be suppressed"
        );
    }

    #[test]
    fn test_pk_removed_by_drop_constraint_fires() {
        // CREATE TABLE defines a PK in IR, but a subsequent DROP CONSTRAINT
        // in the same file removed it. catalog_after reflects no PK.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true);
                // No PK — it was dropped by ALTER TABLE DROP CONSTRAINT
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![
                    ColumnDef::test("id", "bigint")
                        .with_nullable(false)
                        .with_inline_pk(),
                    ColumnDef::test("status", "text"),
                ])
                .with_constraints(vec![TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["id".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "PGM502 should fire when PK was removed by DROP CONSTRAINT"
        );
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("email", "text").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm502.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partial unique index should NOT suppress PGM502"
        );
    }
}
