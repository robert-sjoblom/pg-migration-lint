//! PGM104 — Don't use `money` type
//!
//! Detects columns declared as `money`. The money type depends on the
//! `lc_monetary` locale setting, making it unreliable across environments.
//! Use `numeric(p,s)` instead.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TypeName};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags the use of the `money` type.
pub struct Pgm104;

/// Check whether a type name is `money`.
fn is_money(tn: &TypeName) -> bool {
    tn.name == "money"
}

impl Rule for Pgm104 {
    fn id(&self) -> &'static str {
        "PGM104"
    }

    fn default_severity(&self) -> Severity {
        Severity::Minor
    }

    fn description(&self) -> &'static str {
        "Column uses the money type"
    }

    fn explain(&self) -> &'static str {
        "PGM104 — Don't use `money` type\n\
         \n\
         What it detects:\n\
         A column declared as `money`.\n\
         \n\
         Why it's problematic:\n\
         The `money` type formats its output (and parses input) according\n\
         to the `lc_monetary` locale setting on the PostgreSQL server. This\n\
         means the same stored value can appear differently on different\n\
         servers, and importing/exporting data between servers with different\n\
         locale settings can corrupt values. It also has limited precision\n\
         (fixed to the locale's currency format) and poor interoperability\n\
         with other numeric types.\n\
         \n\
         `numeric(p,s)` is the recommended alternative for monetary values.\n\
         It has arbitrary precision, no locale dependency, and well-defined\n\
         arithmetic behavior.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE orders (total money NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE orders (total numeric(12,2) NOT NULL);"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            match &stmt.node {
                IrNode::CreateTable(ct) => {
                    for col in &ct.columns {
                        if is_money(&col.type_name) {
                            findings.push(Finding {
                                rule_id: self.id().to_string(),
                                severity: self.default_severity(),
                                message: format!(
                                    "Column '{}' on '{}' uses the 'money' type. The money type \
                                     depends on the lc_monetary locale setting, making it \
                                     unreliable across environments. Use numeric(p,s) instead.",
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
                                if is_money(&col.type_name) {
                                    findings.push(Finding {
                                        rule_id: self.id().to_string(),
                                        severity: self.default_severity(),
                                        message: format!(
                                            "Column '{}' on '{}' uses the 'money' type. The money type \
                                             depends on the lc_monetary locale setting, making it \
                                             unreliable across environments. Use numeric(p,s) instead.",
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
                                if is_money(new_type) {
                                    findings.push(Finding {
                                        rule_id: self.id().to_string(),
                                        severity: self.default_severity(),
                                        message: format!(
                                            "Column '{}' on '{}' uses the 'money' type. The money type \
                                             depends on the lc_monetary locale setting, making it \
                                             unreliable across environments. Use numeric(p,s) instead.",
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
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_money_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("orders"),
            columns: vec![ColumnDef {
                name: "total".to_string(),
                type_name: TypeName::simple("money"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm104.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM104");
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("total"));
        assert!(findings[0].message.contains("money"));
        assert!(findings[0].message.contains("numeric"));
    }

    #[test]
    fn test_numeric_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("orders"),
            columns: vec![ColumnDef {
                name: "total".to_string(),
                type_name: TypeName::with_modifiers("numeric", vec![12, 2]),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm104.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_money_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "discount".to_string(),
                type_name: TypeName::simple("money"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm104.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("discount"));
        assert!(findings[0].message.contains("money"));
    }
}
