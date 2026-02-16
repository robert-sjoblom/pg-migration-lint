//! Matrix interaction tests
//!
//! These tests verify that rules behave correctly in combination — when
//! multiple rules could potentially fire on the same SQL, the correct
//! subset actually fires.

use pg_migration_lint::catalog::Catalog;
use pg_migration_lint::catalog::builder::CatalogBuilder;
use pg_migration_lint::parser::ir::*;
use pg_migration_lint::rules::*;
use std::collections::HashSet;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers (re-created here because test_helpers is #[cfg(test)] crate-internal)
// ---------------------------------------------------------------------------

fn make_ctx<'a>(
    before: &'a Catalog,
    after: &'a Catalog,
    file: &'a std::path::Path,
    created: &'a HashSet<String>,
) -> LintContext<'a> {
    LintContext {
        catalog_before: before,
        catalog_after: after,
        tables_created_in_change: created,
        run_in_transaction: true,
        is_down: false,
        file,
    }
}

fn located(node: IrNode) -> Located<IrNode> {
    Located {
        node,
        span: SourceSpan {
            start_line: 1,
            end_line: 1,
            start_offset: 0,
            end_offset: 0,
        },
    }
}

/// Run selected rules by ID and return findings sorted by rule_id.
fn run_selected_rules(
    stmts: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    rule_ids: &[&str],
) -> Vec<Finding> {
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let mut findings = Vec::new();
    for rule in registry.iter() {
        if rule_ids.contains(&rule.id().as_str()) {
            findings.extend(rule.check(stmts, ctx));
        }
    }
    findings.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    findings
}

// ---------------------------------------------------------------------------
// (a) ADD COLUMN NOT NULL with volatile default -> PGM007 fires, PGM010 does not
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_column_not_null_with_volatile_default() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::AddColumn(ColumnDef {
            name: "created_at".to_string(),
            type_name: TypeName::simple("timestamptz"),
            nullable: false,
            default_expr: Some(DefaultExpr::FunctionCall {
                name: "now".to_string(),
                args: vec![],
            }),
            is_inline_pk: false,
            is_serial: false,
        })],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM007", "PGM010"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (b) ADD COLUMN NOT NULL without default -> PGM010 fires, PGM007 does not
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_column_not_null_without_default() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::AddColumn(ColumnDef {
            name: "status".to_string(),
            type_name: TypeName::simple("text"),
            nullable: false,
            default_expr: None,
            is_inline_pk: false,
            is_serial: false,
        })],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM007", "PGM010"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (c) CREATE INDEX CONCURRENTLY in transaction -> PGM006 fires, PGM001 does not
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_create_index_concurrent_in_transaction() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    // run_in_transaction = true (default from make_ctx)
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::CreateIndex(CreateIndex {
        index_name: Some("idx_orders_status".to_string()),
        table_name: QualifiedName::unqualified("orders"),
        columns: vec![IndexColumn {
            name: "status".to_string(),
        }],
        unique: false,
        concurrent: true,
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM001", "PGM006"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (d) CREATE INDEX (non-concurrent) in transaction -> PGM001 fires, PGM006 does not
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_create_index_non_concurrent_in_transaction() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::CreateIndex(CreateIndex {
        index_name: Some("idx_orders_status".to_string()),
        table_name: QualifiedName::unqualified("orders"),
        columns: vec![IndexColumn {
            name: "status".to_string(),
        }],
        unique: false,
        concurrent: false,
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM001", "PGM006"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (e) CREATE TABLE no PK with FK no covering index -> PGM003 and PGM004 both fire
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_create_table_no_pk_with_fk_no_index() {
    let before = Catalog::new();
    // catalog_after: table exists with FK but no PK and no covering index
    let after = CatalogBuilder::new()
        .table("customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("orders", |t| {
            t.column("order_id", "integer", false)
                .column("customer_id", "integer", false)
                .fk("fk_customer", &["customer_id"], "customers", &["id"]);
            // No pk, no index on customer_id
        })
        .build();
    let file = PathBuf::from("migrations/001.sql");
    let mut created = HashSet::new();
    created.insert("orders".to_string());
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::CreateTable(CreateTable {
        name: QualifiedName::unqualified("orders"),
        columns: vec![
            ColumnDef {
                name: "order_id".to_string(),
                type_name: TypeName::simple("integer"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
            ColumnDef {
                name: "customer_id".to_string(),
                type_name: TypeName::simple("integer"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
        ],
        constraints: vec![TableConstraint::ForeignKey {
            name: Some("fk_customer".to_string()),
            columns: vec!["customer_id".to_string()],
            ref_table: QualifiedName::unqualified("customers"),
            ref_columns: vec!["id".to_string()],
            not_valid: false,
        }],
        temporary: false,
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM003", "PGM004"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (f) CREATE TABLE no PK but UNIQUE NOT NULL -> PGM005 fires, PGM004 does not
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_create_table_no_pk_but_unique_not_null() {
    let before = Catalog::new();
    let after = CatalogBuilder::new()
        .table("users", |t| {
            t.column("email", "text", false)
                .column("name", "text", true)
                .unique("uk_email", &["email"]);
        })
        .build();
    let file = PathBuf::from("migrations/001.sql");
    let mut created = HashSet::new();
    created.insert("users".to_string());
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::CreateTable(CreateTable {
        name: QualifiedName::unqualified("users"),
        columns: vec![
            ColumnDef {
                name: "email".to_string(),
                type_name: TypeName::simple("text"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
            ColumnDef {
                name: "name".to_string(),
                type_name: TypeName::simple("text"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
        ],
        constraints: vec![TableConstraint::Unique {
            name: Some("uk_email".to_string()),
            columns: vec!["email".to_string()],
        }],
        temporary: false,
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM004", "PGM005"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (g) ADD FK without NOT VALID, no covering index -> PGM003 and PGM017 both fire
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_fk_without_not_valid_no_index() {
    let before = CatalogBuilder::new()
        .table("customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("orders", |t| {
            t.column("id", "integer", false)
                .column("customer_id", "integer", false)
                .pk(&["id"]);
        })
        .build();
    // After: FK added but no covering index
    let after = CatalogBuilder::new()
        .table("customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("orders", |t| {
            t.column("id", "integer", false)
                .column("customer_id", "integer", false)
                .pk(&["id"])
                .fk("fk_customer", &["customer_id"], "customers", &["id"]);
        })
        .build();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::AddConstraint(
            TableConstraint::ForeignKey {
                name: Some("fk_customer".to_string()),
                columns: vec!["customer_id".to_string()],
                ref_table: QualifiedName::unqualified("customers"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            },
        )],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM003", "PGM017"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (h) ADD FK NOT VALID, no covering index -> PGM003 fires, PGM017 does not
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_fk_not_valid_no_index() {
    let before = CatalogBuilder::new()
        .table("customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("orders", |t| {
            t.column("id", "integer", false)
                .column("customer_id", "integer", false)
                .pk(&["id"]);
        })
        .build();
    // After: FK added (NOT VALID) but no covering index
    let after = CatalogBuilder::new()
        .table("customers", |t| {
            t.column("id", "integer", false).pk(&["id"]);
        })
        .table("orders", |t| {
            t.column("id", "integer", false)
                .column("customer_id", "integer", false)
                .pk(&["id"])
                .fk("fk_customer", &["customer_id"], "customers", &["id"]);
        })
        .build();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::AddConstraint(
            TableConstraint::ForeignKey {
                name: Some("fk_customer".to_string()),
                columns: vec!["customer_id".to_string()],
                ref_table: QualifiedName::unqualified("customers"),
                ref_columns: vec!["id".to_string()],
                not_valid: true,
            },
        )],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM003", "PGM017"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (i) ADD CHECK without NOT VALID on existing table -> PGM018 fires
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_check_without_not_valid() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false)
                .column("status", "text", true)
                .pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
            name: Some("orders_status_check".to_string()),
            expression: "status IN ('pending', 'shipped', 'delivered')".to_string(),
            not_valid: false,
        })],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM018"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (j) SET NOT NULL on existing table -> PGM016 fires
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_set_not_null_on_existing() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "integer", false)
                .column("status", "text", true)
                .pk(&["id"]);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::SetNotNull {
            column_name: "status".to_string(),
        }],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM016"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (k) Down migration caps severity to Info
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_down_migration_caps_severity() {
    let mut findings = vec![
        Finding {
            rule_id: RuleId::Migration(MigrationRule::Pgm001),
            severity: Severity::Critical,
            message: "Missing CONCURRENTLY on CREATE INDEX".to_string(),
            file: PathBuf::from("migrations/002.down.sql"),
            start_line: 1,
            end_line: 1,
        },
        Finding {
            rule_id: RuleId::Migration(MigrationRule::Pgm003),
            severity: Severity::Major,
            message: "FK without covering index".to_string(),
            file: PathBuf::from("migrations/002.down.sql"),
            start_line: 3,
            end_line: 3,
        },
        Finding {
            rule_id: RuleId::Migration(MigrationRule::Pgm007),
            severity: Severity::Minor,
            message: "Volatile default on column".to_string(),
            file: PathBuf::from("migrations/002.down.sql"),
            start_line: 5,
            end_line: 5,
        },
    ];

    cap_for_down_migration(&mut findings);

    // Sort by rule_id for stable snapshot
    findings.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (l) CREATE TABLE with bad column types -> PGM101, PGM103, PGM104, PGM105
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_create_table_with_bad_types() {
    let before = Catalog::new();
    let after = Catalog::new();
    let file = PathBuf::from("migrations/001.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::CreateTable(CreateTable {
        name: QualifiedName::unqualified("bad_types"),
        columns: vec![
            // PGM105: serial column
            ColumnDef {
                name: "id".to_string(),
                type_name: TypeName::simple("int4"),
                nullable: false,
                default_expr: Some(DefaultExpr::FunctionCall {
                    name: "nextval".to_string(),
                    args: vec![],
                }),
                is_inline_pk: true,
                is_serial: true,
            },
            // PGM101: timestamp without time zone
            ColumnDef {
                name: "created_at".to_string(),
                type_name: TypeName::simple("timestamp"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
            // PGM103: char(10)
            ColumnDef {
                name: "code".to_string(),
                type_name: TypeName::with_modifiers("bpchar", vec![10]),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
            // PGM104: money
            ColumnDef {
                name: "price".to_string(),
                type_name: TypeName::simple("money"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            },
        ],
        constraints: vec![],
        temporary: false,
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM101", "PGM103", "PGM104", "PGM105"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (m) ADD PRIMARY KEY fires PGM012 only, not PGM021
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_pk_fires_pgm012_not_pgm021() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "bigint", false)
                .column("email", "text", false);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![AlterTableAction::AddConstraint(
            TableConstraint::PrimaryKey {
                columns: vec!["id".to_string()],
            },
        )],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM012", "PGM021"]);
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// (n) Multi-action ALTER TABLE: ADD PK + ADD UNIQUE fires both PGM012 and PGM021
// ---------------------------------------------------------------------------
#[test]
fn test_matrix_add_pk_and_add_unique_both_fire() {
    let before = CatalogBuilder::new()
        .table("orders", |t| {
            t.column("id", "bigint", false)
                .column("email", "text", false);
        })
        .build();
    let after = before.clone();
    let file = PathBuf::from("migrations/002.sql");
    let created = HashSet::new();
    let ctx = make_ctx(&before, &after, &file, &created);

    let stmts = vec![located(IrNode::AlterTable(AlterTable {
        name: QualifiedName::unqualified("orders"),
        actions: vec![
            AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                columns: vec!["id".to_string()],
            }),
            AlterTableAction::AddConstraint(TableConstraint::Unique {
                name: Some("uq_orders_email".to_string()),
                columns: vec!["email".to_string()],
            }),
        ],
    }))];

    let findings = run_selected_rules(&stmts, &ctx, &["PGM012", "PGM021"]);
    insta::assert_yaml_snapshot!(findings);
}
