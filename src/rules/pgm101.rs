//! PGM101 — Don't use `timestamp` (without time zone)
//!
//! Detects columns declared as `timestamp` (i.e. `timestamp without time zone`).
//! This type stores no timezone context, making values ambiguous.
//! Use `timestamptz` (timestamp with time zone) instead.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TypeName};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags the use of `timestamp` without time zone.
pub struct Pgm101;

/// Check whether a type name is `timestamp` (without time zone).
fn is_bare_timestamp(tn: &TypeName) -> bool {
    tn.name == "timestamp"
}

impl Rule for Pgm101 {
    fn id(&self) -> &'static str {
        "PGM101"
    }

    fn default_severity(&self) -> Severity {
        Severity::Minor
    }

    fn description(&self) -> &'static str {
        "Column uses timestamp without time zone"
    }

    fn explain(&self) -> &'static str {
        "PGM101 — Don't use `timestamp` (without time zone)\n\
         \n\
         What it detects:\n\
         A column declared as `timestamp` (which PostgreSQL interprets as\n\
         `timestamp without time zone`).\n\
         \n\
         Why it's problematic:\n\
         `timestamp` (without time zone) stores a date/time value with no\n\
         timezone context. This makes the stored values ambiguous — they could\n\
         represent any timezone, and PostgreSQL performs no conversion on\n\
         input or output. When servers, clients, or applications use different\n\
         timezones, this leads to subtle, hard-to-debug data corruption.\n\
         \n\
         `timestamptz` (timestamp with time zone) stores values as UTC\n\
         internally and converts to the session's timezone on output. This\n\
         ensures unambiguous points in time.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE events (created_at timestamp NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE events (created_at timestamptz NOT NULL);"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            match &stmt.node {
                IrNode::CreateTable(ct) => {
                    for col in &ct.columns {
                        if is_bare_timestamp(&col.type_name) {
                            findings.push(Finding {
                                rule_id: self.id().to_string(),
                                severity: self.default_severity(),
                                message: format!(
                                    "Column '{}' on '{}' uses 'timestamp without time zone'. \
                                     Use 'timestamptz' (timestamp with time zone) instead to \
                                     store unambiguous points in time.",
                                    col.name, ct.name,
                                ),
                                file: ctx.file.clone(),
                                start_line: stmt.span.start_line,
                                end_line: stmt.span.end_line,
                            });
                        }
                    }
                }
                IrNode::AlterTable(at) => {
                    for action in &at.actions {
                        match action {
                            AlterTableAction::AddColumn(col) => {
                                if is_bare_timestamp(&col.type_name) {
                                    findings.push(Finding {
                                        rule_id: self.id().to_string(),
                                        severity: self.default_severity(),
                                        message: format!(
                                            "Column '{}' on '{}' uses 'timestamp without time zone'. \
                                             Use 'timestamptz' (timestamp with time zone) instead to \
                                             store unambiguous points in time.",
                                            col.name, at.name,
                                        ),
                                        file: ctx.file.clone(),
                                        start_line: stmt.span.start_line,
                                        end_line: stmt.span.end_line,
                                    });
                                }
                            }
                            AlterTableAction::AlterColumnType {
                                column_name,
                                new_type,
                                ..
                            } => {
                                if is_bare_timestamp(new_type) {
                                    findings.push(Finding {
                                        rule_id: self.id().to_string(),
                                        severity: self.default_severity(),
                                        message: format!(
                                            "Column '{}' on '{}' uses 'timestamp without time zone'. \
                                             Use 'timestamptz' (timestamp with time zone) instead to \
                                             store unambiguous points in time.",
                                            column_name, at.name,
                                        ),
                                        file: ctx.file.clone(),
                                        start_line: stmt.span.start_line,
                                        end_line: stmt.span.end_line,
                                    });
                                }
                            }
                            _ => {}
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

    #[test]
    fn test_create_table_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "created_at".to_string(),
                type_name: TypeName::simple("timestamp"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm101.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM101");
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("created_at"));
        assert!(findings[0].message.contains("events"));
    }

    #[test]
    fn test_timestamptz_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "created_at".to_string(),
                type_name: TypeName::simple("timestamptz"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm101.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "updated_at".to_string(),
                type_name: TypeName::simple("timestamp"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm101.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("updated_at"));
    }

    #[test]
    fn test_alter_column_type_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "created_at".to_string(),
                new_type: TypeName::simple("timestamp"),
                old_type: None,
            }],
        }))];

        let findings = Pgm101.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("created_at"));
    }
}
