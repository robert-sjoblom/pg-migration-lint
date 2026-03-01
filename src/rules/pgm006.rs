//! PGM006 — Volatile default on column
//!
//! Detects `ADD COLUMN` on an existing table that uses a function call as the
//! DEFAULT expression. Known volatile functions produce a WARNING; `nextval`
//! gets a serial-specific message; unknown functions produce an INFO suggesting
//! the developer verify volatility.
//!
//! Also detects `ALTER COLUMN SET DEFAULT` with volatile functions (INFO level).
//! Unlike `ADD COLUMN`, `SET DEFAULT` does not cause a table rewrite — it only
//! affects future inserts. The finding warns that existing rows are NOT backfilled.
//!
//! `CREATE TABLE` and `ADD COLUMN` on tables created in the same changeset are
//! exempt — there are no existing rows, so no table rewrite occurs.

use crate::parser::ir::{
    AlterTableAction, ColumnDef, DefaultExpr, IrNode, Located, QualifiedName, SourceSpan,
};
use crate::rules::fn_volatility::{self, FnVolatility};
use crate::rules::{Finding, LintContext, Rule, Severity};
use std::path::Path;

pub(super) const DESCRIPTION: &str = "Volatile default on column";

pub(super) const EXPLAIN: &str = "PGM006 — Volatile default on column\n\
         \n\
         What it detects:\n\
         A column definition (in ALTER TABLE ... ADD COLUMN) that uses a\n\
         volatile function call as the DEFAULT expression on an existing table.\n\
         \n\
         Why it's dangerous:\n\
         On PostgreSQL 11+, non-volatile defaults on ADD COLUMN don't rewrite\n\
         the table — they are applied lazily. Volatile defaults (random(),\n\
         gen_random_uuid(), clock_timestamp(), etc.) must be evaluated per-row\n\
         at write time, forcing a full table rewrite under an ACCESS EXCLUSIVE\n\
         lock.\n\
         \n\
         Note: now() and current_timestamp are STABLE in PostgreSQL, not\n\
         volatile. They return the transaction start time and are evaluated\n\
         once at ALTER TABLE time. The resulting value is stored in the\n\
         catalog and applied lazily — no table rewrite occurs.\n\
         \n\
         Volatility classification is derived from PostgreSQL's pg_proc catalog\n\
         covering all ~2700 built-in functions.\n\
         \n\
         Severity levels:\n\
         - MINOR (WARNING): Volatile built-in functions (ADD COLUMN)\n\
         - MINOR (WARNING): nextval (serial/bigserial) — standard but volatile (ADD COLUMN)\n\
         - INFO: SET DEFAULT with volatile or unrecognized function\n\
         - INFO: Unrecognized functions on ADD COLUMN — developer should verify volatility\n\
         - No finding: Literal defaults, stable/immutable functions\n\
         \n\
         Example (flagged — ADD COLUMN):\n\
           ALTER TABLE orders ADD COLUMN token uuid DEFAULT gen_random_uuid();\n\
         \n\
         Fix:\n\
           ALTER TABLE orders ADD COLUMN token uuid;\n\
           -- Then backfill:\n\
           UPDATE orders SET token = gen_random_uuid() WHERE token IS NULL;\n\
         \n\
         Also detects SET DEFAULT with volatile functions (INFO):\n\
           ALTER TABLE orders ALTER COLUMN token SET DEFAULT gen_random_uuid();\n\
         Unlike ADD COLUMN, SET DEFAULT does NOT cause a table rewrite — it only\n\
         affects future INSERTs. Existing rows are NOT backfilled.\n\
         \n\
         Note: For CREATE TABLE, volatile defaults are harmless (no existing\n\
         rows) and are not flagged.";

/// Returns true if the function name is `nextval` (case-insensitive).
/// Used to emit a serial-specific message instead of the generic volatile warning.
fn is_nextval(func_name: &str) -> bool {
    func_name.eq_ignore_ascii_case("nextval")
}

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        // Only check ALTER TABLE … ADD COLUMN and ALTER COLUMN SET DEFAULT.
        // CREATE TABLE is intentionally skipped — no existing rows means no rewrite.
        if let IrNode::AlterTable(at) = &stmt.node {
            // Skip tables created in this changeset — no existing rows to rewrite.
            let table_key = at.name.catalog_key();
            if ctx.tables_created_in_change.contains(table_key) {
                continue;
            }

            for action in &at.actions {
                match action {
                    AlterTableAction::AddColumn(col) => {
                        if let Some(finding) =
                            check_column(col, &at.name, &rule, ctx.file, &stmt.span)
                        {
                            findings.push(finding);
                        }
                    }
                    AlterTableAction::SetDefault {
                        column_name,
                        default_expr,
                    } => {
                        if let Some(finding) = check_set_default(
                            column_name,
                            default_expr,
                            &at.name,
                            &rule,
                            ctx.file,
                            &stmt.span,
                        ) {
                            findings.push(finding);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    findings
}

/// Check a single column definition for a volatile default.
fn check_column(
    col: &ColumnDef,
    table_name: &QualifiedName,
    rule: &impl Rule,
    file: &Path,
    span: &SourceSpan,
) -> Option<Finding> {
    let func_name = match &col.default_expr {
        Some(DefaultExpr::FunctionCall { name, .. }) => name,
        Some(DefaultExpr::Literal(_)) | Some(DefaultExpr::Other(_)) | None => return None,
    };

    match fn_volatility::lookup(func_name) {
        Some(FnVolatility::Stable | FnVolatility::Immutable) => None,
        Some(FnVolatility::Volatile) if is_nextval(func_name) => Some(Finding::new(
            rule.id(),
            Severity::Minor,
            format!(
                "Column '{col}' on '{table}' uses a sequence default (serial/bigserial). \
                 This is standard usage — suppress if intentional. Note: on ADD COLUMN \
                 to an existing table, this is volatile and forces a table rewrite.",
                col = col.name,
                table = table_name.display_name(),
            ),
            file,
            span,
        )),
        Some(FnVolatility::Volatile) => Some(Finding::new(
            rule.id(),
            Severity::Minor,
            format!(
                "Column '{col}' on '{table}' uses '{fn_name}()' as default (known volatile). \
                 Unlike non-volatile defaults, this forces a full table rewrite under an \
                 ACCESS EXCLUSIVE lock \u{2014} every existing row must be physically updated \
                 with a computed value. For large tables, this causes extended downtime. \
                 Consider adding the column without a default, then backfilling with \
                 batched UPDATEs.",
                col = col.name,
                table = table_name.display_name(),
                fn_name = func_name,
            ),
            file,
            span,
        )),
        None => Some(Finding::new(
            rule.id(),
            Severity::Info,
            format!(
                "Column '{col}' on '{table}' uses function '{fn_name}()' as default. \
                 If this function is volatile (the default for user-defined functions), \
                 it forces a full table rewrite under an ACCESS EXCLUSIVE lock instead \
                 of a cheap catalog-only change. Verify the function's volatility classification.",
                col = col.name,
                table = table_name,
                fn_name = func_name,
            ),
            file,
            span,
        )),
    }
}

/// Check a `SET DEFAULT` action for a volatile function default.
///
/// Unlike `ADD COLUMN`, `SET DEFAULT` does NOT cause a table rewrite — it only
/// changes the catalog entry for future inserts. However, volatile defaults on
/// `SET DEFAULT` are unusual and may indicate the developer expects existing rows
/// to be backfilled (they won't be). Emits INFO severity.
fn check_set_default(
    column_name: &str,
    default_expr: &DefaultExpr,
    table_name: &QualifiedName,
    rule: &impl Rule,
    file: &Path,
    span: &SourceSpan,
) -> Option<Finding> {
    let func_name = match default_expr {
        DefaultExpr::FunctionCall { name, .. } => name,
        DefaultExpr::Literal(_) | DefaultExpr::Other(_) => return None,
    };

    match fn_volatility::lookup(func_name) {
        Some(FnVolatility::Stable | FnVolatility::Immutable) => None,
        Some(FnVolatility::Volatile) if is_nextval(func_name) => Some(Finding::new(
            rule.id(),
            Severity::Info,
            format!(
                "SET DEFAULT 'nextval()' on column '{col}' of '{table}' (serial/bigserial). \
                 This is standard usage — suppress if intentional. Note: SET DEFAULT only \
                 affects future INSERTs — existing rows are NOT backfilled.",
                col = column_name,
                table = table_name.display_name(),
            ),
            file,
            span,
        )),
        Some(FnVolatility::Volatile) => Some(Finding::new(
            rule.id(),
            Severity::Info,
            format!(
                "SET DEFAULT '{fn_name}()' on column '{col}' of '{table}' (known volatile). \
                 Note: SET DEFAULT only affects future INSERTs — existing rows are NOT \
                 backfilled. If you need to populate existing rows, use a batched UPDATE.",
                fn_name = func_name,
                col = column_name,
                table = table_name.display_name(),
            ),
            file,
            span,
        )),
        None => Some(Finding::new(
            rule.id(),
            Severity::Info,
            format!(
                "SET DEFAULT '{fn_name}()' on column '{col}' of '{table}' uses a function \
                 default. If this function is volatile, note that SET DEFAULT only affects \
                 future INSERTs — existing rows are NOT backfilled.",
                fn_name = func_name,
                col = column_name,
                table = table_name.display_name(),
            ),
            file,
            span,
        )),
    }
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

    fn col_with_default(name: &str, default: DefaultExpr) -> ColumnDef {
        ColumnDef::test(name, "timestamptz").with_default(default)
    }

    #[test]
    fn test_now_default_no_finding() {
        // now() is STABLE in PostgreSQL — evaluated once at ALTER TABLE time.
        // No table rewrite occurs on PG 11+, so no finding should be emitted.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "created_at",
                DefaultExpr::FunctionCall {
                    name: "now".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "now() is STABLE, should not trigger PGM006"
        );
    }

    #[test]
    fn test_clock_timestamp_fires_warning() {
        // clock_timestamp() is truly volatile — changes between calls.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "created_at",
                DefaultExpr::FunctionCall {
                    name: "clock_timestamp".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "clock_timestamp is volatile");
        assert_eq!(findings[0].severity, Severity::Minor);
    }

    #[test]
    fn test_create_table_with_volatile_default_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("tokens")).with_columns(vec![
                col_with_default(
                    "id",
                    DefaultExpr::FunctionCall {
                        name: "gen_random_uuid".to_string(),
                        args: vec![],
                    },
                ),
            ]),
        ))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "CREATE TABLE should not trigger PGM006"
        );
    }

    #[test]
    fn test_add_column_on_new_in_same_changeset_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created: HashSet<String> = ["orders".to_string()].into_iter().collect();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "created_at",
                DefaultExpr::FunctionCall {
                    name: "now".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "ADD COLUMN on table created in same changeset should not trigger PGM006"
        );
    }

    #[test]
    fn test_literal_default_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(
                ColumnDef::test("status", "text")
                    .with_default(DefaultExpr::Literal("active".to_string())),
            )],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_unknown_function_fires_info() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "computed",
                DefaultExpr::FunctionCall {
                    name: "my_custom_func".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_nextval_fires_warning() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "id",
                DefaultExpr::FunctionCall {
                    name: "nextval".to_string(),
                    args: vec!["orders_id_seq".to_string()],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_default_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test("note", "text"))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_mixed_case_stable_no_finding() {
        // Verify case-insensitive matching: "NOW" should be classified as STABLE.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "created_at",
                DefaultExpr::FunctionCall {
                    name: "NOW".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "NOW (uppercase) is STABLE, should not trigger PGM006"
        );
    }

    #[test]
    fn test_mixed_case_volatile_fires_warning() {
        // Verify case-insensitive matching: "Gen_Random_Uuid" should be KnownVolatile.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "token",
                DefaultExpr::FunctionCall {
                    name: "Gen_Random_Uuid".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Gen_Random_Uuid (mixed case) is volatile"
        );
        assert_eq!(findings[0].severity, Severity::Minor);
    }

    #[test]
    fn test_txid_current_is_stable_no_finding() {
        // txid_current() is STABLE in PostgreSQL (provolatile = 's').
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(col_with_default(
                "txid",
                DefaultExpr::FunctionCall {
                    name: "txid_current".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "txid_current() is STABLE, should not trigger PGM006"
        );
    }

    // -----------------------------------------------------------------------
    // SET DEFAULT — volatile function detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_default_volatile_fires_info() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false).column("token", "uuid", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetDefault {
                column_name: "token".to_string(),
                default_expr: DefaultExpr::FunctionCall {
                    name: "gen_random_uuid".to_string(),
                    args: vec![],
                },
            }],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "volatile SET DEFAULT should fire");
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("NOT backfilled"));
    }

    #[test]
    fn test_set_default_stable_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false)
                    .column("created_at", "timestamptz", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetDefault {
                column_name: "created_at".to_string(),
                default_expr: DefaultExpr::FunctionCall {
                    name: "now".to_string(),
                    args: vec![],
                },
            }],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "now() is STABLE, SET DEFAULT should not trigger PGM006"
        );
    }

    #[test]
    fn test_set_default_literal_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false).column("status", "text", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetDefault {
                column_name: "status".to_string(),
                default_expr: DefaultExpr::Literal("active".to_string()),
            }],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(findings.is_empty(), "Literal SET DEFAULT should not fire");
    }

    #[test]
    fn test_set_default_nextval_fires_info() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false).column("seq_col", "int", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetDefault {
                column_name: "seq_col".to_string(),
                default_expr: DefaultExpr::FunctionCall {
                    name: "nextval".to_string(),
                    args: vec!["orders_seq_col_seq".to_string()],
                },
            }],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "nextval SET DEFAULT should fire");
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("serial/bigserial"));
        assert!(findings[0].message.contains("NOT backfilled"));
    }

    #[test]
    fn test_set_default_unknown_function_fires_info() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int", false)
                    .column("computed", "text", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetDefault {
                column_name: "computed".to_string(),
                default_expr: DefaultExpr::FunctionCall {
                    name: "my_custom_func".to_string(),
                    args: vec![],
                },
            }],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "unknown function SET DEFAULT should fire"
        );
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("NOT backfilled"));
    }

    #[test]
    fn test_set_default_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created: HashSet<String> = ["orders".to_string()].into_iter().collect();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetDefault {
                column_name: "token".to_string(),
                default_expr: DefaultExpr::FunctionCall {
                    name: "gen_random_uuid".to_string(),
                    args: vec![],
                },
            }],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "SET DEFAULT on table created in same changeset should not fire"
        );
    }
}
