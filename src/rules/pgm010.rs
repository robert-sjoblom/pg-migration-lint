#![doc = include_str!("docs/pgm010.md")]

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, drop_column_check};

pub(super) const DESCRIPTION: &str = "DROP COLUMN silently removes unique constraint";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm010.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    drop_column_check::check_drop_column_constraints(
        statements,
        ctx,
        |name, at, table, stmt, ctx| {
            let mut findings = Vec::new();
            // Backing indexes the index loop below must skip. A backing index
            // has its constraint's name. A PK's is PGM011's to report. A
            // UNIQUE constraint's is reported by the constraint loop:
            // PostgreSQL drops a constraint and its backing index as one
            // atomic action, so the index loop must not report the same
            // removal again.
            let mut backing_index_names: Vec<&str> = table
                .constraints
                .iter()
                .filter(|c| matches!(c, ConstraintState::PrimaryKey { .. }))
                .map(ConstraintState::name)
                .collect();

            // Check UNIQUE constraints that include this column.
            for constraint in table.constraints_involving_column(name) {
                if let ConstraintState::Unique {
                    name: constraint_name,
                    ..
                } = constraint
                {
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' silently \
                             removes unique constraint '{constraint_name}'. Verify \
                             that the uniqueness guarantee is no longer needed.",
                            col = name,
                            table = at.name.display_name(),
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                    backing_index_names.push(constraint_name);
                }
            }

            // Check unique indexes that include this column.
            for idx in table.indexes_involving_column(name) {
                if idx.unique && !backing_index_names.contains(&idx.name.as_str()) {
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' silently \
                             removes unique constraint '{constraint}'. Verify that \
                             the uniqueness guarantee is no longer needed.",
                            col = name,
                            table = at.name.display_name(),
                            constraint = idx.name,
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                }
            }

            findings
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, located, normalized};

    #[test]
    fn test_drop_column_with_unique_constraint_fires() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .unique("uq_users_email", &["email"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_column_with_unique_index_fires() {
        let before = CatalogBuilder::new()
            .table("products", |t| {
                t.column("id", "integer", false)
                    .column("code", "text", false)
                    .pk(&["id"])
                    .index("idx_products_code_unique", &["code"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("products"),
            actions: vec![AlterTableAction::DropColumn {
                name: "code".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_column_not_in_unique_no_finding() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .column("name", "text", true)
                    .pk(&["id"])
                    .unique("uq_users_email", &["email"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "name".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_column_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("nonexistent"),
            actions: vec![AlterTableAction::DropColumn {
                name: "col".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_pk_column_does_not_fire_pgm010() {
        // PK columns are handled by PGM011, not PGM010.
        // PGM010 checks UNIQUE constraints and unique indexes, not PKs.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        // PK constraints are ConstraintState::PrimaryKey, not Unique, so PGM013 ignores them.
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fires_even_when_table_in_created_set() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .unique("uq_users_email", &["email"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", created: ["users"]);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RuleId::Pgm010);
    }

    #[test]
    fn test_multi_column_unique_drop_one_fires() {
        let before = CatalogBuilder::new()
            .table("subscriptions", |t| {
                t.column("id", "integer", false)
                    .column("a", "text", false)
                    .column("b", "text", false)
                    .pk(&["id"])
                    .unique("uq_subscriptions_a_b", &["a", "b"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("subscriptions"),
            actions: vec![AlterTableAction::DropColumn {
                name: "a".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_column_with_plain_unique_constraint_fires_once() {
        // A UNIQUE constraint added without USING INDEX has a synthetic
        // backing index in the catalog (see catalog::replay). PostgreSQL
        // drops the constraint and its backing index as one atomic action,
        // so PGM010 must report one finding, not one per (constraint, index)
        // pair — otherwise every plain UNIQUE constraint drop double-counts.
        use crate::catalog::replay::apply;
        use crate::input::MigrationUnit;
        use std::path::PathBuf;

        let mut catalog = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"]);
            })
            .build();

        let unit = MigrationUnit {
            id: "add_unique".to_string(),
            statements: vec![Located {
                node: IrNode::AlterTable(AlterTable {
                    name: QualifiedName::unqualified("users").resolve("public"),
                    actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
                        name: Some("uq_users_email".to_string()),
                        columns: vec!["email".to_string()],
                        include_columns: vec![],
                        using_index: None,
                    })],
                }),
                span: SourceSpan {
                    start_line: 1,
                    end_line: 1,
                    start_offset: 0,
                    end_offset: 0,
                },
            }],
            source_file: PathBuf::from("migrations/001.sql"),
            source_line_offset: 1,
            run_in_transaction: true,
            is_down: false,
        };
        apply(&mut catalog, &unit);

        let before = catalog;
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "constraint and its own backing index are one event, not two"
        );
    }

    #[test]
    fn test_drop_unique_column_created_via_using_index_fires() {
        // UNIQUE constraint was created via ADD UNIQUE USING INDEX — replay resolves
        // the index columns into the constraint so DROP COLUMN detects it.
        use crate::catalog::replay::apply;
        use crate::input::MigrationUnit;
        use std::path::PathBuf;

        // Step 1: build a table with an index (no unique constraint yet).
        let mut catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .index("idx_orders_email", &["email"], true);
            })
            .build();

        // Step 2: replay ADD UNIQUE USING INDEX to get resolved columns.
        let unit = MigrationUnit {
            id: "add_unique".to_string(),
            statements: vec![Located {
                node: IrNode::AlterTable(AlterTable {
                    name: QualifiedName::unqualified("orders").resolve("public"),
                    actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
                        name: Some("uq_orders_email".to_string()),
                        columns: vec![], // empty with USING INDEX
                        using_index: Some("idx_orders_email".to_string()),
                        include_columns: vec![],
                    })],
                }),
                span: SourceSpan {
                    start_line: 1,
                    end_line: 1,
                    start_offset: 0,
                    end_offset: 0,
                },
            }],
            source_file: PathBuf::from("migrations/001.sql"),
            source_line_offset: 1,
            run_in_transaction: true,
            is_down: false,
        };
        apply(&mut catalog, &unit);

        let before = catalog;
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        // Fires once: the constraint and its USING INDEX-attached backing
        // index are the same PostgreSQL object, not two independent ones.
        assert_eq!(
            findings.len(),
            1,
            "Should detect unique constraint removal even when created via USING INDEX"
        );
    }

    #[test]
    fn test_drop_multi_column_unique_partial_drop_fires() {
        // Multi-column unique constraint, drop only one column still fires
        let before = CatalogBuilder::new()
            .table("products", |t| {
                t.column("id", "integer", false)
                    .column("category", "text", false)
                    .column("code", "text", false)
                    .pk(&["id"])
                    .unique("uq_products_cat_code", &["category", "code"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("products"),
            actions: vec![AlterTableAction::DropColumn {
                name: "code".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_drop_column_after_unique_already_dropped_no_finding() {
        // UNIQUE constraint was explicitly removed via DROP CONSTRAINT in an
        // earlier migration. catalog_before reflects no UNIQUE, so DROP COLUMN
        // should NOT warn about silently removing it.
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"]);
                // No UNIQUE constraint — it was dropped by a prior migration
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Should not fire when UNIQUE was already explicitly dropped"
        );
    }

    #[test]
    fn test_drop_pkey_index_column_not_pgm010() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .index("users_pkey", &["id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "the PK's backing index belongs to PGM011, got {findings:?}"
        );
    }

    #[test]
    fn test_drop_named_pk_column_not_pgm010() {
        let before = catalog_from_sql(
            "CREATE TABLE users (id int, email text, CONSTRAINT pk_users PRIMARY KEY (id));",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql("ALTER TABLE users DROP COLUMN id;"));

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "named PK's backing index should not trigger PGM010, got {findings:?}"
        );
    }

    #[test]
    fn test_drop_unnamed_unique_column_of_schema_qualified_table_reported_once() {
        let before = catalog_from_sql("CREATE TABLE analytics.users (id int, email text UNIQUE);");
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql("ALTER TABLE analytics.users DROP COLUMN email;"));

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "got {findings:?}");
    }

    #[test]
    fn test_drop_unnamed_unique_column_names_the_generated_constraint() {
        let before = catalog_from_sql("CREATE TABLE users (id int, email text UNIQUE);");
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql("ALTER TABLE users DROP COLUMN email;"));

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        let messages: Vec<&str> = findings.iter().map(|f| f.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Dropping column 'email' from table 'users' silently removes unique constraint \
              'users_email_key'. Verify that the uniqueness guarantee is no longer needed."
            ]
        );
    }

    #[test]
    fn test_drop_column_of_pk_added_using_index_not_pgm010() {
        let before = catalog_from_sql(
            "CREATE TABLE users (id int NOT NULL);
             CREATE UNIQUE INDEX users_id_idx ON users (id);
             ALTER TABLE users ADD PRIMARY KEY USING INDEX users_id_idx;",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql("ALTER TABLE users DROP COLUMN id;"));

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn test_drop_pk_column_after_table_rename_not_pgm010() {
        let before = catalog_from_sql(
            "CREATE TABLE orders (id int, PRIMARY KEY (id));
             ALTER TABLE orders RENAME TO purchases;",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql("ALTER TABLE purchases DROP COLUMN id;"));

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }
}
