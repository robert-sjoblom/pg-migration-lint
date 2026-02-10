//! PGM007 — Volatile default on column
//!
//! Detects column definitions (in `CREATE TABLE` or `ADD COLUMN`) that use
//! a function call as the DEFAULT expression. Known volatile functions produce
//! a WARNING; `nextval` gets a serial-specific message; unknown functions
//! produce an INFO suggesting the developer verify volatility.

use crate::parser::ir::{AlterTableAction, ColumnDef, DefaultExpr, IrNode, Located, QualifiedName};
use crate::rules::{Finding, LintContext, Rule, Severity};
use std::path::Path;

/// Rule that flags volatile function defaults on columns.
pub struct Pgm007;

/// Known volatile functions that always force a table rewrite when used as
/// a column default on `ADD COLUMN` to an existing table.
const KNOWN_VOLATILE: &[&str] = &[
    "now",
    "current_timestamp",
    "random",
    "gen_random_uuid",
    "uuid_generate_v4",
    "clock_timestamp",
    "timeofday",
    "txid_current",
];

/// Check a single column definition for a volatile default.
fn check_column(
    col: &ColumnDef,
    table_name: &QualifiedName,
    rule: &Pgm007,
    file: &Path,
    start_line: usize,
    end_line: usize,
) -> Option<Finding> {
    let func_name = match &col.default_expr {
        Some(DefaultExpr::FunctionCall { name, .. }) => name,
        Some(DefaultExpr::Literal(_)) | Some(DefaultExpr::Other(_)) | None => return None,
    };

    let lower = func_name.to_lowercase();

    if lower == "nextval" {
        return Some(Finding {
            rule_id: rule.id().to_string(),
            severity: Severity::Minor, // WARNING level — standard but volatile
            message: format!(
                "Column '{col}' on '{table}' uses a sequence default (serial/bigserial). \
                 This is standard usage — suppress if intentional. Note: on ADD COLUMN \
                 to an existing table, this is volatile and forces a table rewrite.",
                col = col.name,
                table = table_name,
            ),
            file: file.to_path_buf(),
            start_line,
            end_line,
        });
    }

    if KNOWN_VOLATILE.contains(&lower.as_str()) {
        return Some(Finding {
            rule_id: rule.id().to_string(),
            severity: Severity::Minor, // WARNING level — known volatile
            message: format!(
                "Column '{col}' on '{table}' uses volatile default '{fn_name}()'. \
                 Unlike non-volatile defaults, this forces a full table rewrite under an \
                 ACCESS EXCLUSIVE lock \u{2014} every existing row must be physically updated \
                 with a computed value. For large tables, this causes extended downtime. \
                 Consider adding the column without a default, then backfilling with \
                 batched UPDATEs.",
                col = col.name,
                table = table_name,
                fn_name = func_name,
            ),
            file: file.to_path_buf(),
            start_line,
            end_line,
        });
    }

    // Unknown function — INFO level.
    Some(Finding {
        rule_id: rule.id().to_string(),
        severity: Severity::Info,
        message: format!(
            "Column '{col}' on '{table}' uses function '{fn_name}()' as default. \
             If this function is volatile (the default for user-defined functions), \
             it forces a full table rewrite under an ACCESS EXCLUSIVE lock instead \
             of a cheap catalog-only change. Verify the function's volatility classification.",
            col = col.name,
            table = table_name,
            fn_name = func_name,
        ),
        file: file.to_path_buf(),
        start_line,
        end_line,
    })
}

impl Rule for Pgm007 {
    fn id(&self) -> &'static str {
        "PGM007"
    }

    fn default_severity(&self) -> Severity {
        // The actual severity varies per finding; this is the "worst case" default.
        // WARNING maps to Minor in the Severity enum.
        Severity::Minor
    }

    fn description(&self) -> &'static str {
        "Volatile default on column"
    }

    fn explain(&self) -> &'static str {
        "PGM007 — Volatile default on column\n\
         \n\
         What it detects:\n\
         A column definition (in CREATE TABLE or ALTER TABLE ... ADD COLUMN)\n\
         that uses a function call as the DEFAULT expression.\n\
         \n\
         Why it's dangerous:\n\
         On PostgreSQL 11+, non-volatile defaults on ADD COLUMN don't rewrite\n\
         the table — they are applied lazily. Volatile defaults (now(), random(),\n\
         gen_random_uuid(), etc.) must be evaluated per-row at write time,\n\
         forcing a full table rewrite under an ACCESS EXCLUSIVE lock.\n\
         \n\
         Severity levels:\n\
         - MINOR (WARNING): Known volatile functions (now, current_timestamp,\n\
           random, gen_random_uuid, uuid_generate_v4, clock_timestamp,\n\
           timeofday, txid_current)\n\
         - MINOR (WARNING): nextval (serial/bigserial) — standard but volatile\n\
         - INFO: Unknown function calls — developer should verify volatility\n\
         - No finding: Literal defaults (0, 'active', TRUE)\n\
         \n\
         Example (flagged):\n\
           ALTER TABLE orders ADD COLUMN created_at timestamptz DEFAULT now();\n\
         \n\
         Fix:\n\
           ALTER TABLE orders ADD COLUMN created_at timestamptz;\n\
           -- Then backfill:\n\
           UPDATE orders SET created_at = now() WHERE created_at IS NULL;\n\
         \n\
         Note: For CREATE TABLE, volatile defaults are harmless (no existing\n\
         rows). The rule still flags them for awareness."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            match &stmt.node {
                IrNode::CreateTable(ct) => {
                    for col in &ct.columns {
                        if let Some(finding) = check_column(
                            col,
                            &ct.name,
                            self,
                            ctx.file,
                            stmt.span.start_line,
                            stmt.span.end_line,
                        ) {
                            findings.push(finding);
                        }
                    }
                }
                IrNode::AlterTable(at) => {
                    for action in &at.actions {
                        if let AlterTableAction::AddColumn(col) = action
                            && let Some(finding) = check_column(
                                col,
                                &at.name,
                                self,
                                ctx.file,
                                stmt.span.start_line,
                                stmt.span.end_line,
                            )
                        {
                            findings.push(finding);
                        }
                    }
                }
                _ => {}
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn make_ctx<'a>(
        before: &'a Catalog,
        after: &'a Catalog,
        file: &'a PathBuf,
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

    fn col_with_default(name: &str, default: DefaultExpr) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            type_name: TypeName::simple("timestamptz"),
            nullable: true,
            default_expr: Some(default),
            is_inline_pk: false,
            is_serial: false,
        }
    }

    #[test]
    fn test_now_default_fires_warning() {
        let before = Catalog::new();
        let after = Catalog::new();
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

        let findings = Pgm007.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("now()"));
        assert!(findings[0].message.contains("volatile"));
    }

    #[test]
    fn test_gen_random_uuid_fires_warning() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("tokens"),
            columns: vec![col_with_default(
                "id",
                DefaultExpr::FunctionCall {
                    name: "gen_random_uuid".to_string(),
                    args: vec![],
                },
            )],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm007.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("gen_random_uuid()"));
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
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "status".to_string(),
                type_name: TypeName::simple("text"),
                nullable: true,
                default_expr: Some(DefaultExpr::Literal("active".to_string())),
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm007.check(&stmts, &ctx);
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

        let findings = Pgm007.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("my_custom_func()"));
        assert!(findings[0].message.contains("volatility"));
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

        let findings = Pgm007.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("serial/bigserial"));
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
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "note".to_string(),
                type_name: TypeName::simple("text"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm007.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
