//! PGM006 — Volatile default on column
//!
//! Detects `ADD COLUMN` on an existing table that uses a function call as the
//! DEFAULT expression. Known volatile functions produce a WARNING; `nextval`
//! gets a serial-specific message; unknown functions produce an INFO suggesting
//! the developer verify volatility.
//!
//! `CREATE TABLE` and `ADD COLUMN` on tables created in the same changeset are
//! exempt — there are no existing rows, so no table rewrite occurs.

use crate::parser::ir::{
    AlterTableAction, ColumnDef, DefaultExpr, IrNode, Located, QualifiedName, SourceSpan,
};
use crate::rules::{Finding, LintContext, Rule, Severity};
use std::path::Path;

/// Known volatile functions that always force a table rewrite when used as
/// a column default on `ADD COLUMN` to an existing table.
const KNOWN_VOLATILE: &[&str] = &[
    "random",
    "gen_random_uuid",
    "uuid_generate_v4",
    "clock_timestamp",
    "timeofday",
    "txid_current",
];

/// Known stable functions that do NOT force a table rewrite on `ADD COLUMN`
/// (PostgreSQL 11+). These are evaluated once at ALTER TABLE time and the
/// single resulting value is stored in the catalog, applied lazily to existing
/// rows. No finding is emitted for these.
const KNOWN_STABLE: &[&str] = &[
    "now",
    "current_timestamp",
    "statement_timestamp",
    "transaction_timestamp",
    "current_date",
    "current_time",
    "localtime",
    "localtimestamp",
];

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
         Severity levels:\n\
         - MINOR (WARNING): Known volatile functions (random, gen_random_uuid,\n\
           uuid_generate_v4, clock_timestamp, timeofday, txid_current)\n\
         - MINOR (WARNING): nextval (serial/bigserial) — standard but volatile\n\
         - INFO: Unknown function calls — developer should verify volatility\n\
         - No finding: Literal defaults, stable functions (now, current_timestamp)\n\
         \n\
         Example (flagged):\n\
           ALTER TABLE orders ADD COLUMN token uuid DEFAULT gen_random_uuid();\n\
         \n\
         Fix:\n\
           ALTER TABLE orders ADD COLUMN token uuid;\n\
           -- Then backfill:\n\
           UPDATE orders SET token = gen_random_uuid() WHERE token IS NULL;\n\
         \n\
         Note: For CREATE TABLE, volatile defaults are harmless (no existing\n\
         rows) and are not flagged.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        // Only check ALTER TABLE … ADD COLUMN.
        // CREATE TABLE is intentionally skipped — no existing rows means no rewrite.
        if let IrNode::AlterTable(at) = &stmt.node {
            // Skip tables created in this changeset — no existing rows to rewrite.
            let table_key = at.name.catalog_key();
            if ctx.tables_created_in_change.contains(table_key) {
                continue;
            }

            for action in &at.actions {
                if let AlterTableAction::AddColumn(col) = action
                    && let Some(finding) = check_column(col, &at.name, &rule, ctx.file, &stmt.span)
                {
                    findings.push(finding);
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

    let lower = func_name.to_lowercase();

    if lower == "nextval" {
        return Some(Finding::new(
            rule.id(),
            Severity::Minor, // WARNING level — standard but volatile
            format!(
                "Column '{col}' on '{table}' uses a sequence default (serial/bigserial). \
                 This is standard usage — suppress if intentional. Note: on ADD COLUMN \
                 to an existing table, this is volatile and forces a table rewrite.",
                col = col.name,
                table = table_name.display_name(),
            ),
            file,
            span,
        ));
    }

    // Stable functions (e.g., now(), current_timestamp) are evaluated once
    // at ALTER TABLE time and the single value is stored in the catalog.
    // No table rewrite occurs on PostgreSQL 11+.
    if KNOWN_STABLE.contains(&lower.as_str()) {
        return None;
    }

    if KNOWN_VOLATILE.contains(&lower.as_str()) {
        return Some(Finding::new(
            rule.id(),
            Severity::Minor, // WARNING level — known volatile
            format!(
                "Column '{col}' on '{table}' uses volatile default '{fn_name}()'. \
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
        ));
    }

    // Unknown function — INFO level.
    Some(Finding::new(
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
    ))
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
    fn test_current_timestamp_default_no_finding() {
        // current_timestamp is STABLE — same behavior as now().
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
                    name: "current_timestamp".to_string(),
                    args: vec![],
                },
            ))],
        }))];

        let findings = RuleId::Pgm006.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "current_timestamp is STABLE, should not trigger PGM006"
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
}
